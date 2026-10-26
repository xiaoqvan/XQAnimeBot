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
///
/// `page` 是导航里的分页序号（指向导航频道里的文本消息）；集数资源本身在
/// `resources` 集合，不在这里。字段与 TS 逐字对齐，没有 `episode`。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NavMessage {
    #[serde(default)]
    pub page: i64,
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

/// `bangumi_users` 集合：Telegram 用户 ↔ Bangumi 账号的绑定。
///
/// 字段与 TS 版逐字对齐（camelCase）：`id` 是**内部自增序列**（`getNextSequence`），
/// Bangumi 用户 id 放在 `bgmUserId`；`tgUserId` 用于「这个 Telegram 用户绑的是哪个
/// BGM 账号」。OAuth 凭证（access/refresh token、过期时间）也都在这里。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BangumiUserDoc {
    /// 内部自增 id（唯一主键，用于 oauth state）。
    #[serde(default)]
    pub id: i64,
    /// Telegram 用户 id（绑定用，必需）。
    pub tg_user_id: i64,
    /// OAuth 访问凭证。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// OAuth 刷新凭证。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// 过期时间（秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<i64>,
    /// OAuth token 类型（如 `"Bearer"`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    /// Bangumi 平台返回的用户 id（bgm.tv 的 user id）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bgm_user_id: Option<i64>,
    /// OAuth scope 字符串。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// 最近一次授权（或刷新）时间。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

/// `cacheItem` 集合：待审核的新番缓存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheItem {
    pub id: i64,
    pub item: AnimeItem,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
}

/// 分段/相册上传时每条视频消息的引用（TS 的 `albumMessageType`）。
///
/// 字段与 TS 逐字对齐（全是 snake_case：`chat_id` / `message_id` / `videoid`…）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AlbumMessage {
    #[serde(default)]
    pub chat_id: i64,
    #[serde(default)]
    pub message_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<serde_json::Value>,
    /// TG 视频远程 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub videoid: Option<String>,
    /// TG 视频唯一 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_id: Option<String>,
}

/// `resources` / `cache_resources` 集合：某部番某集的资源（字幕组、消息、视频）。
///
/// TS 的 `EpisodeResourceDoc`——导航正文、内联取视频、集数纠正都从这张表读。
/// 字段名与 TS 逐字对齐（`anime_id` / `unique_id` / `cache_id` 是 snake_case，
/// `episodeId` / `createdAt` 是 camelCase，TS 本来就这么混排）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpisodeResourceDoc {
    /// Mongo ObjectId（字符串形态，读老库时可能有，写入时留空让驱动生成）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 所属动漫（Bangumi subject id）。
    pub anime_id: i64,
    /// 集数（字符串，如 `"01"` / `"SP"`）。
    pub episode: String,
    /// 对应 eps.id（章节 id）。
    #[serde(rename = "episodeId", default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<i64>,
    /// 字幕组列表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// 标题。
    pub title: String,
    /// 名称（解析用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names: Option<Vec<String>>,
    /// TG 单条消息。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<MessageRef>,
    /// TG 分段消息。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<AlbumMessage>>,
    /// 单视频 videoid（相册时取第一条）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub videoid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_id: Option<String>,
    /// 相册各段的 videoid（TS 只在有多段时才写）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub videoids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_ids: Option<Vec<String>>,
    /// 缓存 id（cache_resources 用，指向 cacheItem.id）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_id: Option<serde_json::Value>,
    /// 来源。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(rename = "createdAt", default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

/// `pendingReviews` 集合：待审核番剧文档。
///
/// TS 用自增 `id` 当主键（回调只带 `r=<id>`，详情回查文档）；结构化字段取代旧版
/// 的 `payload` blob。`matchDetail` / `episodeMatch` 是 LLM 与集数匹配的结论，阶段 3/7
/// 填充，这里先用 `Value` 承载（保持与 TS 的字段名一致，便于双向互通）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingReviewDoc {
    /// 自增 id（回调数据里携带）。
    #[serde(default)]
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// 审核状态。
    #[serde(default)]
    pub status: String,
    /// 原始 RSS 条目数据。
    pub item: Option<AnimeItem>,
    /// 匹配到的番剧信息。
    pub anime: Option<Anime>,
    /// 匹配到的章节 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<i64>,
    /// 展示用的集数编号（sort 字段）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_sort: Option<f64>,
    /// 已发送到动漫频道的消息数据。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_messages: Option<Vec<AlbumMessage>>,
    /// 发送的主消息 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_message: Option<MessageRef>,
    /// LLM 匹配详情。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_detail: Option<serde_json::Value>,
    /// 集数匹配结果。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_match: Option<serde_json::Value>,
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

    #[test]
    fn bangumi_user_reads_ts_camelcase_shape() {
        // newdbtest 真实形状：camelCase、id 是内部自增、bgmUserId 存 Bangumi 用户 id，
        // 没有 username/nickname。旧代码会报 missing field 'username'，这里必须能读。
        let raw = r#"{
            "id": 7,
            "tgUserId": 123456,
            "accessToken": "tok",
            "refreshToken": "ref",
            "expiresIn": 604800,
            "tokenType": "Bearer",
            "bgmUserId": 88888,
            "scope": "read",
            "authTime": "2026-10-27T00:00:00Z"
        }"#;
        let user: BangumiUserDoc = serde_json::from_str(raw).unwrap();
        assert_eq!(user.id, 7);
        assert_eq!(user.tg_user_id, 123456);
        assert_eq!(user.access_token.as_deref(), Some("tok"));
        assert_eq!(user.refresh_token.as_deref(), Some("ref"));
        assert_eq!(user.expires_in, Some(604800));
        assert_eq!(user.token_type.as_deref(), Some("Bearer"));
        assert_eq!(user.bgm_user_id, Some(88888));

        // 反向序列化也是 TS 认得的 camelCase 字段名。
        let back = serde_json::to_value(&user).unwrap();
        assert!(back.get("accessToken").is_some());
        assert!(back.get("bgmUserId").is_some());
        assert!(back.get("username").is_none(), "TS 不存 username");
    }

    #[test]
    fn episode_resource_reads_ts_camelcase_shape() {
        let raw = r#"{
            "anime_id": 400602,
            "episode": "01",
            "episodeId": 12345,
            "groups": ["喵萌奶茶屋"],
            "title": "[喵萌奶茶屋] 葬送的芙莉莲 [01]",
            "names": ["葬送的芙莉莲"],
            "message": {"chat_id": -100, "message_id": 5, "link": "https://t.me/c/1/5"},
            "messages": [{"chat_id": -100, "message_id": 6}],
            "videoid": "v1",
            "unique_id": "u1",
            "cache_id": 99,
            "source": "Baha",
            "createdAt": "2026-10-27T00:00:00Z"
        }"#;
        let res: EpisodeResourceDoc = serde_json::from_str(raw).unwrap();
        assert_eq!(res.anime_id, 400602);
        assert_eq!(res.episode, "01");
        assert_eq!(res.episode_id, Some(12345));
        assert_eq!(res.groups, vec!["喵萌奶茶屋".to_owned()]);
        assert_eq!(res.message.as_ref().unwrap().message_id, 5);
        assert_eq!(res.messages.as_ref().unwrap()[0].message_id, 6);

        let back = serde_json::to_value(&res).unwrap();
        assert!(back.get("anime_id").is_some());
        assert!(back.get("episodeId").is_some());
        assert!(back.get("cache_id").is_some());
    }

    #[test]
    fn pending_review_reads_ts_camelcase_shape() {
        let raw = r#"{
            "id": 3,
            "createdAt": "2026-10-27T00:00:00Z",
            "status": "pending",
            "item": {"title": "某番 [01]", "pubDate": "x", "magnet": "m", "link": "l", "team": "t", "fansub": ["t"], "names": ["某番"]},
            "anime": {"id": 400602, "name": "某番", "name_cn": "某番"},
            "episodeId": 12345,
            "episodeSort": 1.0,
            "sentMessages": [{"chat_id": -100, "message_id": 6}],
            "primaryMessage": {"chat_id": -100, "message_id": 6}
        }"#;
        let review: PendingReviewDoc = serde_json::from_str(raw).unwrap();
        assert_eq!(review.id, 3);
        assert_eq!(review.status, "pending");
        assert_eq!(review.item.as_ref().unwrap().title, "某番 [01]");
        assert_eq!(review.anime.as_ref().unwrap().id, 400602);
        assert_eq!(review.episode_id, Some(12345));
        assert_eq!(review.episode_sort, Some(1.0));
        assert_eq!(review.sent_messages.as_ref().unwrap()[0].message_id, 6);
        assert_eq!(review.primary_message.as_ref().unwrap().message_id, 6);
    }
}
