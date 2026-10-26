//! 运行时状态：插件启动后一直活着的那些东西。
//!
//! 配置的来源有三层，顺序固定（见 [`ConfigSource`]）：
//!
//! 1. **`plugin_start` 的 `config_toml` 内联** —— 测试 / 回放专用，保证单测确定性；
//! 2. **宿主全局配置的 `[plugin.xq.anime]` 段** —— 生产唯一真相，经
//!    `host.get_config("xq.anime", "")` 读整段（JSON）；
//! 3. **内置默认值** —— 没配也能起来，日志里说清「用的默认值」。
//!
//! ```json
//! { "config_toml": "[telegram]\n..." }   // 内联（测试/回放用）
//! {}                                      // 走宿主全局配置，没有就用默认值
//! ```
//!
//! 为什么把业务配置放进宿主全局配置：宿主 ABI v3 有 `get_config` / `set_config`，
//! 配置统一落在 `fuyubot.toml` 的 `[plugin.xq.anime]` 段，「谁的字段、谁能改」一目了然。
//! 凭据仍靠 `apply_env_overrides` 走环境变量，不写进配置文件。

use std::sync::Arc;

use anime_core::config::Config;
use anime_core::logger::Logger;
use fuyubot_abi::{HostApi, LogLevel};

use crate::poll::Stats;
use crate::store::HostStore;
use crate::td::{HostLog, Td};

/// 配置最终从哪一层来，启动日志里要能一眼看出来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// `plugin_start` 的 `config_toml` 内联（测试 / 回放）。
    Inline,
    /// 宿主全局配置的 `[plugin.xq.anime]` 段。
    Global,
    /// 内置默认值。
    Default,
}

impl ConfigSource {
    /// 给人看的来源描述，写进启动日志。
    pub fn describe(&self) -> &'static str {
        match self {
            ConfigSource::Inline => "内联 config_toml",
            ConfigSource::Global => "宿主全局 [plugin.xq.anime]",
            ConfigSource::Default => "内置默认值",
        }
    }
}

/// 启动时从宿主 JSON 里读出来的「配置在哪」。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Bootstrap {
    /// 内联 TOML 文本（优先级最高，测试 / 回放用）。
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
        let text = value
            .get("config_toml")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
        Self { config_toml: text }
    }

    /// 按 内联 TOML → 宿主全局 `[plugin.xq.anime]` → 内置默认值 的顺序拿到最终配置。
    ///
    /// `global_json` 是调用方从宿主读出的整段 JSON（`host.get_config("xq.anime", "")` 的
    /// 返回值）；为 `None` 或空串 / `null` 都当成「没配」落回默认值。把 host 依赖留在
    /// 调用方，这里就能纯函数地测三层优先级。
    pub fn resolve(&self, global_json: Option<&str>) -> Result<(Config, ConfigSource), String> {
        if let Some(text) = &self.config_toml {
            let config = Config::parse(text).map_err(|e| format!("内联配置解析失败：{e}"))?;
            return Ok((config, ConfigSource::Inline));
        }

        if let Some(raw) = global_json {
            let raw = raw.trim();
            if !raw.is_empty() && raw != "null" {
                let value: serde_json::Value = serde_json::from_str(raw)
                    .map_err(|e| format!("宿主全局配置不是合法 JSON：{e}"))?;
                let config = Config::from_json(&value).map_err(|e| e.to_string())?;
                return Ok((config, ConfigSource::Global));
            }
        }

        Ok((Config::default(), ConfigSource::Default))
    }

    /// 应用环境变量覆盖后返回最终配置。凭据一律走环境变量，不落配置文件。
    pub fn resolve_with_env(&self, global_json: Option<&str>) -> Result<(Config, ConfigSource), String> {
        let (mut config, source) = self.resolve(global_json)?;
        config.apply_env_overrides();
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
    /// 配置从哪一层来的。
    pub config_source: ConfigSource,
    /// 启动时刻，`/status` 用来算运行时长。
    pub started_at: chrono::DateTime<chrono::Local>,
}

impl Runtime {
    pub fn new(host: HostApi, config: Config, config_source: ConfigSource) -> Arc<Self> {
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
        self.host.log(
            LogLevel::Info,
            &format!(
                "[anime] 已启动｜配置={}｜RSS 间隔={}s｜番剧频道={}｜导航频道={}",
                self.config_source.describe(),
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
    fn inline_toml_wins_over_global() {
        let bootstrap = Bootstrap::parse(
            r#"{"config_toml":"[rss]\ninterval_secs = 42"}"#,
        );
        let (config, source) = bootstrap
            .resolve(Some(r#"{"rss":{"interval_secs":99}}"#))
            .expect("内联配置应当生效");
        assert_eq!(config.rss.interval_secs, 42);
        assert_eq!(source, ConfigSource::Inline);
    }

    #[test]
    fn global_json_is_parsed_when_no_inline() {
        let bootstrap = Bootstrap::parse("");
        let global = r#"{"telegram":{"anime_channel":-100123},"rss":{"interval_secs":7}}"#;
        let (config, source) = bootstrap.resolve(Some(global)).expect("全局配置应当生效");
        assert_eq!(config.telegram.anime_channel, -100123);
        assert_eq!(config.rss.interval_secs, 7);
        assert_eq!(source, ConfigSource::Global);
    }

    #[test]
    fn empty_or_null_global_falls_back_to_default() {
        let bootstrap = Bootstrap::parse("");
        for global in [None, Some(""), Some("null")] {
            let (_, source) = bootstrap.resolve(global).expect("应当回退默认值");
            assert_eq!(source, ConfigSource::Default, "global={global:?}");
        }
    }

    #[test]
    fn unknown_field_in_global_is_a_hard_error() {
        // 全局配置走 JSON，`deny_unknown_fields` 同样生效：拼错字段得炸出来。
        let bootstrap = Bootstrap::parse("");
        let err = bootstrap
            .resolve(Some(r#"{"telegram":{"anime_channle":1}}"#))
            .unwrap_err();
        assert!(err.contains("anime_channle"), "{err}");
    }

    #[test]
    fn blank_inline_is_treated_as_missing() {
        let bootstrap = Bootstrap::parse(r#"{"config_toml":"   "}"#);
        assert_eq!(bootstrap.config_toml, None);
    }

    #[test]
    fn uptime_reads_naturally() {
        assert_eq!(format_uptime(0), "0 秒");
        assert_eq!(format_uptime(59), "59 秒");
        assert_eq!(format_uptime(60), "1 分 0 秒");
        assert_eq!(format_uptime(3725), "1 时 2 分");
    }
}
