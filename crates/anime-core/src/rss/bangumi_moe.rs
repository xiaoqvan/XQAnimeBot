//! bangumi.moe 的 JSON API（**不是** bangumi.tv，别混）。
//!
//! RSS 里只给 `.torrent` 地址和 torrent id，磁力、发布组、多语言番名都在 API 里：
//!
//! | 接口 | 用途 |
//! | --- | --- |
//! | `POST /api/torrent/fetch` `{"_id": id}` | 磁力、`team_id`、`tag_ids` |
//! | `POST /api/team/fetch` `{"_ids": [...]}` | 发布组名 |
//! | `POST /api/tag/fetch` `{"_ids": [...]}` | `type == "bangumi"` 的标签带中日英番名 |
//!
//! 这三个接口都是 POST + `_id`/`_ids`，且都要求浏览器 UA（[`super::Http`] 已固定）。

use serde::Deserialize;
use serde_json::json;

use crate::Result;
use crate::rss::Http;

const BASE: &str = "https://bangumi.moe/api";

/// 种子详情。只取我们要的字段，其余忽略。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TorrentInfo {
    #[serde(rename = "_id", default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub magnet: String,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub tag_ids: Vec<String>,
}

/// 发布组。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TeamInfo {
    #[serde(rename = "_id", default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// 标签。`type == "bangumi"` 的那条带番剧的多语言名。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TagInfo {
    #[serde(rename = "_id", default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub locale: Locale,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Locale {
    #[serde(default)]
    pub zh_cn: Option<String>,
    #[serde(default)]
    pub ja: Option<String>,
    #[serde(default)]
    pub en: Option<String>,
}

impl TagInfo {
    /// Bangumi 番剧标签（分类标签 `type` 是 `tag`，只有番剧标签才有多语言名）。
    pub fn is_bangumi(&self) -> bool {
        self.kind.as_deref() == Some("bangumi")
    }

    /// 非空的多语言名，顺序：中文 → 日文 → 英文（与 TS 版合并顺序一致）。
    pub fn names(&self) -> Vec<String> {
        [&self.locale.zh_cn, &self.locale.ja, &self.locale.en]
            .into_iter()
            .flatten()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

/// 拉一条种子详情。
pub fn fetch_torrent(http: &Http, id: &str) -> Result<TorrentInfo> {
    let url = format!("{BASE}/torrent/fetch");
    let value = http.post_json(&url, &json!({ "_id": id }))?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// 批量拉发布组。空 id 列表直接返回空，不打网络。
pub fn fetch_teams(http: &Http, ids: &[String]) -> Result<Vec<TeamInfo>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!("{BASE}/team/fetch");
    let value = http.post_json(&url, &json!({ "_ids": ids }))?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// 批量拉标签。空 id 列表直接返回空。
pub fn fetch_tags(http: &Http, ids: &[String]) -> Result<Vec<TagInfo>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!("{BASE}/tag/fetch");
    let value = http.post_json(&url, &json!({ "_ids": ids }))?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// 从标签列表里挑出番剧的多语言名（中文、日文、英文），去重保序。
pub fn bangumi_names(tags: &[TagInfo]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for tag in tags.iter().filter(|tag| tag.is_bangumi()) {
        for name in tag.names() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_torrent_payload() {
        let raw = r#"{
            "_id": "65f0a1b2c3d4e5f6",
            "title": "[喵萌奶茶屋] 某番 05",
            "magnet": "magnet:?xt=urn:btih:abc",
            "team_id": "team1",
            "tag_ids": ["tag1", "tag2"],
            "content": "...",
            "publish_time": "2026-10-25T13:00:00.000Z"
        }"#;
        let info: TorrentInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(info.id, "65f0a1b2c3d4e5f6");
        assert_eq!(info.magnet, "magnet:?xt=urn:btih:abc");
        assert_eq!(info.team_id.as_deref(), Some("team1"));
        assert_eq!(info.tag_ids.len(), 2);
    }

    #[test]
    fn missing_fields_do_not_explode() {
        let info: TorrentInfo = serde_json::from_str("{}").unwrap();
        assert!(info.magnet.is_empty());
        assert!(info.team_id.is_none());
        assert!(info.tag_ids.is_empty());
    }

    #[test]
    fn bangumi_tag_names_are_ordered_and_deduped() {
        let tags: Vec<TagInfo> = serde_json::from_str(
            r#"[
                {"_id":"t1","name":"分类","type":"tag","locale":{}},
                {"_id":"t2","name":"某番","type":"bangumi",
                 "locale":{"zh_cn":"某番","ja":"とある","en":"Some Anime"}},
                {"_id":"t3","name":"某番","type":"bangumi",
                 "locale":{"zh_cn":"某番","ja":"","en":""}}
            ]"#,
        )
        .unwrap();
        assert_eq!(bangumi_names(&tags), vec!["某番", "とある", "Some Anime"]);
    }

    #[test]
    fn empty_id_list_skips_network() {
        // 没有 HTTP 实例也能过：空列表在发请求前就返回了。
        let impossible = Http::new(std::time::Duration::from_millis(1));
        assert!(fetch_teams(&impossible, &[]).unwrap().is_empty());
        assert!(fetch_tags(&impossible, &[]).unwrap().is_empty());
    }
}
