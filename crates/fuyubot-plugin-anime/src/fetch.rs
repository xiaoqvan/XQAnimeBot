//! 抓一轮 RSS。
//!
//! 这一层只回答「这一轮拿到了什么」，不关心拿去干什么——对应 TS 的
//! `anime/rss/index.ts::fetchMergedRss`（三源并行 → 优先级合并去重 → 过滤）。
//!
//! 输出是**日志**，不是 Telegram 消息：这里是后台轮询的一环，往群里刷屏是错的。
//! 谁要给人看，谁自己去 `/progress` 之类的命令里问。
//!
//! 网络不能在命令回调或插件 `start` 里同步做——那会堵住宿主的投递线程，
//! 同进程所有插件一起卡住。所以它只被 `poll` 的后台线程调用。

use std::time::{Duration, Instant};

use anime_core::config::RssConfig;
use anime_core::log;
use anime_core::logger::Logger;
use anime_core::models::RssKind;
use anime_core::rss::{self, Http};
use anime_core::title;

/// 单源抓取的耗时上限。`fetch_all` 内部是并行的，所以这也是整轮的上限。
const TIMEOUT: Duration = Duration::from_secs(15);
/// 日志里附几条样本。多了没人看，还会把宿主日志冲掉。
const SAMPLE_LIMIT: usize = 3;
/// 单行截短长度。错误原文（ureq 的）能很长，日志里留个开头就够。
const SHORTEN: usize = 60;

/// 一个源的抓取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLine {
    pub name: String,
    pub count: usize,
    pub error: Option<String>,
}

impl SourceLine {
    fn describe(&self) -> String {
        match &self.error {
            Some(error) => format!("{} 失败（{}）", self.name, shorten(error, SHORTEN)),
            None => format!("{} {} 条", self.name, self.count),
        }
    }
}

/// 一轮抓取的结果。
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub sources: Vec<SourceLine>,
    /// 合并去重后的总条数。
    pub total: usize,
    /// 其中能解析出番名的条数（其余是规则表里没收录的字幕组）。
    pub usable: usize,
    /// 前几条的样子，给日志用。
    pub samples: Vec<String>,
    /// 归一化后的标题列表。**去重键**：下游拿它判断这条见过没有。
    ///
    /// 单独列出来是因为去重发生在 `poll` 层，而汇总本是为日志准备的——
    /// 不带上它，下游为了去重就得再遍历一遍原始条目。
    pub titles: Vec<String>,
    /// 解析成功的条目（下游拿它去查库/匹配）。
    ///
    /// 与 `titles` 不是一一对应：`titles` 覆盖所有合并后的原始条目（含解析失败的），
    /// 这里只有能进业务的那些。
    pub items: Vec<anime_core::models::AnimeItem>,
    pub elapsed_ms: u64,
}

impl Summary {
    /// 渲染成一行日志。
    pub fn render(&self) -> String {
        let mut text = format!(
            "RSS 一轮 {}｜三源：{}｜去重 {} 条，可解析 {} 条",
            format_elapsed(self.elapsed_ms),
            self.sources
                .iter()
                .map(SourceLine::describe)
                .collect::<Vec<_>>()
                .join("｜"),
            self.total,
            self.usable
        );
        for sample in &self.samples {
            text.push_str(&format!("\n    {sample}"));
        }
        text
    }
}

/// 抓一轮。
pub fn run(config: &RssConfig, logger: &Logger) -> Summary {
    let started = Instant::now();
    let http = Http::new(TIMEOUT);

    let report = rss::fetch_all(&http, config.max_items, &config.blacklist, logger);
    let sources = report
        .sources
        .iter()
        .map(|source| SourceLine {
            name: source.kind.as_str().to_owned(),
            count: source.count,
            error: source.error.clone(),
        })
        .collect::<Vec<_>>();

    // 去重键对所有条目都收集：解析不出番名的条目也占一次投放机会，
    // 不记进 `Seen` 就会在下一轮被再处理一遍。
    let titles = report
        .items
        .iter()
        .map(|item| title::clean_title(&item.title))
        .collect::<Vec<_>>();

    let mut usable = 0usize;
    let mut samples = Vec::new();
    let mut items = Vec::new();
    for item in &report.items {
        // bangumi.moe 的组名/磁力/多语言名都在 API 里，RSS 条目本身没有 `author`：
        // 不走 `enrich_bangumi` 就只能靠标题里的组标签猜，猜不到就整条丢掉。
        // dmhy / acgnx 的 `author` 就是组名，直接解析即可。
        let built = match item.kind {
            RssKind::Bangumi => rss::enrich_bangumi(&http, item, logger),
            RssKind::Dmhy | RssKind::Acgnx => rss::build_item(item, None),
        };
        if built.is_none() {
            continue;
        }
        usable += 1;
        if samples.len() < SAMPLE_LIMIT {
            samples.push(shorten(&item.title, SHORTEN));
        }
        if let Some(built) = built {
            items.push(built);
        }
    }

    // 宏的第二个参数是 `ident`：`log!(logger, Warn, ...)`，不是 `Level::Warn`。
    for failure in report.failures() {
        log!(logger, Warn, "{} 抓取失败", failure.kind.as_str());
    }

    Summary {
        sources,
        total: report.items.len(),
        usable,
        samples,
        titles,
        items,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

/// 毫秒 → 人话。RSS 一轮是秒级的，别报「1832 ms」这种。
fn format_elapsed(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{}.{} 秒", ms / 1000, ms % 1000 / 100)
    }
}

/// 单行截短：错误里如果有换行，会破坏日志排版。
fn shorten(text: &str, limit: usize) -> String {
    let single = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single.chars().count() <= limit {
        single
    } else {
        let mut result = single.chars().take(limit).collect::<String>();
        result.push('…');
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(name: &str, count: usize, error: Option<&str>) -> SourceLine {
        SourceLine {
            name: name.to_owned(),
            count,
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn render_is_one_line_plus_samples() {
        let summary = Summary {
            sources: vec![
                source("bangumi.moe", 18, None),
                source("动漫花园", 15, None),
                source("末日动漫", 6, Some("timeout")),
            ],
            total: 25,
            usable: 17,
            samples: vec!["[桜都字幕组] 死神 千年血战篇".to_owned()],
            titles: vec!["[桜都字幕组] 死神 千年血战篇".to_owned()],
            items: Vec::new(),
            elapsed_ms: 1900,
        };
        let text = summary.render();
        assert!(text.contains("bangumi.moe 18 条"), "{text}");
        assert!(text.contains("末日动漫 失败（timeout）"), "{text}");
        assert!(text.contains("去重 25 条，可解析 17 条"), "{text}");
        assert!(text.contains("[桜都字幕组] 死神 千年血战篇"), "{text}");
    }

    #[test]
    fn empty_round_still_renders() {
        let summary = Summary {
            sources: vec![source("动漫花园", 0, Some("DNS 失败"))],
            total: 0,
            usable: 0,
            samples: Vec::new(),
            titles: Vec::new(),
            items: Vec::new(),
            elapsed_ms: 12,
        };
        let text = summary.render();
        assert!(text.contains("去重 0 条，可解析 0 条"), "{text}");
    }

    #[test]
    fn elapsed_reads_in_seconds() {
        assert_eq!(format_elapsed(0), "0 ms");
        assert_eq!(format_elapsed(999), "999 ms");
        assert_eq!(format_elapsed(1900), "1.9 秒");
    }

    #[test]
    fn shorten_collapses_newlines() {
        assert_eq!(shorten("一行\n两行", 60), "一行 两行");
        assert_eq!(shorten("短", 60), "短");
        assert!(shorten(&"字".repeat(100), 10).ends_with('…'));
    }
}
