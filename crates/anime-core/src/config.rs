//! 配置。TS 版把这些放在 `database/set.json` + 环境变量里；这里统一成一份 TOML，
//! 字段名改成更自然的 snake_case（**这是配置，不是数据库文档，不需要兼容老格式**）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// serde 的默认值：布尔字段不写时默认 true（与 `Default::default()` 保持一致）。
fn default_true() -> bool {
    true
}

/// 插件总配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub telegram: TelegramConfig,
    pub qbittorrent: QbConfig,
    pub mongodb: MongoConfig,
    pub rss: RssConfig,
    pub bangumi: BangumiConfig,
    pub media: MediaConfig,
    pub ai: AiConfig,
    pub http: HttpConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelegramConfig {
    /// 导航频道（番剧导航消息发这里）。
    pub nav_channel: i64,
    /// 正片频道（每集视频发这里）。
    pub anime_channel: i64,
    /// 缓冲频道/群：拿不准的片子（低置信度新番、集数没对齐）先发这里，管理员
    /// 确认后才转进正片频道。
    ///
    /// 为 0 时退回 TS 的落点：管理群（`admin_group_id`）的番剧话题
    /// （`anime_group_thread_id`）——TS 的 `sendMegToCache` 就是发那儿。
    /// 单独配一个频道/群时，话题仍用 `anime_group_thread_id`。
    ///
    /// 注意不能回退到 `anime_channel`：把没确认过的片子直接丢进正片频道，正是
    /// 缓冲频道要避免的事（发出去就撤不回了）。
    pub cache_channel: i64,
    /// 管理群（审核按钮、错误通知）。
    pub admin_group_id: i64,
    /// 管理群里的话题（0 = 不用话题）。
    pub anime_group_thread_id: i64,
    pub nav_group_thread_id: i64,
    pub error_group_thread_id: i64,
    /// 主人 user id：只有他们能跑管理命令。
    #[serde(default)]
    pub owner_id: i64,
    /// 发消息后是否等 `messageSendSucceeded` 再算成功。
    ///
    /// 默认 true（生产）：等到确认才能拿 message_id 回写导航。
    /// 回放/无 TDLib 的环境里等不到确认，关掉后退化成「投递即成功」。
    #[serde(default = "default_true")]
    pub wait_send: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QbConfig {
    /// 例：`http://127.0.0.1:8080`
    pub host: String,
    pub username: String,
    pub password: String,
    /// 下载落到哪个目录（交给 qB 自己去落盘，插件只读）。
    pub save_path: String,
    /// 等待下载完成的最长秒数。
    pub download_timeout_secs: u64,
    /// 发送成功后是否把种子和文件一起从 qB 删掉（TS 的 cleanupLocalMedia 行为）。
    /// 关掉的话种子会继续做种占磁盘，源文件被删后还会变 missingFiles。
    pub delete_torrent_after_send: bool,
    pub poll_interval_secs: u64,
}

impl Default for QbConfig {
    fn default() -> Self {
        Self {
            host: "http://127.0.0.1:8080".to_owned(),
            username: String::new(),
            password: String::new(),
            save_path: String::new(),
            download_timeout_secs: 60 * 60 * 6,
            delete_torrent_after_send: true,
            poll_interval_secs: 5,
        }
    }
}

impl QbConfig {
    pub fn is_configured(&self) -> bool {
        !self.host.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MongoConfig {
    pub uri: String,
    pub database: String,
    /// 连接/选择超时（毫秒）。云端抖动时宁可快速失败，默认 10 秒。
    ///
    /// 之前这个字段在 `xq-anime.toml.example` 里有、但结构体没有，照抄样例会被
    /// `deny_unknown_fields` 拒掉起不来；下发 `storage.mongo` 时又写死 `10_000`。
    /// 现在把它接成真正的可配值。
    pub timeout_ms: u64,
}

impl Default for MongoConfig {
    fn default() -> Self {
        // 库名与 TS 版对齐：TS 从 `MONGODB_URL` 里取库名、取不到时用 `fuyubot`。
        // 这里保持同样的缺省值，接老库时不用再改配置。
        Self {
            uri: "mongodb://127.0.0.1:27017".to_owned(),
            database: "fuyubot".to_owned(),
            timeout_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RssConfig {
    /// 轮询间隔（秒）。**`0` 表示按北京时间自动分时段**——夜里新番集中更新时
    /// 一分钟一轮，白天十五分钟一轮（见 `fuyubot-plugin-anime` 的 `poll` 模块）。
    ///
    /// 非 0 则固定为这个值，自动分时段不生效。**默认 0**：手动固定间隔等于
    /// 拿 CPU 换延迟，不该是默认行为。
    pub interval_secs: u64,
    /// 每个源最多取多少条（TS 版 dmhy 是 50）。
    pub max_items: usize,
    /// 并发 worker 数（TS 版靠 promise 池，这里用固定槽位）。
    pub workers: usize,
    /// MKV（需要烧录字幕）的独立队列槽位。
    pub mkv_workers: usize,
    /// 黑名单关键词。
    pub blacklist: Vec<String>,
    /// 是否启用 AI 兜底解析集数。
    pub ai_episode_fallback: bool,
}

impl Default for RssConfig {
    fn default() -> Self {
        Self {
            interval_secs: 0,
            max_items: 50,
            workers: 3,
            mkv_workers: 1,
            blacklist: Vec::new(),
            ai_episode_fallback: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BangumiConfig {
    /// bangumi.tv 的 OAuth App。
    pub app_id: String,
    pub app_secret: String,
    pub access_token: String,
    /// 拉章节信息用的 UA（Bangumi 要求带可识别的 UA）。
    pub user_agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaConfig {
    pub ffmpeg: String,
    pub ffprobe: String,
    /// 临时工作目录；空则用系统 temp 下的 `xq-anime`。
    pub work_dir: String,
    /// 转码后是否删除源文件。
    pub delete_source_after_remux: bool,
    /// 单个视频最长处理秒数。
    pub timeout_secs: u64,
    /// 单条消息能发的最大字节数；超过就按大小分段（TG 约 2GB）。
    pub max_upload_bytes: u64,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            ffmpeg: "ffmpeg".to_owned(),
            ffprobe: "ffprobe".to_owned(),
            work_dir: String::new(),
            delete_source_after_remux: true,
            timeout_secs: 60 * 60 * 2,
            max_upload_bytes: 1_950_000_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub enabled: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "https://api.deepseek.com".to_owned(),
            // 官方当前推荐名（由 V4.1-Flash 提供服务）。旧名 `deepseek-v4-flash`
            // 仍被接受但对应模型已退役，别再写它。
            model: "deepseek-flash".to_owned(),
            enabled: false,
        }
    }
}

impl AiConfig {
    pub fn usable(&self) -> bool {
        self.enabled && !self.api_key.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpConfig {
    /// 管理 API 监听地址；空表示不启动。
    pub listen: String,
    /// 访问令牌（`Authorization: Bearer <token>`）。
    pub token: String,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            listen: String::new(),
            token: String::new(),
        }
    }
}

impl Config {
    /// 从 TOML 文本解析。
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).map_err(|e| Error::Config(e.to_string()))
    }

    /// 从文件读；文件不存在则报错（不静默用默认值——配置错了要立刻看得见）。
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| {
            Error::Config(format!(
                "读取配置 {} 失败：{e}（先跑一次 init 生成）",
                path.display()
            ))
        })?;
        Self::parse(&text)
    }

    /// 从 JSON 值解析——宿主全局配置的 `[plugin.xq.anime]` 段给的是 JSON 不是 TOML。
    ///
    /// `deny_unknown_fields` 对 JSON 同样生效：字段对齐的约束不变，拼错照样炸出来。
    pub fn from_json(value: &serde_json::Value) -> Result<Self> {
        serde_json::from_value(value.clone())
            .map_err(|e| Error::Config(format!("解析 [plugin.xq.anime] JSON 失败：{e}")))
    }

    /// 环境变量覆盖：凭据一律允许走环境变量，避免写进仓库里的配置文件。
    pub fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("XQ_MONGO_URI") {
            self.mongodb.uri = v;
        }
        if let Ok(v) = std::env::var("XQ_MONGO_DB") {
            self.mongodb.database = v;
        }
        if let Ok(v) = std::env::var("XQ_QB_HOST") {
            self.qbittorrent.host = v;
        }
        if let Ok(v) = std::env::var("XQ_QB_USERNAME") {
            self.qbittorrent.username = v;
        }
        if let Ok(v) = std::env::var("XQ_QB_PASSWORD") {
            self.qbittorrent.password = v;
        }
        // 凭据：配置文件优先，环境变量只在配置文件没填时兜底。
        //
        // 反过来（环境变量无条件覆盖）会出现「配置里明明换了 key，实际还在用旧的」，
        // 排查时只看配置文件会完全对不上。这里保证「配置文件写什么就用什么」。
        if self.ai.api_key.trim().is_empty() {
            if let Ok(v) =
                std::env::var("DEEPSEEK_API_KEY").or_else(|_| std::env::var("OPENAI_API_KEY"))
            {
                self.ai.api_key = v;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_toml_falls_back_to_defaults() {
        let config = Config::parse(
            r#"
            [telegram]
            anime_channel = -100123
            "#,
        )
        .unwrap();
        assert_eq!(config.telegram.anime_channel, -100123);
        // 没写的段用默认值，不是 null。RSS 间隔默认 0 = 自动分时段。
        assert_eq!(config.rss.interval_secs, 0);
        assert_eq!(config.qbittorrent.host, "http://127.0.0.1:8080");
    }

    #[test]
    fn unknown_field_is_a_hard_error() {
        // 拼错的字段必须炸出来：静默忽略会让人配了半天不知道为什么没生效。
        let err = Config::parse("[telegram]\nanime_channle = 1\n").unwrap_err();
        assert!(err.to_string().contains("anime_channle"), "{err}");
    }

    #[test]
    fn missing_file_reports_the_path() {
        let err = Config::load("definitely-not-here.toml").unwrap_err();
        assert!(err.to_string().contains("definitely-not-here.toml"));
    }

    #[test]
    fn from_json_accepts_timeout_ms_and_full_section() {
        // 全局配置给的是 JSON（TOML 转过来的）。`timeout_ms` 之前在样例里存在但结构体
        // 没有，照抄样例会被 deny_unknown_fields 拒掉——这条测试钉住它现在是合法字段。
        let json = serde_json::json!({
            "telegram": { "anime_channel": -100123 },
            "mongodb": { "uri": "mongodb://x", "timeout_ms": 3000 },
            "rss": { "workers": 5, "mkv_workers": 2 }
        });
        let config = Config::from_json(&json).unwrap();
        assert_eq!(config.mongodb.timeout_ms, 3000);
        assert_eq!(config.mongodb.uri, "mongodb://x");
        assert_eq!(config.rss.workers, 5);
        assert_eq!(config.rss.mkv_workers, 2);
        assert_eq!(config.telegram.anime_channel, -100123);
    }

    #[test]
    fn from_json_rejects_unknown_field_like_toml() {
        let json = serde_json::json!({ "telegram": { "anime_channle": 1 } });
        let err = Config::from_json(&json).unwrap_err();
        assert!(err.to_string().contains("anime_channle"), "{err}");
    }
}
