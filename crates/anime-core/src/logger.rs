//! 日志：领域层不认识宿主的 `HostApi::log`，所以定一个最小 trait，
//! 由插件壳实现成「转发到宿主日志」，测试里则退化到 `eprintln!`。

use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// 日志出口。实现必须是线程安全的：worker 线程都会写。
pub trait LogSink: Send + Sync {
    fn log(&self, level: Level, message: &str);
}

/// 共享日志器。用 `Arc<dyn LogSink>` 而不是泛型参数，是为了让
/// [`crate::db::Repo`] 之类的 trait object 也能持有它。
pub type Logger = Arc<dyn LogSink>;

/// 什么都不做的日志器（测试默认值）。
pub struct NoopLog;

impl LogSink for NoopLog {
    fn log(&self, _level: Level, _message: &str) {}
}

/// 打到 stderr 的日志器（`cargo test` / 本地冒烟用）。
pub struct StderrLog;

impl LogSink for StderrLog {
    fn log(&self, level: Level, message: &str) {
        eprintln!("[xq-anime][{}] {message}", level.as_str());
    }
}

pub fn noop() -> Logger {
    Arc::new(NoopLog)
}

pub fn stderr() -> Logger {
    Arc::new(StderrLog)
}

/// 便捷宏：`log!(logger, Warn, "…")`。比每次写 `logger.log(Level::Warn, &format!(..))` 短。
#[macro_export]
macro_rules! log {
    ($logger:expr, $level:ident, $($arg:tt)*) => {
        $logger.log($crate::logger::Level::$level, &format!($($arg)*))
    };
}
