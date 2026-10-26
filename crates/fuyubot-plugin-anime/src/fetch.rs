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

use anime_core::ai::AiClient;
use anime_core::config::{AiConfig, RssConfig};
use anime_core::log;
use anime_core::logger::Logger;
use anime_core::models::RssKind;
use anime_core::rss::{self, Http};
use anime_core::title;

/// 单源抓取的耗时上限。`fetch_all` 内部是并行的，所以这也是整轮的上限。
const TIMEOUT: Duration = Duration::from_secs(15);
/// 日志里附几条样本。多了没人看，还会把宿主日志冲掉。
const SAMPLE_LIMIT: usize = 3;
/// 内存里记住多少条标题。RSS 一轮几十条，这个容量够撑几十轮。
pub(crate) const SEEN_CAPACITY: usize = 2000;
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
    /// 本轮真正的新条目数（跨轮次去重后）。
    ///
    /// 去重放在解析**之前**：bangumi 条目每条要打 3 次 API（种子/发布组/标签），
    /// 对已经处理过的标题再解析一遍纯属浪费——一轮几十条时这是分钟级差距。
    pub fresh: usize,
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

/// 抓一轮。`seen` 由调用方持有：跨轮次去重在解析之前就挡掉老条目。
pub fn run(config: &RssConfig, ai: &AiConfig, logger: &Logger, seen: &mut Seen) -> Summary {
    let ai_episode_fallback = config.ai_episode_fallback;
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

    let mut fresh = 0usize;
    let mut usable = 0usize;
    let mut samples = Vec::new();
    let mut items = Vec::new();
    // AI 客户端建一次：每轮几十条，逐条 new 会白建连接池。
    let ai_client = AiClient::new(ai.clone());
    for item in &report.items {
        // 先去重再解析：老条目连 `enrich_bangumi` 都不该调。
        if !seen.accept(&title::clean_title(&item.title)) {
            continue;
        }
        fresh += 1;
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
        if let Some(mut built) = built {
            // AI 只做兜底：规则解析出集数后仍然问一次模型，模型给了更可信的结果才覆盖。
            // 开关关掉时连请求都不发。
            if ai_episode_fallback
                && ai_client.usable()
                && let Some(episode) = ai_client.extract_episode(&built.title, &built.names)
            {
                log!(
                    logger,
                    Debug,
                    "AI 修正集数：{} → {}（{}）",
                    built.episode.as_deref().unwrap_or("未知"),
                    episode,
                    built.title
                );
                built.episode = Some(episode);
            }
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
        fresh,
        items,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

/// 跨轮次去重。
///
/// 满了就整个清空：RSS 一轮几十条，几千条够撑几十轮；清空的代价是最早的一批标题
/// 会被再处理一次，而那些漏过去的会被 `db` 的持久去重拦住。为了省这点重复，
/// 不值得在内存里维护一个真环形缓冲——那是 `db` 该操心的事。
#[derive(Debug, Default)]
pub struct Seen {
    titles: std::collections::HashSet<String>,
}

impl Seen {
    /// 这条见过吗？没见过就记住并返回 `true`（= 是新条目）。
    pub fn accept(&mut self, title: &str) -> bool {
        if self.titles.contains(title) {
            return false;
        }
        if self.titles.len() >= SEEN_CAPACITY {
            self.titles.clear();
        }
        self.titles.insert(title.to_owned());
        true
    }

    /// 只在测试里用（断言容量上限）：正式代码没有「查去重表多大」的需求。
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.titles.len()
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
    fn seen_rejects_repeats_and_accepts_new() {
        let mut seen = Seen::default();
        assert!(seen.accept("A"));
        assert!(!seen.accept("A"), "同一条标题第二轮要被挡住");
        assert!(seen.accept("B"));
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn seen_clears_at_capacity() {
        let mut seen = Seen::default();
        for index in 0..SEEN_CAPACITY + 10 {
            seen.accept(&format!("标题 {index}"));
        }
        assert!(seen.len() <= SEEN_CAPACITY + 1, "容量要有上限：{}", seen.len());
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
            fresh: 1,
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
            fresh: 0,
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
