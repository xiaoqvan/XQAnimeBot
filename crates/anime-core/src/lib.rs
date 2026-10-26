// to_html_payload 的 json! 字面量有几十个键，serde_json 宏递归默认深度不够用。
#![recursion_limit = "512"]

//! 动漫插件的**领域层**：只做业务，不认识宿主、也不认识 TDLib。
//!
//! 分层是刻意的：
//!
//! ```text
//! fuyubot-plugin-anime (cdylib)   宿主桥：HostApi / TDLib / 命令路由 / 回调
//!         │
//!         ▼
//!     anime-core                  纯逻辑：RSS 解析、标题解析、番剧匹配、下载、转码、存储
//! ```
//!
//! 这样 `anime-core` 可以被 `cargo test` 直接跑（不需要 Telegram、不需要 tdjson、
//! 甚至不需要 MongoDB），而插件壳只负责把宿主事件灌进来。
//!
//! 出图**不在这里**：那是 ultralightGenImage 插件的通用能力，本 crate 只负责
//! 把业务数据整理成模板 payload（见 [`card`]），由插件壳去调 `image.render`。

pub mod ai;
pub mod bangumi;
pub mod card;
pub mod config;
pub mod logger;
pub mod matching;
pub mod models;
pub mod rss;
pub mod store;
pub mod title;

/// 领域层统一错误：都带「下一步该干什么」，别让调用方只看到 `error`。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("网络请求失败（{url}）：{source}")]
    Http {
        url: String,
        #[source]
        source: Box<ureq::Error>,
    },
    /// 同 [`Error::Http`]，但**带 HTTP 状态码**（`0` = 连接层失败，没拿到响应）。
    ///
    /// 给「按码决定是否重试」的调用方用：429 限流、502/503 抖动值得重试，
    /// 401/403/400 重试只是白等。
    #[error("网络请求失败（{url}）[HTTP {status}]：{source}")]
    HttpStatused {
        url: String,
        status: u16,
        #[source]
        source: Box<ureq::Error>,
    },
    #[error("响应解析失败（{what}）：{message}")]
    Parse { what: &'static str, message: String },
    #[error("qBittorrent 未登录：{0}")]
    QbAuth(String),
    #[error("qBittorrent 接口错误（{status}）：{body}")]
    QbStatus { status: u16, body: String },
    #[error("存储错误：{0}")]
    Store(String),
    #[error("外部命令失败（{program}）：{message}")]
    Command { program: String, message: String },
    #[error("配置无效：{0}")]
    Config(String),
    /// 跨插件服务调用失败（qBittorrent / ffmpeg / 出图）。
    ///
    /// 单独一个变体是为了让日志一眼看出「是宿主里的服务坏了」而不是业务逻辑错。
    #[error("服务调用失败（{service}）：{message}")]
    Service { service: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;
