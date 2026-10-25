//! 配置。TS 版把这些放在 `database/set.json` + 环境变量里；这里统一成一份 TOML，
//! 字段名改成更自然的 snake_case（**这是配置，不是数据库文档，不需要兼容老格式**）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

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
    /// 管理群（审核按钮、错误通知）。
    pub admin_group_id: i64,
    /// 管理群里的话题（0 = 不用话题）。
    pub anime_group_thread_id: i64,
    pub nav_group_thread_id: i64,
    pub error_group_thread_id: i64,
    /// 主人 user id：只有他们能跑管理命令。
    #[serde(default)]
    pub owner_id: i64,
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
}

impl Default for MongoConfig {
    fn default() -> Self {
        // 库名与 TS 版对齐：TS 从 `MONGODB_URL` 里取库名、取不到时用 `fuyubot`。
        // 这里保持同样的缺省值，接老库时不用再改配置。
        Self {
            uri: "mongodb://127.0.0.1:27017".to_owned(),
            database: "fuyubot".to_owned(),
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
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            ffmpeg: "ffmpeg".to_owned(),
            ffprobe: "ffprobe".to_owned(),
            work_dir: String::new(),
            delete_source_after_remux: true,
            timeout_secs: 60 * 60 * 2,
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
            model: "deepseek-v4-flash".to_owned(),
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

    /// 配置定位顺序：显式路径 → 环境变量 `XQ_ANIME_CONFIG` → 当前目录 `xq-anime.toml`。
    ///
    /// 全都没有就返回默认配置 + `None`，让调用方能选择「按默认跑」还是「拒绝启动」。
    pub fn discover(explicit: Option<&Path>) -> Result<(Self, Option<PathBuf>)> {
        let candidates: Vec<PathBuf> = explicit
            .map(Path::to_path_buf)
            .into_iter()
            .chain(std::env::var_os("XQ_ANIME_CONFIG").map(PathBuf::from))
            .chain(std::iter::once(PathBuf::from("xq-anime.toml")))
            .collect();
        for path in candidates {
            if path.is_file() {
                return Ok((Self::load(&path)?, Some(path)));
            }
        }
        Ok((Self::default(), None))
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
        if let Ok(v) =
            std::env::var("DEEPSEEK_API_KEY").or_else(|_| std::env::var("OPENAI_API_KEY"))
        {
            self.ai.api_key = v;
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
}
