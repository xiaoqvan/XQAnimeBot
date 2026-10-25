//! 数据模型。**字段名与 TS 版 MongoDB 文档逐字对齐**——这是迁移的硬要求：
//! 老库直接接上就能跑，不需要写迁移脚本。`chat_id` / `createdAt` 这类混排
//! （TDLib 风格 snake + 业务侧 camel）在 TS 里本来就这样，这里照抄，不做统一。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 导航频道里那条「番剧导航」消息的引用（对应 TS 的 `messageType`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MessageRef {
    #[serde(default)]
    pub chat_id: i64,
    #[serde(default)]
    pub message_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// 单集资源消息引用（TS 的 `navMessageType`，带 `page` 页码）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NavMessage {
    #[serde(default)]
    pub page: i64,
    /// 这一条对应第几集（`01` / `SP`…）。TS 的导航文本按它排序展示。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<String>,
    #[serde(default)]
    pub chat_id: i64,
    #[serde(default)]
    pub message_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// `anime` 集合。`id` 是 Bangumi subject id。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Anime {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_cn: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// 总集数：TS 版这里是字符串（`"12"` / `"12.5"` 都可能），保持字符串以免丢精度。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<String>,
    /// 评分：老库里既有数字也有字符串，用 [`Scalar`] 原样存取。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<Scalar>,
    #[serde(
        rename = "navMessage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub nav_message: Option<MessageRef>,
    #[serde(
        rename = "navVideoMessage",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub nav_video_message: Vec<NavMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r18: Option<bool>,
    #[serde(rename = "airingDay", default, skip_serializing_if = "Option::is_none")]
    pub airing_day: Option<String>,
    #[serde(
        rename = "airingStart",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub airing_start: Option<String>,
    #[serde(
        rename = "navImageHash",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub nav_image_hash: Option<String>,
    #[serde(rename = "createdAt", default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(rename = "updatedAt", default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

impl Anime {
    /// 展示名：优先中文名。
    pub fn display_name(&self) -> &str {
        self.name_cn
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(self.name.as_deref())
            .unwrap_or("(未命名)")
    }

    /// 全部可用于匹配的名字（names + name + name_cn，去重保序）。
    pub fn all_names(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for candidate in self
            .names
            .iter()
            .map(String::as_str)
            .chain(self.name.as_deref())
            .chain(self.name_cn.as_deref())
        {
            if !candidate.trim().is_empty() && !out.contains(&candidate) {
                out.push(candidate);
            }
        }
        out
    }
}

/// 数字 / 字符串两态标量：老库 `score` 两种类型都出现过。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Scalar {
    Num(f64),
    Text(String),
}

impl Scalar {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Num(n) => Some(*n),
            Self::Text(s) => s.trim().parse().ok(),
        }
    }
}

/// 种子/任务状态机。**中文常量与 TS 版完全一致**，库里已有数据不用改。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TorrentStatus {
    #[serde(rename = "等待下载")]
    PendingDownload,
    #[serde(rename = "等待元数据")]
    PendingMetadata,
    #[serde(rename = "下载中")]
    Downloading,
    #[serde(rename = "下载完成")]
    Downloaded,
    #[serde(rename = "上传中")]
    Uploading,
    #[serde(rename = "完成")]
    Done,
    #[serde(rename = "失败")]
    Failed,
    #[serde(rename = "等待纠正")]
    PendingFix,
}

impl TorrentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingDownload => "等待下载",
            Self::PendingMetadata => "等待元数据",
            Self::Downloading => "下载中",
            Self::Downloaded => "下载完成",
            Self::Uploading => "上传中",
            Self::Done => "完成",
            Self::Failed => "失败",
            Self::PendingFix => "等待纠正",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.as_str() == text)
    }

    pub const ALL: [Self; 8] = [
        Self::PendingDownload,
        Self::PendingMetadata,
        Self::Downloading,
        Self::Downloaded,
        Self::Uploading,
        Self::Done,
        Self::Failed,
        Self::PendingFix,
    ];
}

/// `torrents` 集合：`title` 是唯一键（写入前一律 [`crate::title::clean_title`]）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentDoc {
    pub title: String,
    #[serde(rename = "magnetLink")]
    pub magnet_link: String,
    /// 磁力 infoHash（小写十六进制），跨标题变体去重用。
    #[serde(rename = "infoHash", default, skip_serializing_if = "Option::is_none")]
    pub info_hash: Option<String>,
    pub status: TorrentStatus,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

/// `episodes_meta` 集合：Bangumi 章节元数据。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpisodeMeta {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub ep: f64,
    #[serde(default)]
    pub sort: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airdate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_cn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<i64>,
    #[serde(default)]
    pub subject_id: i64,
}

impl EpisodeMeta {
    /// Bangumi 定义：`type == 0` 才是本篇（1=SP，2=OP，3=ED，4=预告，5=MAD，6=其他）。
    pub fn is_main(&self) -> bool {
        matches!(self.r#type, None | Some(0))
    }
}

/// RSS 源条目：三源共用一套字段，`kind` 区分来源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssItem {
    pub kind: RssKind,
    /// bangumi.moe 的 torrent id（其他源为空）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub title: String,
    pub link: String,
    #[serde(rename = "pubDate")]
    pub pub_date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub magnet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

impl RssItem {
    /// 这条条目能拿到的「种子地址」：bangumi 是 .torrent，其余是磁力。
    pub fn download_source(&self) -> Option<&str> {
        match self.kind {
            RssKind::Bangumi => self.torrent.as_deref(),
            _ => self.magnet.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RssKind {
    Bangumi,
    Dmhy,
    Acgnx,
}

impl RssKind {
    /// 合并去重时的优先级：bangumi > dmhy > acgnx（数字大的赢）。
    pub fn priority(self) -> u8 {
        match self {
            Self::Bangumi => 3,
            Self::Dmhy => 2,
            Self::Acgnx => 1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bangumi => "bangumi",
            Self::Dmhy => "dmhy",
            Self::Acgnx => "acgnx",
        }
    }
}

/// 解析完成的待处理条目（TS 的 `animeItem`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimeItem {
    pub title: String,
    #[serde(rename = "pubDate")]
    pub pub_date: String,
    pub magnet: String,
    pub link: String,
    /// 主字幕组（取标题里第一个）。
    pub team: String,
    pub fansub: Vec<String>,
    /// 标题解析出的番剧名（按优先级）。
    pub names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<String>,
}

impl From<RssItem> for AnimeItem {
    fn from(item: RssItem) -> Self {
        let team = item.author.clone().unwrap_or_default();
        let magnet = item.download_source().unwrap_or_default().to_owned();
        Self {
            title: item.title,
            pub_date: item.pub_date,
            magnet,
            link: item.link,
            team,
            fansub: Vec::new(),
            names: Vec::new(),
            source: None,
            episode: None,
        }
    }
}

/// `task_checkpoints`：崩溃恢复用的任务中继（字段与 TS 的 `TaskCheckpointDoc` 一致）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCheckpoint {
    /// 唯一键：RSS/MKV 用种子 title，web 用 `web:<taskId>`。
    pub key: String,
    pub kind: CheckpointKind,
    /// 恢复所需完整载荷：`{"kind":"rss","item":{...}}`。
    pub payload: serde_json::Value,
    pub stage: String,
    pub status: CheckpointStatus,
    pub attempt: i32,
    #[serde(
        rename = "torrentHash",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub torrent_hash: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointKind {
    Rss,
    Mkv,
    Web,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointStatus {
    Queued,
    Running,
    Done,
    Failed,
    Canceled,
}

impl CheckpointStatus {
    /// 只有这两个状态会在重启后被恢复——`failed` 是「正常失败」，不该反复重放。
    pub fn is_resumable(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

/// `cacheItem` 集合：待审核的新番缓存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheItem {
    pub id: i64,
    pub item: AnimeItem,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torrent_status_round_trips_over_the_wire() {
        for status in TorrentStatus::ALL {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, format!("\"{}\"", status.as_str()));
            assert_eq!(TorrentStatus::parse(status.as_str()), Some(status));
        }
    }

    #[test]
    fn score_accepts_both_shapes() {
        let num: Scalar = serde_json::from_str("7.8").unwrap();
        let text: Scalar = serde_json::from_str("\"7.8\"").unwrap();
        assert_eq!(num.as_f64(), Some(7.8));
        assert_eq!(text.as_f64(), Some(7.8));
    }

    #[test]
    fn anime_document_keeps_legacy_field_names() {
        let raw = r#"{
            "id": 123,
            "name": "テスト",
            "name_cn": "测试",
            "names": ["テスト", "测试"],
            "navMessage": {"chat_id": -100, "message_id": 5},
            "navVideoMessage": [{"page": 1, "chat_id": -100, "message_id": 6}],
            "navImageHash": "abc",
            "airingDay": "周五",
            "airingStart": "2026年1月16日"
        }"#;
        let anime: Anime = serde_json::from_str(raw).unwrap();
        assert_eq!(anime.nav_message.as_ref().unwrap().message_id, 5);
        assert_eq!(anime.nav_video_message[0].page, 1);
        assert_eq!(anime.nav_image_hash.as_deref(), Some("abc"));
        // 反向序列化也必须还是 TS 认得的驼峰字段名。
        let back = serde_json::to_value(&anime).unwrap();
        assert!(back.get("navMessage").is_some());
        assert!(back.get("airingStart").is_some());
        assert!(back.get("navVideoMessage").is_some());
    }

    #[test]
    fn checkpoint_status_is_resumable_only_when_unfinished() {
        assert!(CheckpointStatus::Queued.is_resumable());
        assert!(CheckpointStatus::Running.is_resumable());
        assert!(!CheckpointStatus::Failed.is_resumable());
        assert!(!CheckpointStatus::Done.is_resumable());
    }
}
