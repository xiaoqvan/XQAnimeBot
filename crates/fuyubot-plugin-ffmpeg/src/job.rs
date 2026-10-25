//! 任务管理：一个转码 = 一个线程 + 一个子进程。
//!
//! 进度是**推**出来的，不是轮询出来的：ffmpeg 往 stderr 打 `time=00:12:34.56`，
//! `ffmpeg-sidecar` 把它解析成 `FfmpegProgress`，我们换算成百分比后通过 [`Sink`] 交出去。
//! 生产环境里 [`HostSink`] 把进度发到宿主事件总线，同时更新注册表供 `job_status` 轮询；
//! 测试/命令行里换成打印版 sink，同一套逻辑不需要宿主也能跑。
//!
//! 两条通道（事件 + 轮询）都留着是有原因的：事件适合「业务边收边更新消息」，
//! 轮询适合「用户手动点了下按钮」。只做其中一种总有人不够用。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::{FfmpegEvent, LogLevel as FfmpegLogLevel};
use fuyubot_abi::HostApi;
use serde_json::{Value, json};

use crate::capability::parse_timecode;
use crate::plan::Plan;

/// 进度回调的最小间隔：ffmpeg 默认每 0.5s 一条，再密就是刷屏。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(400);

/// 进度出口。生产环境用 [`HostSink`]，测试/命令行可以用打印版。
pub trait Sink: Send + Sync {
    fn progress(&self, status: &JobStatus);
    fn finished(&self, event: &'static str, status: &JobStatus);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Done,
    Failed,
    Canceled,
}

impl JobState {
    fn as_str(self) -> &'static str {
        match self {
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Canceled => "canceled",
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, JobState::Running)
    }
}

#[derive(Debug, Clone)]
pub struct JobStatus {
    pub id: String,
    pub input: String,
    pub output: String,
    pub state: JobState,
    pub percent: f64,
    pub speed: f32,
    pub fps: f32,
    pub frame: u32,
    pub out_time_ms: i64,
    pub duration_ms: i64,
    pub elapsed_ms: u64,
    pub mode: &'static str,
    pub encoder: String,
    pub error: Option<String>,
}

impl JobStatus {
    pub fn to_json(&self) -> Value {
        let eta_ms = if self.speed > 0.05 && self.duration_ms > 0 {
            let remaining = (self.duration_ms - self.out_time_ms).max(0) as f32;
            Some((remaining / self.speed) as i64)
        } else {
            None
        };
        json!({
            "id": self.id,
            "input": self.input,
            "output": self.output,
            "state": self.state.as_str(),
            // percent 内部是 0~1，对外给 0~100 且保留一位小数。
            "percent": (self.percent * 1000.0).round() / 10.0,
            "speed": self.speed,
            "fps": self.fps,
            "frame": self.frame,
            "out_time_ms": self.out_time_ms,
            "duration_ms": self.duration_ms,
            "elapsed_ms": self.elapsed_ms,
            "eta_ms": eta_ms,
            "mode": self.mode,
            "encoder": self.encoder,
            "error": self.error,
        })
    }
}

/// 生产用出口：更新注册表 + 发宿主事件。
pub struct HostSink {
    pub host: HostApi,
    pub status: Arc<Mutex<JobStatus>>,
}

impl Sink for HostSink {
    fn progress(&self, status: &JobStatus) {
        {
            let mut guard = self.status.lock().unwrap_or_else(|p| p.into_inner());
            *guard = status.clone();
        }
        let _ = self
            .host
            .emit("ffmpeg.progress", &status.to_json().to_string());
    }

    fn finished(&self, event: &'static str, status: &JobStatus) {
        {
            let mut guard = self.status.lock().unwrap_or_else(|p| p.into_inner());
            *guard = status.clone();
        }
        let _ = self.host.emit(event, &status.to_json().to_string());
    }
}

pub struct JobHandle {
    pub status: Arc<Mutex<JobStatus>>,
    pub cancel: Arc<AtomicBool>,
}

static JOBS: OnceLock<Mutex<HashMap<String, JobHandle>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn jobs() -> &'static Mutex<HashMap<String, JobHandle>> {
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 起一个转码任务，立刻返回任务 id。
pub fn start(
    host: HostApi,
    ffmpeg: std::path::PathBuf,
    plan: Plan,
    input: String,
    output: String,
    duration_ms: i64,
) -> String {
    let id = new_id();
    let status = Arc::new(Mutex::new(JobStatus {
        id: id.clone(),
        input: input.clone(),
        output: output.clone(),
        state: JobState::Running,
        percent: 0.0,
        speed: 0.0,
        fps: 0.0,
        frame: 0,
        out_time_ms: 0,
        duration_ms,
        elapsed_ms: 0,
        mode: plan.mode,
        encoder: plan.encoder.clone(),
        error: None,
    }));
    let cancel = Arc::new(AtomicBool::new(false));

    {
        let mut guard = jobs().lock().unwrap_or_else(|p| p.into_inner());
        guard.insert(
            id.clone(),
            JobHandle {
                status: Arc::clone(&status),
                cancel: Arc::clone(&cancel),
            },
        );
    }

    let sink = HostSink {
        host,
        status: Arc::clone(&status),
    };
    let plan_for_thread = plan.clone();
    let thread_id = id.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("ffmpeg-{thread_id}"))
        .spawn(move || {
            let _ = execute(
                &sink,
                &ffmpeg,
                &plan_for_thread,
                &input,
                &output,
                duration_ms,
                &cancel,
            );
        });
    if let Err(_err) = spawned {
        let mut guard = jobs().lock().unwrap_or_else(|p| p.into_inner());
        guard.remove(&id);
        return format!("ERR:{id}");
    }
    id
}

/// 同步跑一次转码（命令行 / 测试用）。返回最终状态。
pub fn run_blocking(
    sink: &dyn Sink,
    ffmpeg: &Path,
    plan: &Plan,
    input: String,
    output: String,
    duration_ms: i64,
) -> JobStatus {
    let cancel = AtomicBool::new(false);
    execute(sink, ffmpeg, plan, &input, &output, duration_ms, &cancel)
}

fn new_id() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("ff-{stamp:x}-{seq}")
}

/// 真正的执行体：起进程 → 读事件 → 报进度 → 收尾。
fn execute(
    sink: &dyn Sink,
    ffmpeg: &Path,
    plan: &Plan,
    input: &str,
    output: &str,
    duration_ms: i64,
    cancel: &AtomicBool,
) -> JobStatus {
    let started = Instant::now();
    let mut status = JobStatus {
        id: String::new(),
        input: input.to_owned(),
        output: output.to_owned(),
        state: JobState::Running,
        percent: 0.0,
        speed: 0.0,
        fps: 0.0,
        frame: 0,
        out_time_ms: 0,
        duration_ms,
        elapsed_ms: 0,
        mode: plan.mode,
        encoder: plan.encoder.clone(),
        error: None,
    };

    let mut cmd = FfmpegCommand::new_with_path(ffmpeg);
    cmd.args(plan.args.iter().map(String::as_str));
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            status.state = JobState::Failed;
            status.error = Some(format!("启动 ffmpeg 失败：{err}"));
            status.elapsed_ms = started.elapsed().as_millis() as u64;
            sink.finished("ffmpeg.failed", &status);
            return status;
        }
    };

    let mut last_emit = Instant::now() - PROGRESS_INTERVAL;
    let mut last_error: Option<String> = None;
    let mut canceled = false;

    if let Ok(events) = child.iter() {
        for event in events {
            if cancel.load(Ordering::Relaxed) {
                canceled = true;
                break;
            }
            match event {
                FfmpegEvent::Progress(progress) => {
                    status.out_time_ms = parse_timecode(&progress.time);
                    status.speed = progress.speed;
                    status.fps = progress.fps;
                    status.frame = progress.frame;
                    status.elapsed_ms = started.elapsed().as_millis() as u64;
                    status.percent = if duration_ms > 0 {
                        (status.out_time_ms as f64 / duration_ms as f64).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    if last_emit.elapsed() >= PROGRESS_INTERVAL {
                        last_emit = Instant::now();
                        sink.progress(&status);
                    }
                }
                FfmpegEvent::Log(level, message) => {
                    if matches!(level, FfmpegLogLevel::Error | FfmpegLogLevel::Fatal) {
                        last_error = Some(message);
                    }
                }
                FfmpegEvent::Error(message) => {
                    // 这里的 Error 多半是**解析器**报的（例如没解析到输出流），
                    // 不代表 ffmpeg 失败——真正的结论看退出码。
                    last_error = Some(message);
                }
                FfmpegEvent::Done => break,
                _ => {}
            }
        }
    }

    if canceled {
        let _ = child.kill();
        let _ = child.wait();
        status.state = JobState::Canceled;
        status.elapsed_ms = started.elapsed().as_millis() as u64;
        sink.finished("ffmpeg.canceled", &status);
        return status;
    }

    match child.wait() {
        Ok(exit) if exit.success() => {
            // 进度条最后一定要落到 100%，否则界面上会停在 99.8%。
            status.state = JobState::Done;
            status.percent = 1.0;
            status.out_time_ms = status.out_time_ms.max(duration_ms);
            status.elapsed_ms = started.elapsed().as_millis() as u64;
            sink.finished("ffmpeg.done", &status);
        }
        Ok(exit) => {
            status.state = JobState::Failed;
            status.error = Some(
                last_error
                    .unwrap_or_else(|| format!("ffmpeg 退出码 {}", exit.code().unwrap_or(-1))),
            );
            status.elapsed_ms = started.elapsed().as_millis() as u64;
            sink.finished("ffmpeg.failed", &status);
        }
        Err(err) => {
            status.state = JobState::Failed;
            status.error = Some(format!("等待 ffmpeg 结束失败：{err}"));
            status.elapsed_ms = started.elapsed().as_millis() as u64;
            sink.finished("ffmpeg.failed", &status);
        }
    }
    status
}

/// 全部任务快照，最近的排前面。
pub fn list() -> Vec<Value> {
    let guard = jobs().lock().unwrap_or_else(|p| p.into_inner());
    let mut items: Vec<Value> = guard
        .values()
        .map(|handle| {
            handle
                .status
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .to_json()
        })
        .collect();
    items.sort_by(|a, b| {
        b["id"]
            .as_str()
            .unwrap_or_default()
            .cmp(a["id"].as_str().unwrap_or_default())
    });
    items
}

pub fn get(id: &str) -> Option<Value> {
    let guard = jobs().lock().unwrap_or_else(|p| p.into_inner());
    guard.get(id).map(|handle| {
        handle
            .status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .to_json()
    })
}

/// 请求取消。真正退出要等下一批进度事件到达（通常 <1s）。
pub fn cancel(id: &str) -> bool {
    let guard = jobs().lock().unwrap_or_else(|p| p.into_inner());
    match guard.get(id) {
        Some(handle) => {
            handle.cancel.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// 清掉已结束的任务记录，避免注册表无限长。
pub fn prune(keep: usize) -> usize {
    let mut guard = jobs().lock().unwrap_or_else(|p| p.into_inner());
    let finished: Vec<String> = guard
        .iter()
        .filter(|(_, handle)| {
            handle
                .status
                .lock()
                .map(|status| status.state.is_terminal())
                .unwrap_or(false)
        })
        .map(|(id, _)| id.clone())
        .collect();
    let remove_count = finished.len().saturating_sub(keep);
    for id in finished.into_iter().take(remove_count) {
        guard.remove(&id);
    }
    remove_count
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopSink;
    impl Sink for NoopSink {
        fn progress(&self, _status: &JobStatus) {}
        fn finished(&self, _event: &'static str, _status: &JobStatus) {}
    }

    #[test]
    fn ids_are_unique() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert!(a.starts_with("ff-"));
    }

    #[test]
    fn terminal_states_are_distinguished() {
        assert!(!JobState::Running.is_terminal());
        assert!(JobState::Done.is_terminal());
        assert!(JobState::Failed.is_terminal());
        assert!(JobState::Canceled.is_terminal());
    }

    #[test]
    fn json_includes_eta_and_rounded_percent() {
        let status = JobStatus {
            id: "ff-1".to_owned(),
            input: "a.mkv".to_owned(),
            output: "b.mp4".to_owned(),
            state: JobState::Running,
            percent: 0.51234,
            speed: 2.0,
            fps: 23.9,
            frame: 100,
            out_time_ms: 30_000,
            duration_ms: 60_000,
            elapsed_ms: 15_000,
            mode: "encode",
            encoder: "libx264".to_owned(),
            error: None,
        };
        let value = status.to_json();
        assert_eq!(value["percent"], 51.2);
        // 剩余 30s，2× 速度 → 15s
        assert_eq!(value["eta_ms"], 15_000);
        assert_eq!(value["state"], "running");
    }

    #[test]
    fn missing_binary_fails_without_panicking() {
        let plan = Plan {
            mode: "copy",
            encoder: String::new(),
            hardware: false,
            args: vec!["-version".to_owned()],
            reason: "test".to_owned(),
        };
        let status = run_blocking(
            &NoopSink,
            Path::new("definitely-not-an-ffmpeg-binary"),
            &plan,
            "in.mp4".to_owned(),
            "out.mp4".to_owned(),
            1000,
        );
        assert_eq!(status.state, JobState::Failed);
        assert!(status.error.is_some());
    }
}
