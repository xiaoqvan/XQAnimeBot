//! 领域层的 HTTP 出口：只有「GET 文本」和「POST JSON」两种用法，够 RSS 与 bangumi.moe 用。
//!
//! 为什么是 `ureq` 而不是 `reqwest`：这里的调用全是同步的（插件壳把它们放在自己的线程里跑），
//! `ureq` 的同步模型更贴合，也不会把 tokio 的运行时要求渗进领域层。
//!
//! 三个源都需要「浏览器 UA」——dmhy 没有 UA 会直接 403，bangumi.moe 的 API 也会挑 UA，
//! 所以 UA 固定在 [`Http::new`] 里，不留给调用方随手改。

use std::time::Duration;

use serde_json::Value;
use ureq::http::Response;
use ureq::{Agent, Body};

use crate::{Error, Result};

/// 与 TS 版 axios 调用时的 UA 对齐（dmhy 会按 UA 拦爬虫）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

/// 默认超时：TS 版 dmhy 是 15s，其余走 axios 默认（无）。统一 15s 够用。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// 共享的 HTTP 客户端。内部 `Agent` 带连接池，**一次进程建一个就够**。
#[derive(Clone)]
pub struct Http {
    agent: Agent,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Http")
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT)
    }
}

impl Http {
    pub fn new(timeout: Duration) -> Self {
        let config = Agent::config_builder()
            .user_agent(UA)
            .timeout_global(Some(timeout))
            .max_redirects(5)
            .build();
        Self {
            agent: config.new_agent(),
        }
    }

    /// GET 一段文本（RSS XML）。
    pub fn get(&self, url: &str) -> Result<String> {
        self.get_with(url, None)
    }

    /// GET 文本并带 `Referer`。dmhy 的 RSS 依赖 Referer 才给完整内容。
    pub fn get_with(&self, url: &str, referer: Option<&str>) -> Result<String> {
        let mut request = self.agent.get(url);
        if let Some(referer) = referer {
            request = request.header("Referer", referer);
        }
        let response = request.call().map_err(|source| Error::Http {
            url: url.to_owned(),
            source: Box::new(source),
        })?;
        read_body(response, url)
    }

    /// 带 Bearer 的 GET（Bangumi `/v0/me` 这类需要登录的接口）。
    pub fn get_json_auth(&self, url: &str, token: Option<&str>) -> Result<Value> {
        let mut request = self.agent.get(url);
        if let Some(token) = token.filter(|token| !token.trim().is_empty()) {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let response = request.call().map_err(|source| Error::Http {
            url: url.to_owned(),
            source: Box::new(source),
        })?;
        let text = read_body(response, url)?;
        serde_json::from_str(&text).map_err(|err| Error::Parse {
            what: "JSON 响应",
            message: format!("{url}：{err}"),
        })
    }

    /// 带 Bearer 的 PUT（Bangumi 章节收藏用）。
    pub fn put_json_auth(&self, url: &str, body: &Value, token: Option<&str>) -> Result<Value> {
        let payload = serde_json::to_string(body).map_err(|err| Error::Parse {
            what: "请求体",
            message: err.to_string(),
        })?;
        let mut request = self
            .agent
            .put(url)
            .header("Content-Type", "application/json");
        if let Some(token) = token.filter(|token| !token.trim().is_empty()) {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let response = request.send(payload.as_str()).map_err(|source| Error::Http {
            url: url.to_owned(),
            source: Box::new(source),
        })?;
        let text = read_body(response, url)?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|err| Error::Parse {
            what: "JSON 响应",
            message: format!("{url}：{err}"),
        })
    }

    /// POST 一个 JSON 并读回 JSON（bangumi.moe 的三次 `fetch` 接口）。
    pub fn post_json(&self, url: &str, body: &Value) -> Result<Value> {
        self.post_json_auth(url, body, None)
    }

    /// 带 Bearer 的 POST（OpenAI 兼容接口用）。`token` 为空等价于不带。
    pub fn post_json_auth(&self, url: &str, body: &Value, token: Option<&str>) -> Result<Value> {
        let payload = serde_json::to_string(body).map_err(|err| Error::Parse {
            what: "请求体",
            message: err.to_string(),
        })?;
        let mut request = self
            .agent
            .post(url)
            .header("Content-Type", "application/json");
        if let Some(token) = token.filter(|token| !token.trim().is_empty()) {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let response = request
            .send(payload.as_str())
            .map_err(|source| Error::Http {
                url: url.to_owned(),
                source: Box::new(source),
            })?;
        let text = read_body(response, url)?;
        serde_json::from_str(&text).map_err(|err| Error::Parse {
            what: "JSON 响应",
            message: format!("{url}：{err}"),
        })
    }
}

fn read_body(mut response: Response<Body>, url: &str) -> Result<String> {
    response
        .body_mut()
        .read_to_string()
        .map_err(|source| Error::Http {
            url: url.to_owned(),
            source: Box::new(source),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_is_cloneable_and_cheap_to_share() {
        // Agent 内部是 Arc：clone 不该建新连接池。这里只保证类型层面可用。
        let http = Http::default();
        let _clone = http.clone();
        assert!(format!("{http:?}") == "Http");
    }
}
