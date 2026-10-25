//! AI 兜底：集数抽取与章节匹配。
//!
//! 对应 TS 的 `utils/animeParser.ts::extractEpisodeByAI` 与
//! `bangumi/bangumiAgent.ts::aiEpisodeSearch`。区别只有一处：TS 把调用埋在两个
//! 大函数里，这里抽成独立模块，**关掉 AI 时整条链路完全不受影响**。
//!
//! 原则：AI 只做「兜底」，永远不是唯一判断。任何异常（没配 key、超时、返回不是
//! JSON、模型给了不存在的章节）都返回 `None`，调用方继续走规则逻辑。

use serde_json::{Value, json};

use crate::config::AiConfig;
use crate::rss::Http;

/// 请求超时。模型慢，但也不能让 RSS 一轮被一条卡住。
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// 客户端。`usable()` 为 false 时所有方法都直接返回 None，调用方不用自己判断。
#[derive(Debug, Clone)]
pub struct AiClient {
    config: AiConfig,
    http: Http,
}

impl AiClient {
    pub fn new(config: AiConfig) -> Self {
        Self {
            config,
            http: Http::new(TIMEOUT),
        }
    }

    /// 配置齐了没（enabled + api_key）。
    pub fn usable(&self) -> bool {
        self.config.usable()
    }

    /// 从标题里抽集数。返回规范化的集数字符串（`02v2` / `SP` / `剧场版` …）。
    pub fn extract_episode(&self, title: &str, names: &[String]) -> Option<String> {
        if !self.usable() {
            return None;
        }
        let hints = names.iter().take(5).cloned().collect::<Vec<_>>().join(" / ");
        let user = format!(
            "标题: {title}\n候选番名: {}\n可接受 episode 示例：01, 12, 02v2, SP, SP1, OVA2, 12.5, 剧场版, 电影。",
            if hints.is_empty() { "(空)" } else { &hints }
        );
        let reply = self.complete(
            "你是动漫资源标题的集数提取器。请从标题中提取单个集数字段，仅输出 JSON：{\"episode\":\"...\"}。如果无法确定则输出 {\"episode\":\"未知\"}。禁止输出解释。",
            &user,
        )?;
        let raw = reply.get("episode")?.as_str()?;
        normalize_episode(raw)
    }

    /// 从章节列表里挑出 BT 集数对应的那一条，返回章节 id。
    pub fn match_episode(&self, bt_episode: &str, episodes: &[(i64, f64)]) -> Option<i64> {
        if !self.usable() || episodes.is_empty() {
            return None;
        }
        let list = episodes
            .iter()
            .map(|(id, sort)| format!("{{\"id\":{id},\"sort\":{sort}}}"))
            .collect::<Vec<_>>()
            .join(",");
        let user = format!(
            "BT 集数: {bt_episode}\n章节列表（id, sort）: [{list}]\n只输出 JSON：{{\"id\":<章节id>}}，没有对应章节输出 {{\"id\":null}}。"
        );
        let reply = self.complete(
            "你是番剧章节匹配器。根据 BT 集数在给定的章节 sort 里找对应的一集，只输出 JSON。",
            &user,
        )?;
        let id = reply.get("id")?.as_i64()?;
        // 模型可能编一个不存在的 id：必须在列表里验证过才采信。
        episodes
            .iter()
            .any(|(candidate, _)| *candidate == id)
            .then_some(id)
    }

    fn complete(&self, system: &str, user: &str) -> Option<Value> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let body = json!({
            "model": self.config.model,
            "temperature": 0,
            "response_format": { "type": "json_object" },
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        let reply: Value = self
            .http
            .post_json_auth(&url, &body, Some(&self.config.api_key))
            .ok()?;
        let content = reply
            .get("choices")?
            .get(0)?
            .get("message")?
            .get("content")?
            .as_str()?;
        serde_json::from_str(content).ok()
    }

    /// 只读访问配置（测试与日志用）。
    pub fn config(&self) -> &AiConfig {
        &self.config
    }
}

/// 规范化模型返回的集数：只接受单集/特殊集形态，别让长句污染流程。
///
/// 规则与 TS 的 `normalizeAiEpisode` 一致：数字（可带 v2）、SP/OVA/…+数字、
/// 剧场版/电影，其余一律 `None`。
pub fn normalize_episode(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    if matches!(lower.as_str(), "未知" | "null" | "none" | "n/a") {
        return None;
    }
    if is_episode_number(value) || is_special(value) || is_decimal(value) || is_movie(value) {
        return Some(value.to_owned());
    }
    None
}

fn is_episode_number(value: &str) -> bool {
    let (digits, rest) = split_digits(value);
    // 最多 3 位：TS 的 `\d{1,3}`。4 位以上更可能是年份或分辨率。
    if digits.is_empty() || digits.len() > 3 || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    match rest.strip_prefix('v') {
        Some(version) => version.chars().all(|ch| ch.is_ascii_digit()) && !version.is_empty(),
        None => rest.is_empty(),
    }
}

fn is_special(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    for prefix in ["SP", "OVA", "OAD", "EXTRA", "番外", "特典"] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            if rest.chars().all(|ch| ch.is_ascii_digit()) && rest.len() <= 3 {
                return true;
            }
        }
    }
    false
}

fn is_decimal(value: &str) -> bool {
    let Some((left, right)) = value.split_once('.') else {
        return false;
    };
    !left.is_empty()
        && left.len() <= 3
        && left.chars().all(|ch| ch.is_ascii_digit())
        && right.len() == 1
        && right.chars().all(|ch| ch.is_ascii_digit())
}

fn is_movie(value: &str) -> bool {
    matches!(value, "剧场版" | "剧场总集篇" | "电影" | "SP")
}

/// 把 `02v2` 拆成数字与剩余部分；`02` → `("02", "")`。
fn split_digits(value: &str) -> (&str, &str) {
    let end = value
        .char_indices()
        .find(|(_, ch)| !ch.is_ascii_digit())
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    (&value[..end], &value[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_episode_forms() {
        assert_eq!(normalize_episode("01").as_deref(), Some("01"));
        assert_eq!(normalize_episode(" 12 ").as_deref(), Some("12"));
        assert_eq!(normalize_episode("02v2").as_deref(), Some("02v2"));
        assert_eq!(normalize_episode("SP1").as_deref(), Some("SP1"));
        assert_eq!(normalize_episode("OVA2").as_deref(), Some("OVA2"));
        assert_eq!(normalize_episode("12.5").as_deref(), Some("12.5"));
        assert_eq!(normalize_episode("剧场版").as_deref(), Some("剧场版"));
    }

    #[test]
    fn rejects_non_episode_answers() {
        assert!(normalize_episode("未知").is_none());
        assert!(normalize_episode("这个标题里没有集数").is_none());
        assert!(normalize_episode("").is_none());
        assert!(normalize_episode("第 5 集").is_none());
        assert!(normalize_episode("1234").is_none());
    }

    #[test]
    fn disabled_client_never_calls_out() {
        let client = AiClient::new(AiConfig::default());
        assert!(!client.usable());
        assert!(client.extract_episode("标题", &[]).is_none());
        assert!(client.match_episode("01", &[(1, 1.0)]).is_none());
    }
}
