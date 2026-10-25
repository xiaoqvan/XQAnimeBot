//! 后台 RSS 轮询。
//!
//! 对应 TS 的 `anime/index.ts::anime()`：一个 `while (true)` 循环，周期抓 RSS，
//! 抓到的条目交给下游，然后睡到下次该醒的时间。**没有 `/rss` 命令**——原项目里
//! 这就是个无人值守的后台任务，用户不敲任何命令它也在跑。
//!
//! 两件原项目有、这里必须照做的行为：
//!
//! 1. **智能间隔**（`getSmartDelayInfo`）：按北京时间分时段，夜里新番集中更新时
//!    一分钟一轮，白天十五分钟一轮。硬编一个固定间隔是拿 CPU 换延迟。
//! 2. **跨轮次去重**：RSS 里同一条种子会连续出现好几轮。真正的去重归 `db`
//!    （持久化、跨重启），这里只在内存里挡掉短期重复，挡不住也无妨——
//!    漏过去的会被 `db` 拦住。
//!
//! 并发 worker 池（TS 的 `AnimeProcessorManager`）**现在还没做**：下游目前只记账，
//! 秒级就完事，开并发反而让日志乱序。等下游变成下载+转码（分钟级）时再加。

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::{DateTime, Timelike, Utc};

use anime_core::log;
use anime_core::matching::{
    AnimeCandidate, MatchPolicy, SeasonDecision, decide_season_with, match_item_with, today_local,
};
use anime_core::store::Storage;
use anime_core::title;

use crate::chain;
use crate::fetch;
use crate::review;
use crate::runtime::Runtime;

/// 北京时间的时段切换点（整点）。
const CHANGE_HOURS: [u32; 5] = [2, 11, 14, 18, 21];
/// 内存去重记住多少条标题。RSS 一轮几十条，这个容量足够挡住短期重复。
const SEEN_CAPACITY: usize = 2000;

/// 北京时间（UTC+8）的当前小时 → 这一时段的轮询间隔秒数。
///
/// 夜里（21 点~次日 2 点）是新番集中放出的时间，一分钟一轮；白天十五分钟一轮。
///
/// 上界不能省：TS 里是 `hour >= 18 && hour < 21` 这样的闭开区间，少写上界
/// 会把 14~17 点也划进 5 分钟档（那个时段其实该是 15 分钟）。
pub fn interval_secs_for(beijing_hour: u32) -> u64 {
    match beijing_hour {
        hour if hour >= 21 || hour < 2 => 60,
        hour if (18..21).contains(&hour) => 3 * 60,
        hour if (11..14).contains(&hour) => 5 * 60,
        _ => 15 * 60,
    }
}

/// 距离下一个时段切换点还有多少秒。
fn secs_to_next_change(beijing: DateTime<Utc>) -> u64 {
    let today = beijing.date_naive();
    let target = match CHANGE_HOURS.iter().find(|&&hour| hour > beijing.hour()) {
        // 今天还有切换点。
        Some(&hour) => today.and_hms_opt(hour, 0, 0),
        // 今天没了，取明天 2 点。
        None => today.succ_opt().and_then(|day| day.and_hms_opt(2, 0, 0)),
    };
    let Some(target) = target else {
        return u64::MAX;
    };
    (target.and_utc() - beijing).num_seconds().max(0) as u64
}

/// 这次该睡多久。`config_secs` 非 0 时它是固定间隔，为 0 时走自动分时段。
///
/// 除了按时段给间隔，还要处理「切换点比间隔先到」：比如 20:59 时距离 21 点只剩
/// 1 分钟，而当前时段间隔是 3 分钟——那就睡到 21 点，切进高频时段后再立刻抓。
/// TS 里额外加 1 秒，是为了跨过边界，避免算出 0 导致空转。
pub fn wait_secs(config_secs: u64, now: DateTime<Utc>) -> u64 {
    if config_secs > 0 {
        return config_secs;
    }
    let beijing = now + Duration::from_secs(8 * 3600);
    let interval = interval_secs_for(beijing.hour());
    let to_change = secs_to_next_change(beijing);
    if to_change < interval {
        to_change + 1
    } else {
        interval
    }
}

/// 给 `/status` 用的一句话：现在这个点，多久抓一轮。
pub fn interval_label(config_secs: u64, now: DateTime<Utc>) -> String {
    if config_secs > 0 {
        return format!("{}（配置固定）", humanize(config_secs));
    }
    let beijing = now + Duration::from_secs(8 * 3600);
    format!(
        "{}，按北京时间自动分时段",
        humanize(interval_secs_for(beijing.hour()))
    )
}

/// 秒数 → 人话。给状态展示用，「900 秒」不如「15 分钟」好读。
fn humanize(seconds: u64) -> String {
    if seconds % 60 != 0 {
        return format!("每 {seconds} 秒");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("每 {minutes} 分钟");
    }
    format!("每 {} 小时", minutes / 60)
}

/// 跨轮次去重。
///
/// 满了就整个清空：RSS 一轮几十条，几千条够撑几十轮；清空的代价是最早的一批标题
/// 会被再处理一次，而那些漏过去的会被 `db` 的持久去重拦住。为了省这点重复，
/// 不值得在内存里维护一个真环形缓冲——那是 `db` 该操心的事。
#[derive(Debug, Default)]
pub struct Seen {
    titles: HashSet<String>,
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
}

/// 轮询累计计数。原子：被后台线程加，被命令回调读。
#[derive(Debug, Default)]
pub struct Stats {
    pub rounds: AtomicU64,
    pub items: AtomicU64,
    pub usable: AtomicU64,
    pub failures: AtomicU64,
}

impl Stats {
    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.rounds.load(Ordering::Relaxed),
            self.items.load(Ordering::Relaxed),
            self.usable.load(Ordering::Relaxed),
            self.failures.load(Ordering::Relaxed),
        )
    }
}

/// 后台线程的句柄。
///
/// 句柄归 `AnimePlugin`：销毁它就停轮询。计数（`Stats`）在 `Runtime` 里，
/// 因为命令回调要读，而命令拿得到的是 `Runtime`。
pub struct Poller {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Poller {
    /// 起一条后台线程。`stats` 由调用方持有，两边共享同一份计数。
    pub fn spawn(runtime: Arc<Runtime>, stats: Arc<Stats>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = (Arc::clone(&stop), Arc::clone(&stats));
        let worker = Arc::clone(&runtime);

        // 线程起不来不该让插件起不来：没有轮询是「功能残缺」，起不来是「整个插件没了」。
        let handle = match thread::Builder::new()
            .name("xq-anime-rss".to_owned())
            .spawn(move || {
                let (stop, stats) = shared;
                loop_forever(&worker, &stop, &stats);
            }) {
            Ok(handle) => Some(handle),
            Err(error) => {
                log!(&runtime.logger, Error, "RSS 轮询线程起不来：{error}");
                None
            }
        };

        Self { stop, handle }
    }

    /// 请求停止。只是置标志，不 `join`——线程可能正卡在网络里，
    /// 插件 `stop` 不该被它拖住。
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(handle) = self.handle.take() {
            // 只唤醒，不等待：线程睡在 `sleep` 上会自己醒、看到标志就退。
            handle.thread().unpark();
        }
    }
}

/// 轮询主循环。抽出来是为了能在测试里跑确定的轮数。
fn loop_forever(runtime: &Arc<Runtime>, stop: &AtomicBool, stats: &Stats) {
    let mut seen = Seen::default();
    let interval_secs = runtime.config.rss.interval_secs;
    while !stop.load(Ordering::Acquire) {
        // 一轮失败不该让线程死掉：网络抖一下就退出，等于整个插件的 RSS 断了。
        let outcome = run_round(runtime, &mut seen);
        stats.rounds.fetch_add(1, Ordering::Relaxed);
        stats
            .items
            .fetch_add(outcome.total as u64, Ordering::Relaxed);
        stats
            .usable
            .fetch_add(outcome.usable as u64, Ordering::Relaxed);
        stats
            .failures
            .fetch_add(outcome.failures as u64, Ordering::Relaxed);

        sleep_until_next(interval_secs, stop);
    }
}

/// 一轮的结果。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Round {
    pub total: usize,
    pub usable: usize,
    /// 本轮里跨轮次去重后剩下的新条目数。
    pub fresh: usize,
    pub failures: usize,
    /// 新条目里命中库内番剧的条数。
    pub matched: usize,
    /// 新条目里没命中任何番剧（按新番候选处理）的条数。
    pub new_anime: usize,
    /// 命中番剧且集数也对齐（= 可以直接投放）的条数。
    pub ready: usize,
}

/// 抓一轮：抓取 → 跨轮次去重 → 查库去重 → 匹配 → 记日志。
///
/// 这一层**不做业务决定**：命中之后要下载还是发审核，是 `anime` 主链的事。
/// 这里只把「这条种子属于谁」判出来，让日志能回答「为什么没发」。
fn run_round(runtime: &Arc<Runtime>, seen: &mut Seen) -> Round {
    let logger = &runtime.logger;
    let summary = fetch::run(&runtime.config.rss, logger);
    let failures = summary
        .sources
        .iter()
        .filter(|line| line.error.is_some())
        .count();

    // 所有合并标题都占一次机会（含解析失败的）：解析失败的 bangumi 条目每轮都会
    // 重打一遍 API，不去重的话光是白跑流量。
    let mut fresh_titles = HashSet::new();
    for title in &summary.titles {
        if seen.accept(title) {
            fresh_titles.insert(title.clone());
        }
    }
    let fresh = fresh_titles.len();

    // 库里的候选番剧一轮只取一次：匹配是纯函数，逐条查库纯属浪费。
    let candidates = load_candidates(runtime);
    let store_ready = runtime.store.available();

    let mut matched = 0usize;
    let mut new_anime = 0usize;
    let mut ready = 0usize;
    for item in &summary.items {
        if !fresh_titles.contains(&title::clean_title(&item.title)) {
            continue;
        }
        if store_ready {
            match runtime.store.has_torrent_title(&item.title) {
                Ok(true) => {
                    log!(logger, Debug, "已入过库，跳过：{}", item.title);
                    continue;
                }
                Ok(false) => {}
                Err(error) => log!(logger, Warn, "查种子去重失败：{error}"),
            }
        }

        // 行为对齐 TS（包含即命中）；要更保守就换 `MatchPolicy::default()`。
        let decision = decide_season_with(&item.names, &candidates, MatchPolicy::ts());
        match &decision {
            SeasonDecision::Matched(hit) => {
                matched += 1;
                // 番剧确定后再查这一部的章节：没确定是哪部番时查章节没有意义。
                // match_item_with 内部会再做一次 season 判定（纯函数、很便宜），
                // 好处是集数/放送窗口的逻辑只有一份实现。
                let episodes = if store_ready {
                    match runtime.store.episodes_by_subject(hit.anime_id) {
                        Ok(episodes) => episodes,
                        Err(error) => {
                            log!(logger, Warn, "读取章节失败（anime {}）：{error}", hit.anime_id);
                            Vec::new()
                        }
                    }
                } else {
                    Vec::new()
                };
                let outcome = match_item_with(
                    item,
                    &candidates,
                    &episodes,
                    today_local(),
                    None,
                    MatchPolicy::ts(),
                );
                if outcome.is_ready() {
                    ready += 1;
                    log!(logger, Info, "可投放：{}｜{}", hit.matched_name, outcome.describe());
                    // 下载可能要几十分钟：交给 chain 的限流线程，轮询继续跑。
                    chain::spawn(Arc::clone(runtime), item.clone(), hit.anime_id);
                } else {
                    log!(
                        logger,
                        Info,
                        "命中但集数没对齐：{}｜{}",
                        item.title,
                        outcome.describe()
                    );
                }
            }
            SeasonDecision::NeedsReview { best, .. } => {
                log!(
                    logger,
                    Info,
                    "待复核『{}』（{} 分）：{}",
                    best.matched_name,
                    best.score,
                    item.title
                );
                // 提示发给管理群，点按钮后走 callback.rs 那条路。
                review::prompt(runtime, item, &decision);
            }
            SeasonDecision::New => {
                new_anime += 1;
                log!(logger, Debug, "新番候选：{}", item.title);
            }
        }
    }

    log!(
        logger,
        Info,
        "{}｜新 {} 条（命中 {}，可投 {}，新番候选 {}）",
        summary.render().replace('\n', " "),
        fresh,
        matched,
        ready,
        new_anime
    );

    Round {
        total: summary.total,
        usable: summary.usable,
        fresh,
        failures,
        matched,
        new_anime,
        ready,
    }
}

/// 取库内候选番剧；存储不可用或查询失败时降级成空表（= 全部按新番处理）。
///
/// 空表不会让流程停下，但会把已有番剧误判成新番——所以失败必须打 warn，别静默。
fn load_candidates(runtime: &Runtime) -> Vec<AnimeCandidate> {
    if !runtime.store.available() {
        return Vec::new();
    }
    match runtime.store.anime_candidates() {
        Ok(candidates) => candidates,
        Err(error) => {
            log!(&runtime.logger, Warn, "读取番剧库失败，本轮按空库处理：{error}");
            Vec::new()
        }
    }
}

/// 睡到下一轮。切成小段睡，这样 `stop` 能及时生效，不必等满一个时段。
fn sleep_until_next(config_secs: u64, stop: &AtomicBool) {
    let total = wait_secs(config_secs, Utc::now());
    let step = Duration::from_secs(5);
    let mut left = Duration::from_secs(total);
    while left > Duration::ZERO && !stop.load(Ordering::Acquire) {
        let slice = left.min(step);
        // `park_timeout` 而不是 `sleep`：`stop` 时能立刻被 `unpark` 唤醒。
        thread::park_timeout(slice);
        left -= slice;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beijing(hour: u32) -> DateTime<Utc> {
        let naive = chrono::NaiveDate::from_ymd_opt(2026, 10, 26)
            .unwrap()
            .and_hms_opt(hour, 0, 0)
            .unwrap();
        (naive.and_utc()) - Duration::from_secs(8 * 3600)
    }

    #[test]
    fn night_is_the_busiest_window() {
        // 21 点~次日 2 点：新番集中放出，一分钟一轮。
        assert_eq!(interval_secs_for(21), 60);
        assert_eq!(interval_secs_for(23), 60);
        assert_eq!(interval_secs_for(0), 60);
        assert_eq!(interval_secs_for(1), 60);
    }

    #[test]
    fn daytime_slows_down() {
        assert_eq!(interval_secs_for(19), 180);
        assert_eq!(interval_secs_for(12), 300);
        assert_eq!(interval_secs_for(9), 900);
    }

    #[test]
    fn every_hour_has_the_interval_ts_gives_it() {
        // 逐小时对照 TS 的 getSmartDelayInfo。曾经漏了时段上界，
        // 把 14~17 点也划进了 5 分钟档——这张表就是为了钉死它。
        let expected = [
            (0, 60),
            (1, 60),
            (2, 900),
            (8, 900),
            (10, 900),
            (11, 300),
            (13, 300),
            (14, 900),
            (17, 900),
            (18, 180),
            (20, 180),
            (21, 60),
            (23, 60),
        ];
        for (hour, want) in expected {
            assert_eq!(interval_secs_for(hour), want, "北京时间 {hour} 点");
        }
    }

    #[test]
    fn interval_shortens_right_before_a_change_point() {
        // 20:59 北京时间：当前时段间隔 3 分钟，但距离 21 点只剩 1 分钟 → 睡到 21 点。
        let now = beijing(21) - Duration::from_secs(60);
        let wait = wait_secs(0, now);
        assert!(wait <= 61, "该睡到切换点，实际 {wait}");
    }

    #[test]
    fn interval_is_the_full_slot_away_from_change_points() {
        // 9 点：当前时段 15 分钟，下一个切换点 11 点还早 → 睡满 15 分钟。
        assert_eq!(wait_secs(0, beijing(9)), 900);
    }

    #[test]
    fn waiting_across_midnight_lands_on_two_am() {
        // 23 点：距离 2 点还有 3 小时，远大于当前时段的 60 秒 → 睡 60 秒。
        assert_eq!(wait_secs(0, beijing(23)), 60);
    }

    #[test]
    fn configured_interval_overrides_the_schedule() {
        // 配了固定间隔就不再看时段：调试时要想「三秒一轮」得能强制。
        assert_eq!(wait_secs(3, beijing(9)), 3);
        assert_eq!(wait_secs(3, beijing(23)), 3);
    }

    #[test]
    fn interval_label_says_what_it_does() {
        assert_eq!(interval_label(300, beijing(9)), "每 5 分钟（配置固定）");
        assert_eq!(
            interval_label(0, beijing(9)),
            "每 15 分钟，按北京时间自动分时段"
        );
        assert_eq!(
            interval_label(0, beijing(23)),
            "每 1 分钟，按北京时间自动分时段"
        );
        assert_eq!(interval_label(45, beijing(9)), "每 45 秒（配置固定）");
    }

    #[test]
    fn seen_rejects_repeats_and_accepts_new() {
        let mut seen = Seen::default();
        assert!(seen.accept("A"));
        assert!(!seen.accept("A"), "同一条标题第二轮要被挡住");
        assert!(seen.accept("B"));
    }

    #[test]
    fn seen_forgets_the_oldest_when_full() {
        let mut seen = Seen::default();
        for index in 0..SEEN_CAPACITY + 10 {
            seen.accept(&format!("标题 {index}"));
        }
        // 满了会整个清空，最早的标题于是又被当成新的。这是有意的取舍：
        // 漏过去的由 `db` 的持久去重兜住，内存里不值得维护一个环形缓冲。
        assert!(seen.accept("标题 0"));
    }

    #[test]
    fn stats_counts_rounds() {
        let stats = Stats::default();
        stats.rounds.fetch_add(2, Ordering::Relaxed);
        stats.items.fetch_add(25, Ordering::Relaxed);
        let (rounds, items, usable, failures) = stats.snapshot();
        assert_eq!(rounds, 2);
        assert_eq!(items, 25);
        assert_eq!(usable, 0);
        assert_eq!(failures, 0);
    }
}
