//! Bangumi OAuth：只做 token 刷新（TS `bangumi/callback.ts::refreshAccessToken`）。
//!
//! 注意端点是 **bgm.tv** 而不是 api.bgm.tv；token 接口只认
//! `application/x-www-form-urlencoded`。`rss::Http` 只有 JSON POST，
//! 所以这一步单独用 `ureq` 发，错误仍然映射成 [`crate::Error::Http`]，
//! 消息里带 URL 与状态码。刷新成功后调用方自己决定是否落库；本层不碰存储。

use serde::Deserialize;

use super::{
    BgmClient, DEFAULT_TIMEOUT, OAUTH_AUTHORIZE_URL, OAUTH_TOKEN_STATUS_URL, OAUTH_TOKEN_URL,
    user_agent,
};
use crate::{Error, Result};

/// OAuth token 响应。字段与 TS 版 `refreshAccessToken` 的返回对齐。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Token {
    pub access_token: String,
    /// 有效期（秒）。
    pub expires_in: u64,
    pub token_type: String,
    /// Bangumi 在 refresh 响应里可能给 `null`。
    pub scope: Option<String>,
    pub refresh_token: Option<String>,
    pub user_id: Option<u64>,
}

impl Token {
    /// 给 `Authorization` 头用的值，例如 `Bearer xxx`。
    ///
    /// 服务端没给 `token_type` 时退回裸 token，避免拼出前面带空格的怪值。
    pub fn bearer(&self) -> String {
        let token_type = self.token_type.trim();
        if token_type.is_empty() {
            self.access_token.clone()
        } else {
            format!("{token_type} {}", self.access_token)
        }
    }
}

/// 刷新 token 的表单字段（顺序固定，方便测试和排查抓包）。
///
/// 与 TS 版一致：`grant_type` / `client_id` / `client_secret` / `refresh_token`。
/// 没有带 `redirect_uri`：RFC 6749 的 refresh 流程不需要，TS 版虽然带了，
/// 但那个地址是作者部署环境的，硬编码到通用客户端里反而会刷失败。
pub fn refresh_form(app_id: &str, app_secret: &str, refresh_token: &str) -> Vec<(String, String)> {
    vec![
        ("grant_type".to_owned(), "refresh_token".to_owned()),
        ("client_id".to_owned(), app_id.to_owned()),
        ("client_secret".to_owned(), app_secret.to_owned()),
        ("refresh_token".to_owned(), refresh_token.to_owned()),
    ]
}

/// 授权页 URL（TS `getAuthUrl` 的后半段）。
///
/// `state` 是库里那条绑定记录的**内部自增 id**：Bangumi 回调时会原样带回来，
/// 靠它才知道这次授权属于哪个 Telegram 用户。
pub fn authorize_url(app_id: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "{OAUTH_AUTHORIZE_URL}?client_id={}&response_type=code&redirect_uri={}&state={}",
        percent_encode(app_id),
        percent_encode(redirect_uri),
        percent_encode(state)
    )
}

/// 用 `code` 换 token 的表单（TS `getAccessToken`）。
///
/// **`redirect_uri` 必须和授权页用的一模一样**：OAuth 规范里它是换取 token 时的
/// 校验项之一，对不上就 400。所以这里不像 refresh 那样省掉它。
pub fn authorization_form(
    app_id: &str,
    app_secret: &str,
    code: &str,
    redirect_uri: &str,
    state: &str,
) -> Vec<(String, String)> {
    vec![
        ("grant_type".to_owned(), "authorization_code".to_owned()),
        ("client_id".to_owned(), app_id.to_owned()),
        ("client_secret".to_owned(), app_secret.to_owned()),
        ("code".to_owned(), code.to_owned()),
        ("redirect_uri".to_owned(), redirect_uri.to_owned()),
        ("state".to_owned(), state.to_owned()),
    ]
}

/// 查询串的百分号编码（RFC 3986 的 `unreserved` 之外全转义）。
///
/// 只用在 OAuth 这三条 URL 上：`app_id` / `redirect_uri` / `state` 都是 ASCII
/// 且通常无需转义，但 `redirect_uri` 里带 `:` `//` 是**合法的**、而空格不是——
/// 与其记住哪些字符安全，不如一律转义。UTF-8 字节逐个转义，与浏览器行为一致。
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// `oauth/token_status` 的响应（TS `getTokenStatus`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TokenStatus {
    pub access_token: String,
    /// Bangumi 返回的是**秒**（不是时间戳）：还剩多久过期。
    pub expires_in: i64,
    pub client_id: String,
    pub user_id: Option<u64>,
    pub scope: Option<String>,
}

impl BgmClient {
    /// 用配置里的 `app_id` / `app_secret` 刷新用户 token（最常用的入口）。
    pub fn refresh_token(&self, refresh_token: &str) -> Result<Token> {
        let config = self.config();
        self.refresh_token_with(&config.app_id, &config.app_secret, refresh_token)
    }

    /// 显式给应用凭据的版本：给「应用凭据不跟客户端配置走」的场景留的口子。
    ///
    /// 应用凭据为空时也照常发请求：Bangumi 会回 400/401，错误里带 URL 和状态，
    /// 比在本地猜「配置没填」更好排查。刷新成功后不会自动写回配置（`&self`），
    /// 由调用方保存新的 `access_token` / `refresh_token`。
    pub fn refresh_token_with(
        &self,
        app_id: &str,
        app_secret: &str,
        refresh_token: &str,
    ) -> Result<Token> {
        let form = refresh_form(app_id, app_secret, refresh_token);
        let text = self.post_form(OAUTH_TOKEN_URL, &form)?;
        serde_json::from_str(&text).map_err(|err| Error::Parse {
            what: "Bangumi Token",
            message: format!("{OAUTH_TOKEN_URL}：{err}"),
        })
    }

    /// 用授权码换 token（TS `getAccessToken`）。
    ///
    /// 成功后**不写库**：本层不认识存储，由调用方把 `access_token` /
    /// `refresh_token` / `expires_in` / `authTime` 存进 `bangumi_users`。
    pub fn exchange_code(&self, code: &str, redirect_uri: &str, state: &str) -> Result<Token> {
        let config = self.config();
        let form = authorization_form(
            &config.app_id,
            &config.app_secret,
            code,
            redirect_uri,
            state,
        );
        let text = self.post_form(OAUTH_TOKEN_URL, &form)?;
        serde_json::from_str(&text).map_err(|err| Error::Parse {
            what: "Bangumi Token",
            message: format!("{OAUTH_TOKEN_URL}：{err}"),
        })
    }

    /// 查授权信息（还剩多久过期、scope 是什么）。
    ///
    /// 注意 Bangumi 这个端点收的是 `application/x-www-form-urlencoded`
    /// （TS 版本虽然写了 `Content-Type: application/json`，发的也是表单串），
    /// 所以这里同样走 `post_form`。
    pub fn token_status(&self, access_token: &str) -> Result<TokenStatus> {
        let form = vec![("access_token".to_owned(), access_token.to_owned())];
        let text = self.post_form(OAUTH_TOKEN_STATUS_URL, &form)?;
        serde_json::from_str(&text).map_err(|err| Error::Parse {
            what: "Bangumi TokenStatus",
            message: format!("{OAUTH_TOKEN_STATUS_URL}：{err}"),
        })
    }

    /// OAuth 三个端点共用的表单 POST：`x-www-form-urlencoded` + 自报家门 UA。
    ///
    /// `rss::Http` 只有 JSON POST，所以这里单独用 `ureq`（与 refresh 的实现一致）。
    fn post_form(&self, url: &str, form: &[(String, String)]) -> Result<String> {
        let agent = ureq::Agent::config_builder()
            .user_agent(user_agent(self.config()))
            .timeout_global(Some(DEFAULT_TIMEOUT))
            .max_redirects(5)
            .build()
            .new_agent();

        let mut response = agent
            .post(url)
            .send_form(form.iter().map(|(key, value)| (key.as_str(), value.as_str())))
            .map_err(|source| Error::Http {
                url: url.to_owned(),
                source: Box::new(source),
            })?;
        response.body_mut().read_to_string().map_err(|source| Error::Http {
            url: url.to_owned(),
            source: Box::new(source),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_form_matches_the_oauth_spec() {
        let form = refresh_form("app-id", "app-secret", "refresh-token");
        assert_eq!(
            form,
            vec![
                ("grant_type".to_owned(), "refresh_token".to_owned()),
                ("client_id".to_owned(), "app-id".to_owned()),
                ("client_secret".to_owned(), "app-secret".to_owned()),
                ("refresh_token".to_owned(), "refresh-token".to_owned()),
            ]
        );
    }

    #[test]
    fn authorization_form_carries_the_same_redirect_uri_as_the_authorize_page() {
        let form = authorization_form("app-id", "app-secret", "code-1", "https://cb", "42");
        assert_eq!(
            form,
            vec![
                ("grant_type".to_owned(), "authorization_code".to_owned()),
                ("client_id".to_owned(), "app-id".to_owned()),
                ("client_secret".to_owned(), "app-secret".to_owned()),
                ("code".to_owned(), "code-1".to_owned()),
                // 与授权页一致：OAuth 规范里这是换取 token 的校验项，对不上就 400。
                ("redirect_uri".to_owned(), "https://cb".to_owned()),
                ("state".to_owned(), "42".to_owned()),
            ]
        );
    }

    #[test]
    fn authorize_url_escapes_the_redirect_uri_but_keeps_unreserved_chars() {
        let url = authorize_url("app id", "https://cb/?a=1&b=2", "42");
        assert!(url.starts_with("https://bgm.tv/oauth/authorize?client_id=app%20id"), "{url}");
        assert!(url.contains("response_type=code"), "{url}");
        // `redirect_uri` 里的 `:` `/` `?` `=` `&` 全部转义，浏览器才会原样回传。
        assert!(url.contains("redirect_uri=https%3A%2F%2Fcb%2F%3Fa%3D1%26b%3D2"), "{url}");
        assert!(url.ends_with("&state=42"), "{url}");
    }

    #[test]
    fn percent_encode_leaves_unreserved_characters_alone() {
        // RFC 3986 的 unreserved 集合：- _ . ~ 与字母数字。
        assert_eq!(percent_encode("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("中"), "%E4%B8%AD");
    }

    #[test]
    fn token_status_reads_remaining_seconds_not_a_timestamp() {
        // `expires_in` 是「还剩多少秒」，不是过期时间戳——名字容易看错。
        let status: TokenStatus = serde_json::from_str(
            r#"{"access_token":"abc","expires_in":604800,"client_id":"cid","user_id":7}"#,
        )
        .unwrap();
        assert_eq!(status.expires_in, 604_800);
        assert_eq!(status.client_id, "cid");
        assert_eq!(status.user_id, Some(7));
        assert_eq!(status.scope, None);
    }

    #[test]
    fn token_response_parses_optional_fields() {
        let token: Token = serde_json::from_str(
            r#"{
                "access_token": "new-access",
                "expires_in": 604800,
                "token_type": "Bearer",
                "scope": null,
                "refresh_token": "new-refresh",
                "user_id": 123456
            }"#,
        )
        .unwrap();
        assert_eq!(token.access_token, "new-access");
        assert_eq!(token.expires_in, 604_800);
        assert_eq!(token.token_type, "Bearer");
        assert_eq!(token.scope, None);
        assert_eq!(token.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(token.user_id, Some(123_456));
        assert_eq!(token.bearer(), "Bearer new-access");
    }

    #[test]
    fn minimal_token_response_falls_back_to_defaults() {
        let token: Token = serde_json::from_str(r#"{"access_token": "abc"}"#).unwrap();
        assert_eq!(token.access_token, "abc");
        assert_eq!(token.expires_in, 0);
        assert!(token.token_type.is_empty());
        assert!(token.refresh_token.is_none());
        assert_eq!(token.bearer(), "abc");
    }
}
