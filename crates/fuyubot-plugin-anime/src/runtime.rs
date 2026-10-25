//! 运行时状态：插件启动后一直活着的那些东西。
//!
//! 配置的来源有两层，顺序固定：
//!
//! 1. **宿主传给 `plugin_start` 的 JSON** —— 只用来定位业务配置，不承载业务字段；
//! 2. **业务配置文件（TOML）** —— `anime-core::config::Config` 是唯一真相。
//!
//! ```json
//! { "config_path": "D:/bots/xq/xq-anime.toml" }   // 显式路径
//! { "config_toml": "[telegram]\n..." }            // 内联（测试/回放用）
//! {}                                              // 走 discover：环境变量 → 当前目录
//! ```
//!
//! 为什么不把业务配置摊进宿主 JSON：那份 JSON 由宿主统一管理，塞进几十个业务字段之后
//! 「谁的字段、谁能改」就说不清了。凭据也要靠 `apply_env_overrides` 走环境变量。

use std::path::PathBuf;
use std::sync::Arc;

use anime_core::config::Config;
use anime_core::logger::Logger;
use fuyubot_abi::{HostApi, LogLevel};

use crate::poll::Stats;
use crate::store::HostStore;
use crate::td::{HostLog, Td};

/// 启动时从宿主 JSON 里读出来的「配置在哪」。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Bootstrap {
    /// 显式配置文件路径。
    pub config_path: Option<PathBuf>,
    /// 内联 TOML 文本（优先级高于 `config_path`）。
    pub config_toml: Option<String>,
}

impl Bootstrap {
    /// 解析宿主传进来的 `plugin_start` 配置。
    ///
    /// 空串 / `null` / 非法 JSON 都不算错误：插件没有配置也该能用默认值起来，
    /// 只是要在日志里说清楚「我用的是默认配置」。
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if raw.is_empty() {
            return Self::default();
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            return Self::default();
        };
        let text = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        Self {
            config_toml: text("config_toml"),
            config_path: text("config_path").map(PathBuf::from),
        }
    }

    /// 按 内联 TOML → 显式路径 → `discover` 的顺序拿到最终配置。
    pub fn resolve(&self) -> Result<(Config, Option<PathBuf>), String> {
        let mut config = match (&self.config_toml, &self.config_path) {
            (Some(text), _) => Config::parse(text).map_err(|e| format!("内联配置解析失败：{e}"))?,
            (None, Some(path)) => {
                Config::load(path).map_err(|e| format!("读取配置 {} 失败：{e}", path.display()))?
            }
            (None, None) => {
                Config::discover(None)
                    .map_err(|e| format!("查找配置失败：{e}"))?
                    .0
            }
        };
        config.apply_env_overrides();
        let source = self
            .config_toml
            .as_ref()
            .map(|_| None)
            .unwrap_or_else(|| self.config_path.clone());
        Ok((config, source))
    }
}

/// 插件启动后一直持有的状态。回调里靠 `Arc` 共享。
///
/// 里面**不放**订阅句柄和任务句柄——那些归 `AnimePlugin` 自己管，
/// 生命周期跟插件实例绑定，混进来会让 `stop` 的清理顺序变得难猜。
pub struct Runtime {
    pub host: HostApi,
    pub td: Td,
    /// 业务存储。领域层只认 `Storage` trait，这里是它在宿主里的实现。
    pub store: HostStore,
    pub config: Config,
    /// 领域层日志出口，转发到宿主日志。
    ///
    /// 后台线程和命令回调共用同一个 `Arc`，所以日志是交织的——这也正是想要的：
    /// 排查「这一轮抓了什么 / 用户当时敲了什么」时顺序就是真相。
    pub logger: Logger,
    /// 后台 RSS 轮询的累计计数。
    ///
    /// 计数放这里、线程句柄放 `AnimePlugin`：**数据是运行时状态，句柄是插件的生命周期**。
    /// 混在一起会让 `stop` 的清理顺序变得难猜。
    pub poll_stats: Arc<Stats>,
    /// 配置从哪个文件来的；`None` = 内置默认值。
    pub config_source: Option<PathBuf>,
    /// 启动时刻，`/status` 用来算运行时长。
    pub started_at: chrono::DateTime<chrono::Local>,
}

impl Runtime {
    pub fn new(host: HostApi, config: Config, config_source: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            host,
            td: Td::new(host),
            store: HostStore::new(host),
            config,
            logger: Arc::new(HostLog::new(host)),
            poll_stats: Arc::new(Stats::default()),
            config_source,
            started_at: chrono::Local::now(),
        })
    }

    /// 启动日志：让「我到底加载了什么配置」一眼可见。
    pub fn log_boot(&self) {
        let source = self.config_source.as_ref().map_or_else(
            || "内置默认值".to_owned(),
            |path| path.display().to_string(),
        );
        self.host.log(
            LogLevel::Info,
            &format!(
                "[anime] 已启动｜配置={source}｜RSS 间隔={}s｜番剧频道={}｜导航频道={}",
                self.config.rss.interval_secs,
                self.config.telegram.anime_channel,
                self.config.telegram.nav_channel
            ),
        );
    }

    /// 跑了多久，`/status` 用。
    pub fn uptime(&self) -> String {
        format_uptime(
            (chrono::Local::now() - self.started_at)
                .num_seconds()
                .max(0),
        )
    }
}

/// 秒数 → 人话。不足一分钟只报秒，别让 `/ping` 回一句「0 分 3 秒」。
fn format_uptime(seconds: i64) -> String {
    match (seconds / 3600, seconds % 3600 / 60) {
        (0, 0) => format!("{seconds} 秒"),
        (0, minutes) => format!("{minutes} 分 {} 秒", seconds % 60),
        (hours, minutes) => format!("{hours} 时 {minutes} 分"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_falls_back_to_defaults() {
        let bootstrap = Bootstrap::parse("");
        assert_eq!(bootstrap, Bootstrap::default());
        assert_eq!(Bootstrap::parse("null"), Bootstrap::default());
        assert_eq!(Bootstrap::parse("not json"), Bootstrap::default());
    }

    #[test]
    fn inline_toml_wins_over_path() {
        let bootstrap = Bootstrap::parse(
            r#"{"config_path":"D:/nope.toml","config_toml":"[rss]\ninterval_secs = 42"}"#,
        );
        let (config, source) = bootstrap.resolve().expect("内联配置应当生效");
        assert_eq!(config.rss.interval_secs, 42);
        assert_eq!(source, None);
    }

    #[test]
    fn blank_path_is_ignored() {
        let bootstrap = Bootstrap::parse(r#"{"config_path":"   "}"#);
        assert_eq!(bootstrap.config_path, None);
    }

    #[test]
    fn missing_config_path_reports_where() {
        let bootstrap = Bootstrap {
            config_path: Some(PathBuf::from("D:/definitely-not-here.toml")),
            config_toml: None,
        };
        let error = bootstrap.resolve().unwrap_err();
        assert!(
            error.contains("definitely-not-here"),
            "错误里要带路径：{error}"
        );
    }

    #[test]
    fn uptime_reads_naturally() {
        assert_eq!(format_uptime(0), "0 秒");
        assert_eq!(format_uptime(59), "59 秒");
        assert_eq!(format_uptime(60), "1 分 0 秒");
        assert_eq!(format_uptime(3725), "1 时 2 分");
    }
}
