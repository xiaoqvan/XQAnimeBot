//! Bangumi OAuth：只做 token 刷新（TS `bangumi/callback.ts::refreshAccessToken`）。
//!
//! 注意端点是 **bgm.tv** 而不是 api.bgm.tv；token 接口只认
//! `application/x-www-form-urlencoded`。`rss::Http` 只有 JSON POST，
//! 所以这一步单独用 `ureq` 发，错误仍然映射成 [`crate::Error::Http`]，
//! 消息里带 URL 与状态码。刷新成功后调用方自己决定是否落库；本层不碰存储。

use serde::Deserialize;

use super::{BgmClient, DEFAULT_TIMEOUT, OAUTH_TOKEN_URL, user_agent};
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
    pub fn bearer(&self) -> String {
        format!("{} {}", self.token_type, self.access_token)
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

        let agent = ureq::Agent::config_builder()
            .user_agent(user_agent(self.config()))
            .timeout_global(Some(DEFAULT_TIMEOUT))
            .max_redirects(5)
            .build()
            .new_agent();

        let mut response = agent
            .post(OAUTH_TOKEN_URL)
            .send_form(
                form.iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            )
            .map_err(|source| Error::Http {
                url: OAUTH_TOKEN_URL.to_owned(),
                source: Box::new(source),
            })?;
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|source| Error::Http {
                url: OAUTH_TOKEN_URL.to_owned(),
                source: Box::new(source),
            })?;

        serde_json::from_str(&text).map_err(|err| Error::Parse {
            what: "Bangumi Token",
            message: format!("{OAUTH_TOKEN_URL}：{err}"),
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
    }
}
