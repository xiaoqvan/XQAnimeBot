//! Bangumi（bgm.tv / api.bgm.tv）v0 API 客户端。
//!
//! 对应 TS 版的四个文件：`bangumi/get.ts`（条目/章节）、`bangumi/set.ts`
//! （用户收藏，留给后续）、`bangumi/callback.ts`（OAuth 换 token / 刷新）与
//! `utils/getBgmToken.ts`（过期刷新）。这里只实现**读接口 + token 刷新**，
//! 写收藏需要用户 token，等命令层接入时再加。
//!
//! | 方法 | 接口 | 鉴权 |
//! | --- | --- | --- |
//! | [`BgmClient::subject`] | `GET /v0/subjects/{id}` | 公开 |
//! | [`BgmClient::episodes`] | `GET /v0/episodes?subject_id=…&limit=100` | 公开 |
//! | [`BgmClient::search`] | `POST /v0/search/subjects?limit=10` | 公开 |
//! | [`BgmClient::refresh_token`] | `POST https://bgm.tv/oauth/access_token` | app_id/secret |
//!
//! ## 为什么 OAuth 单独走 ureq
//!
//! [`crate::rss::Http`] 是给 RSS 用的最小客户端：只有 GET 文本和 POST JSON，
//! UA 也固定成浏览器 UA。Bangumi 的 token 端点要求
//! `application/x-www-form-urlencoded`（TS 版用 `URLSearchParams`），
//! 且 Bangumi 建议带可识别 UA，所以刷新 token 这一步直接用 `ureq` 发一次；
//! 错误依旧映射成 [`crate::Error::Http`]，消息里带 URL 与状态码。
//!
//! 条目 / 章节 / 搜索都是公开 JSON 接口，浏览器 UA 也能正常返回（实测 200），
//! 因此继续复用 `rss::Http`，不另造一套连接池。
//!
//! ## 已知限制（都是 `rss::Http` 的接口决定的）
//!
//! - `Http` 不能改 UA：`BangumiConfig::user_agent` 目前只用于 OAuth 表单请求；
//!   公开接口用 `Http` 固定的浏览器 UA。
//! - `Http` 不能加 `Authorization` 头：公开接口本来就不用登录，所以不附
//!   `access_token`。以后要做「用户收藏」这类带 token 的接口，要么先给
//!   `rss::Http` 加 header 能力，要么在 `oauth` 里用同一套 ureq 方式发。
//!
//! ## 分页
//!
//! `/v0/episodes` 一次最多 100 条。这里按 `offset` 一直翻到拿完为止：
//! 用响应里的 `total`、`limit`、`offset` 判断下一页，并且**强制 `next > offset`**，
//! 防止服务端忽略 offset 时死循环。分页判断抽成纯函数
//! [`EpisodesPage::next_offset`]，可以不联网单测。

pub mod oauth;
pub mod subject;

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::BangumiConfig;
use crate::rss::Http;
use crate::{Error, Result};

pub use oauth::{Token, refresh_form};
pub use subject::{
    Collection, Episode, Images, Infobox, InfoboxItem, InfoboxValue, Rating, STAFF_KEYS, Subject,
    Tag,
};

/// Bangumi 主站 API（**不是** bangumi.moe）。
const API_BASE: &str = "https://api.bgm.tv";

/// OAuth 端点在主站，不在 api.bgm.tv。
const OAUTH_TOKEN_URL: &str = "https://bgm.tv/oauth/access_token";

/// 配置里没写 UA 时的兜底：与 TS 版一致，Bangumi 要求能识别调用方。
const DEFAULT_USER_AGENT: &str = "XQAnimeBot/1.0 (https://github.com/xiaoqvan/XQAnimeBot)";

/// 与 `rss::Http` 默认值保持一致。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// 章节分页大小。Bangumi 的硬上限就是 100。
pub const EPISODE_PAGE_LIMIT: u32 = 100;

/// 搜索固定取前 10 条（与 TS 版 `animeinfo()` 一致）。
pub const SEARCH_LIMIT: u32 = 10;

/// Bangumi 条目类型：2 = 动画。
pub const SUBJECT_TYPE_ANIME: u32 = 2;

/// 客户端。内部持有共享的 [`Http`]（连接池可复用），配置只读。
#[derive(Clone)]
pub struct BgmClient {
    config: BangumiConfig,
    http: Http,
}

impl std::fmt::Debug for BgmClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BgmClient")
            .field("app_id", &self.config.app_id)
            .field(
                "has_access_token",
                &!self.config.access_token.trim().is_empty(),
            )
            .finish()
    }
}

impl BgmClient {
    /// 用默认超时建一个客户端。想和 RSS 共用连接池就用 [`Self::with_http`]。
    pub fn new(config: BangumiConfig) -> Self {
        Self {
            config,
            http: Http::default(),
        }
    }

    /// 复用调用方已有的 [`Http`]：插件壳里 RSS 与 Bangumi 可以共用一个 Agent。
    pub fn with_http(config: BangumiConfig, http: Http) -> Self {
        Self { config, http }
    }

    pub fn config(&self) -> &BangumiConfig {
        &self.config
    }

    /// 配置里的用户 token。公开接口用不到，留给后续的用户收藏/`/v0/me`。
    pub fn access_token(&self) -> &str {
        &self.config.access_token
    }

    /// 拉单个条目。
    pub fn subject(&self, id: u64) -> Result<Subject> {
        let url = subject_url(id);
        let value = self.get_json(&url)?;
        serde_json::from_value(value).map_err(|err| Error::Parse {
            what: "Bangumi 条目",
            message: format!("{url}：{err}"),
        })
    }

    /// 拉一个条目的全部章节。按 100 条一页翻到 `total` 为止。
    pub fn episodes(&self, subject_id: u64) -> Result<Vec<Episode>> {
        let mut all = Vec::new();
        let mut offset = 0u32;
        loop {
            let url = episodes_url(subject_id, offset);
            let value = self.get_json(&url)?;
            let page = parse_episodes_page(&url, &value)?;
            let next = page.next_offset();
            all.extend(page.data);
            match next {
                // 下一页必须前进，否则服务端没认 offset 时会永远转圈。
                Some(next) if next > offset => offset = next,
                _ => break,
            }
        }
        Ok(all)
    }

    /// 按关键词搜动画条目（`filter.type = [2]`，最多 10 条）。
    pub fn search(&self, keyword: &str) -> Result<Vec<Subject>> {
        let url = search_url();
        let value = self.http.post_json(&url, &search_body(keyword))?;
        parse_search_response(&url, value)
    }

    /// 取当前登录用户（`/v0/me`）。access_token 为空时直接报错，不打空请求。
    pub fn me(&self) -> Result<crate::bangumi::BangumiUser> {
        if self.config.access_token.trim().is_empty() {
            return Err(Error::Config(
                "未配置 bangumi.access_token，无法查询 Bangumi 用户".to_owned(),
            ));
        }
        let url = format!("{API_BASE}/v0/me");
        let value = self
            .http
            .get_json_auth(&url, Some(self.config.access_token.as_str()))?;
        serde_json::from_value(value).map_err(|err| Error::Parse {
            what: "Bangumi 用户",
            message: format!("{url}：{err}"),
        })
    }

    fn get_json(&self, url: &str) -> Result<Value> {
        let text = self.http.get(url)?;
        serde_json::from_str(&text).map_err(|err| Error::Parse {
            what: "Bangumi JSON",
            message: format!("{url}：{err}"),
        })
    }
}

pub use subject::BangumiUser;

/// 单页章节响应。字段名与 `/v0/episodes` 一致。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct EpisodesPage {
    pub data: Vec<Episode>,
    pub total: u32,
    pub limit: u32,
    pub offset: u32,
}

impl EpisodesPage {
    /// 下一页 offset；`None` 表示已经拿完（或服务端没有前进）。
    ///
    /// 三个停止条件：
    /// 1. 本页为空；
    /// 2. 本页不足 `limit`（最后一页）；
    /// 3. 已经到 `total`（`total` 缺失/为 0 时忽略）。
    pub fn next_offset(&self) -> Option<u32> {
        let fetched = u32::try_from(self.data.len()).unwrap_or(u32::MAX);
        if fetched == 0 {
            return None;
        }
        let next = self.offset.saturating_add(fetched);
        if next <= self.offset {
            return None;
        }
        let limit = if self.limit == 0 {
            EPISODE_PAGE_LIMIT
        } else {
            self.limit
        };
        if fetched < limit {
            return None;
        }
        if self.total > 0 && next >= self.total {
            return None;
        }
        Some(next)
    }
}

/// 解析一页 `/v0/episodes`。纯函数：测试用固定 JSON，不发网络请求。
pub fn parse_episodes_page(url: &str, value: &Value) -> Result<EpisodesPage> {
    serde_json::from_value(value.clone()).map_err(|err| Error::Parse {
        what: "Bangumi 章节列表",
        message: format!("{url}：{err}"),
    })
}

/// 搜索请求体：`sort=rank` + 只保留动画条目。字段与 TS 版 `animeinfo()` 对齐。
pub fn search_body(keyword: &str) -> Value {
    json!({
        "keyword": keyword,
        "sort": "rank",
        "filter": { "type": [SUBJECT_TYPE_ANIME] },
    })
}

fn subject_url(id: u64) -> String {
    format!("{API_BASE}/v0/subjects/{id}")
}

fn episodes_url(subject_id: u64, offset: u32) -> String {
    format!(
        "{API_BASE}/v0/episodes?subject_id={subject_id}&limit={EPISODE_PAGE_LIMIT}&offset={offset}"
    )
}

fn search_url() -> String {
    format!("{API_BASE}/v0/search/subjects?limit={SEARCH_LIMIT}")
}

fn parse_search_response(url: &str, value: Value) -> Result<Vec<Subject>> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct SearchResponse {
        data: Vec<Subject>,
    }

    serde_json::from_value::<SearchResponse>(value)
        .map(|response| response.data)
        .map_err(|err| Error::Parse {
            what: "Bangumi 搜索结果",
            message: format!("{url}：{err}"),
        })
}

fn user_agent(config: &BangumiConfig) -> &str {
    let configured = config.user_agent.trim();
    if configured.is_empty() {
        DEFAULT_USER_AGENT
    } else {
        configured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_match_the_v0_api() {
        assert_eq!(subject_url(400602), "https://api.bgm.tv/v0/subjects/400602");
        assert_eq!(
            episodes_url(400602, 100),
            "https://api.bgm.tv/v0/episodes?subject_id=400602&limit=100&offset=100"
        );
        assert_eq!(
            search_url(),
            "https://api.bgm.tv/v0/search/subjects?limit=10"
        );
    }

    #[test]
    fn search_body_filters_anime_and_sorts_by_rank() {
        let body = search_body("Frieren");
        assert_eq!(body["keyword"], "Frieren");
        assert_eq!(body["sort"], "rank");
        assert_eq!(body["filter"]["type"], json!([2]));
        // limit 在 URL 上，与 TS 版一致；别让请求体也带一份。
        assert!(body.get("limit").is_none());
    }

    #[test]
    fn episodes_page_walks_until_total() {
        let first_value = json!({
            "data": (0..100).map(|i| json!({
                "id": 1_227_000 + i,
                "type": 0,
                "ep": i + 1,
                "sort": i + 1,
                "name": format!("第{}话", i + 1),
                "name_cn": "",
                "airdate": "2023-09-29",
                "duration": "00:26:00",
                "desc": "",
            })).collect::<Vec<_>>(),
            "total": 120,
            "limit": 100,
            "offset": 0,
        });
        let first = parse_episodes_page("https://api.bgm.tv/v0/episodes", &first_value).unwrap();
        assert_eq!(first.data.len(), 100);
        assert_eq!(first.next_offset(), Some(100));

        let last_value = json!({
            "data": (100..120).map(|i| json!({
                "id": 1_227_000 + i,
                "type": 0,
                "ep": i + 1,
                "sort": i + 1,
            })).collect::<Vec<_>>(),
            "total": 120,
            "limit": 100,
            "offset": 100,
        });
        let last = parse_episodes_page("https://api.bgm.tv/v0/episodes", &last_value).unwrap();
        assert_eq!(last.data.len(), 20);
        assert_eq!(last.next_offset(), None);
    }

    #[test]
    fn empty_page_stops_pagination() {
        let empty = EpisodesPage {
            total: 120,
            limit: 100,
            offset: 120,
            data: Vec::new(),
        };
        assert_eq!(empty.next_offset(), None);
        // 服务端如果原样返回 offset=0，也不能让调用方原地打转。
        let stuck = EpisodesPage {
            total: 0,
            limit: 100,
            offset: 0,
            data: vec![Episode::default(); 100],
        };
        assert_eq!(stuck.next_offset(), Some(100));
    }

    #[test]
    fn short_page_stops_even_if_total_lies() {
        let page = EpisodesPage {
            total: 999,
            limit: 100,
            offset: 100,
            data: vec![Episode::default(); 20],
        };
        assert_eq!(page.next_offset(), None);
    }

    #[test]
    fn episodes_parse_error_mentions_url() {
        let err = parse_episodes_page(
            "https://api.bgm.tv/v0/episodes?subject_id=1",
            &json!({ "data": "not-an-array" }),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("https://api.bgm.tv/v0/episodes?subject_id=1"),
            "{text}"
        );
        assert!(matches!(err, Error::Parse { .. }));
    }

    #[test]
    fn search_response_parses_subjects() {
        let value = json!({
            "data": [{
                "id": 400602,
                "name": "葬送のフリーレン",
                "name_cn": "葬送的芙莉莲",
                "type": 2,
            }],
            "total": 1,
            "limit": 10,
            "offset": 0,
        });
        let subjects =
            parse_search_response("https://api.bgm.tv/v0/search/subjects", value).unwrap();
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0].id, 400602);
        assert_eq!(subjects[0].display_name(), "葬送的芙莉莲");
    }

    #[test]
    fn client_reads_config_without_network() {
        let config = BangumiConfig {
            app_id: "app".to_owned(),
            app_secret: "secret".to_owned(),
            access_token: "token".to_owned(),
            user_agent: "XQAnimeBot/test".to_owned(),
        };
        let client = BgmClient::new(config);
        assert_eq!(client.config().app_id, "app");
        assert_eq!(client.access_token(), "token");
        assert_eq!(user_agent(client.config()), "XQAnimeBot/test");
        assert_eq!(user_agent(&BangumiConfig::default()), DEFAULT_USER_AGENT);
    }
}
