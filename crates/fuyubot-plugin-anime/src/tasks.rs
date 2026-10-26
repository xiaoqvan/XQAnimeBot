//! 并发管理器：任务表 + 分队列槽位 + 取消。
//!
//! 对应 TS 的 `AnimeProcessorManager`：普通 worker / MKV 独立队列 / 重型转换独占槽，
//! 外加一张 `progressMap`（每条任务的阶段、百分比、标签）。这里用 std 的
//! `Condvar` + 原子变量实现——阻塞的只有「等槽位」和「等下载完成」两处，
//! 引第三方并发库不划算。
//!
//! 三条不变量（都对应 TS 的真实行为）：
//!
//! ```text
//! 1. 普通队列容量 = rss.workers，MKV 队列容量 = rss.mkv_workers
//! 2. 转码（重型转换）额外抢 1 个全局独占槽：多路同时转会把机器拖死
//! 3. 取消是「置标志 + 循环里检查」：线程还在等 qB，不能直接杀
//! ```
//!
//! 为什么 MKV 要单开一条队列：烧字幕转码慢且吃 CPU，混在普通队列里会把
//! 「已经能直接发」的 mp4 一起堵住。TS 也是这么分的。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// 队列种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 普通队列：mp4 之类不需要烧字幕的。
    Normal,
    /// MKV 队列：需要抽/烧字幕再转码的。
    Mkv,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "普通",
            Self::Mkv => "MKV",
        }
    }
}

/// 任务阶段。顺序即流程顺序，`Done` / `Failed` / `Cancelled` 是终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// 已入队，还没拿到槽位。
    Queued,
    /// 下载前的容器预检（等 qB 拿到 metadata）。
    Checking,
    Downloading,
    Transcoding,
    Uploading,
    Done,
    Failed,
    Cancelled,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "排队中",
            Self::Checking => "预检",
            Self::Downloading => "下载中",
            Self::Transcoding => "转码中",
            Self::Uploading => "上传中",
            Self::Done => "完成",
            Self::Failed => "失败",
            Self::Cancelled => "已取消",
        }
    }
}

/// 一条任务对外可见的快照。[`TaskView::line`] 直接渲染成 `/progress` 的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskView {
    pub id: u64,
    pub title: String,
    pub anime_id: i64,
    pub kind: Kind,
    pub stage: Stage,
    pub percent: u8,
    /// 附注：`1.2/4.5 GB`、`第 2/3 段` 之类。
    pub detail: String,
    pub elapsed: Duration,
}

impl TaskView {
    /// `/progress` 的一行：编号、阶段、百分比、标题、耗时。
    pub fn line(&self) -> String {
        let detail = if self.detail.trim().is_empty() {
            String::new()
        } else {
            format!("｜{}", self.detail.trim())
        };
        format!(
            "#{} [{} {}%] {}{}（{}）",
            self.id,
            self.stage.label(),
            self.percent,
            shorten(&self.title, 42),
            detail,
            human_elapsed(self.elapsed)
        )
    }
}

/// 标题太长会把一行撑爆，截断时留个省略号。
fn shorten(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…")
}

/// `95s` → `1 分 35 秒`；超过一小时按 `1 小时 5 分`。
fn human_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        return format!("{secs} 秒");
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{minutes} 分 {} 秒", secs % 60);
    }
    format!("{} 小时 {} 分", minutes / 60, minutes % 60)
}

/// 一条任务。状态全部内部可变：`/progress` 和 `run()` 在不同线程里读写。
#[derive(Debug)]
pub struct Task {
    /// `/canceltask` 用的编号，进程内唯一且不复用。
    pub id: u64,
    /// 去重键（btih 或清洗后的标题）。
    pub key: String,
    pub title: String,
    pub anime_id: i64,
    kind: Mutex<Kind>,
    state: Mutex<State>,
    cancel: AtomicBool,
    started: Instant,
}

#[derive(Debug, Default)]
struct State {
    stage: Stage,
    percent: u8,
    detail: String,
}

impl Default for Stage {
    fn default() -> Self {
        Self::Queued
    }
}

impl Task {
    pub fn kind(&self) -> Kind {
        *self.kind.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// 预检出「纯 MKV」后把任务挪进 MKV 队列。
    pub fn set_kind(&self, kind: Kind) {
        *self.kind.lock().unwrap_or_else(|error| error.into_inner()) = kind;
    }

    /// 换阶段，百分比归零（阶段内进度不该带到下一阶段）。
    pub fn set_stage(&self, stage: Stage) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.stage == stage {
            return;
        }
        state.stage = stage;
        state.percent = 0;
        // 附注也是阶段内的：留着上一阶段的「1.2/4.5 GB」会让人以为还在下载。
        state.detail.clear();
    }

    /// 更新阶段内进度。百分比只增不减——qB 的 progress 偶尔会回退（重新校验），
    /// `/progress` 上跳来跳去像是卡住了。回退的那一轮连附注一起忽略，免得出现
    /// 「42%｜重新校验」这种自相矛盾的行。
    pub fn progress(&self, percent: u8, detail: impl Into<String>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let next = percent.min(100);
        if next < state.percent {
            return;
        }
        state.percent = next;
        let detail = detail.into();
        if !detail.trim().is_empty() {
            state.detail = detail;
        }
    }

    /// 请求取消。返回 `true` 表示这次才置位（重复点按钮不该重复记日志）。
    pub fn cancel(&self) -> bool {
        !self.cancel.swap(true, Ordering::SeqCst)
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn view(&self) -> TaskView {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        TaskView {
            id: self.id,
            title: self.title.clone(),
            anime_id: self.anime_id,
            kind: self.kind(),
            stage: state.stage,
            percent: state.percent,
            detail: state.detail.clone(),
            elapsed: self.elapsed(),
        }
    }
}

/// 任务表：`Vec` 而不是 `HashMap`，因为 `/progress` 要按入队顺序列。
static TASKS: LazyLock<Mutex<Vec<Arc<Task>>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// 入队；同 key 已有活跃任务时返回 `None`（去重）。
///
/// 检查与插入在同一把锁里：轮询线程可能在同一秒里投递同一条种子两次，
/// 先 `has_active` 再 `enqueue` 的两步写法正好能漏过去。
pub fn enqueue(key: &str, title: &str, anime_id: i64) -> Option<Arc<Task>> {
    let mut tasks = TASKS.lock().unwrap_or_else(|error| error.into_inner());
    if tasks.iter().any(|task| task.key == key) {
        return None;
    }
    let task = Arc::new(Task {
        id: NEXT_ID.fetch_add(1, Ordering::SeqCst),
        key: key.to_owned(),
        title: title.to_owned(),
        anime_id,
        kind: Mutex::new(Kind::Normal),
        state: Mutex::new(State::default()),
        cancel: AtomicBool::new(false),
        started: Instant::now(),
    });
    tasks.push(Arc::clone(&task));
    Some(task)
}

/// 有没有同 key 的活跃任务（去重）。
///
/// 生产路径不再直接用它——[`enqueue`] 把「查重 + 插入」放进同一把锁，
/// 这里留给单测断言任务表的真实内容。
#[cfg(test)]
pub fn has_active(key: &str) -> bool {
    TASKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .any(|task| task.key == key)
}

/// 收尾：记下终态，然后从任务表里摘掉。
///
/// 摘掉是必须的：`/progress` 只看活跃任务，留着会越积越多，也让「同 key 去重」
/// 永远挡着下一条。终态只写日志，不留在表里。
pub fn finish(task: &Arc<Task>) {
    TASKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|item| item.id != task.id);
}

/// 当前活跃任务（按入队顺序）。
pub fn snapshot() -> Vec<TaskView> {
    TASKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .map(|task| task.view())
        .collect()
}

/// 活跃任务数。
pub fn active_count() -> usize {
    TASKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .len()
}

/// 按编号取消。返回被取消的任务（调用方据此报告标题）。
pub fn cancel(id: u64) -> Option<Arc<Task>> {
    let task = TASKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .find(|task| task.id == id)
        .cloned()?;
    task.cancel();
    Some(task)
}

/// 按标题（子串，忽略大小写）取消，返回取消了几条。
pub fn cancel_matching(title: &str) -> usize {
    let needle = title.trim().to_lowercase();
    if needle.is_empty() {
        return 0;
    }
    let tasks: Vec<Arc<Task>> = TASKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .filter(|task| task.title.to_lowercase().contains(&needle))
        .cloned()
        .collect();
    tasks.iter().filter(|task| task.cancel()).count()
}

/// 全部取消，返回取消了几条。
pub fn cancel_all() -> usize {
    let tasks: Vec<Arc<Task>> = TASKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    tasks.iter().filter(|task| task.cancel()).count()
}

// ─── 槽位 ──────────────────────────────────────────────────────────────────

/// 等槽位时的轮询间隔：被取消的任务不能一直卡在 `wait` 里。
const WAIT_TICK: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
struct Slots {
    normal: usize,
    mkv: usize,
    heavy: usize,
}

static SLOTS: LazyLock<(Mutex<Slots>, Condvar)> =
    LazyLock::new(|| (Mutex::new(Slots::default()), Condvar::new()));

/// 占一个槽位；被取消时返回 `None`。
///
/// 返回的 guard 在 drop 时归还槽位——`run()` 里错误分支很多，手动 release
/// 漏一条就是永久少一个 worker。
pub fn acquire(task: &Task, normal_cap: usize, mkv_cap: usize) -> Option<Slot> {
    let (lock, cvar) = &*SLOTS;
    let mut slots = lock.lock().unwrap_or_else(|error| error.into_inner());
    loop {
        if task.cancelled() {
            return None;
        }
        let kind = task.kind();
        let (used, cap) = match kind {
            Kind::Normal => (slots.normal, normal_cap.max(1)),
            Kind::Mkv => (slots.mkv, mkv_cap.max(1)),
        };
        if used < cap {
            match kind {
                Kind::Normal => slots.normal += 1,
                Kind::Mkv => slots.mkv += 1,
            }
            return Some(Slot { kind });
        }
        let (guard, _) = cvar
            .wait_timeout(slots, WAIT_TICK)
            .unwrap_or_else(|error| error.into_inner());
        slots = guard;
    }
}

/// 槽位 guard：drop 即归还。
#[derive(Debug)]
pub struct Slot {
    kind: Kind,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let (lock, cvar) = &*SLOTS;
        let mut slots = lock.lock().unwrap_or_else(|error| error.into_inner());
        match self.kind {
            Kind::Normal => slots.normal = slots.normal.saturating_sub(1),
            Kind::Mkv => slots.mkv = slots.mkv.saturating_sub(1),
        }
        cvar.notify_all();
    }
}

/// 重型转换（转码）的全局独占槽：同一时刻只允许一条在转。
///
/// 不看 `workers`：转码是 CPU 密集的，多开只会互相拖慢，TS 也是硬编码 1。
pub fn heavy() -> HeavySlot {
    let (lock, cvar) = &*SLOTS;
    let mut slots = lock.lock().unwrap_or_else(|error| error.into_inner());
    while slots.heavy >= 1 {
        let (guard, _) = cvar
            .wait_timeout(slots, WAIT_TICK)
            .unwrap_or_else(|error| error.into_inner());
        slots = guard;
    }
    slots.heavy += 1;
    HeavySlot
}

#[derive(Debug)]
pub struct HeavySlot;

impl Drop for HeavySlot {
    fn drop(&mut self) {
        let (lock, cvar) = &*SLOTS;
        let mut slots = lock.lock().unwrap_or_else(|error| error.into_inner());
        slots.heavy = slots.heavy.saturating_sub(1);
        cvar.notify_all();
    }
}

/// 当前占用：`(普通, MKV, 重型)`。
pub fn usage() -> (usize, usize, usize) {
    let slots = SLOTS
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    (slots.normal, slots.mkv, slots.heavy)
}

/// 只给测试用：把内部计数清零（测试并行跑时互相污染）。
#[cfg(test)]
fn reset_for_tests() {
    {
        let mut slots = SLOTS
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *slots = Slots::default();
    }
    TASKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试串行：槽位和任务表都是进程级单例。
    static LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());
        reset_for_tests();
        guard
    }

    #[test]
    fn enqueue_assigns_increasing_ids() {
        let _lock = guard();
        let first = enqueue("a", "标题 A", 1).expect("入队");
        let second = enqueue("b", "标题 B", 2).expect("入队");
        assert!(second.id > first.id);
        assert_eq!(active_count(), 2);
        assert!(has_active("a"));
        assert!(!has_active("c"));
    }

    #[test]
    fn percent_never_goes_backwards() {
        let _lock = guard();
        let task = enqueue("a", "标题", 1).expect("入队");
        task.set_stage(Stage::Downloading);
        task.progress(40, "1.0/2.5 GB");
        task.progress(30, "重新校验");
        let view = &snapshot()[0];
        assert_eq!(view.percent, 40);
        assert_eq!(view.detail, "1.0/2.5 GB");
    }

    #[test]
    fn switching_stage_resets_percent() {
        let _lock = guard();
        let task = enqueue("a", "标题", 1).expect("入队");
        task.set_stage(Stage::Downloading);
        task.progress(100, "");
        task.set_stage(Stage::Transcoding);
        assert_eq!(snapshot()[0].percent, 0);
    }

    #[test]
    fn cancel_marks_task_and_can_be_repeated() {
        let _lock = guard();
        let task = enqueue("a", "标题", 1).expect("入队");
        assert!(cancel(task.id).is_some());
        assert!(task.cancelled());
        assert!(!task.cancel(), "第二次置位不算新请求");
        assert_eq!(cancel_all(), 0, "已经取消过的不重复计");
    }

    #[test]
    fn same_key_is_rejected_while_task_is_active() {
        let _lock = guard();
        let first = enqueue("same", "标题", 1).expect("首次入队");
        assert!(enqueue("same", "标题", 1).is_none(), "同 key 不重复入队");
        finish(&first);
        assert!(enqueue("same", "标题", 1).is_some(), "收尾后可以重新投递");
    }

    #[test]
    fn cancel_matching_takes_title_substring() {
        let _lock = guard();
        enqueue("a", "葬送的芙莉莲 第03集", 1).expect("入队");
        enqueue("b", "别的番 第01集", 2).expect("入队");
        assert_eq!(cancel_matching("芙莉莲"), 1);
        assert_eq!(cancel_all(), 1, "只剩另一条没取消");
    }

    #[test]
    fn finish_removes_task_from_the_table() {
        let _lock = guard();
        let task = enqueue("a", "标题", 1).expect("入队");
        finish(&task);
        assert!(snapshot().is_empty());
        assert!(!has_active("a"), "摘掉之后同 key 可以再投递");
    }

    #[test]
    fn slots_respect_queue_capacity_and_return_on_drop() {
        let _lock = guard();
        let one = enqueue("a", "A", 0).expect("入队");
        let two = enqueue("b", "B", 0).expect("入队");
        two.set_kind(Kind::Mkv);

        let slot = acquire(&one, 1, 1).expect("普通槽位空闲");
        assert_eq!(usage().0, 1);
        drop(slot);
        assert_eq!(usage().0, 0, "drop 即归还");

        let normal = acquire(&one, 1, 1).expect("普通槽位");
        let mkv = acquire(&two, 1, 1).expect("MKV 是独立队列，普通占满也能进");
        assert_eq!(usage(), (1, 1, 0));
        drop(normal);
        drop(mkv);
        assert_eq!(usage(), (0, 0, 0));
    }

    #[test]
    fn cancelled_task_never_acquires_a_slot() {
        let _lock = guard();
        let task = enqueue("a", "A", 0).expect("入队");
        task.cancel();
        assert!(acquire(&task, 3, 1).is_none());
        assert_eq!(usage(), (0, 0, 0));
    }

    #[test]
    fn heavy_slot_is_exclusive() {
        let _lock = guard();
        let first = heavy();
        assert_eq!(usage().2, 1);
        drop(first);
        assert_eq!(usage().2, 0);
    }

    #[test]
    fn view_line_carries_stage_and_percent() {
        let _lock = guard();
        let task = enqueue("a", "某部番 第03集", 42).expect("入队");
        task.set_stage(Stage::Downloading);
        task.progress(66, "2.0/3.0 GB");
        let line = snapshot()[0].line();
        assert!(line.contains("下载中 66%"), "{line}");
        assert!(line.contains("某部番"), "{line}");
        assert!(line.contains("2.0/3.0 GB"), "{line}");
    }

    #[test]
    fn long_titles_are_shortened() {
        let long = "很长的标题".repeat(20);
        let text = shorten(&long, 10);
        assert!(text.ends_with('…'));
        assert_eq!(text.chars().count(), 11);
    }
}
