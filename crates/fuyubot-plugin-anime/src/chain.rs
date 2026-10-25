//! 命中之后的那一段：下载 → 等完成 → 挑文件 → 转码 → 发频道。
//!
//! 对应 TS 的 `qBittorrent/download.ts` + `anime/sendAnime.ts` 里最小的一条直线。
//! 转码只做「换个容器 / 转成兼容编码」这一层，具体参数由 `media.ffmpeg` 的
//! compat 方案决定；**故意省掉了**：字幕烧录、分段上传、导航消息、审核按钮、
//! 检查点。这些每一项都有自己的状态机，混进来会把「先跑通一条直线」这件事拖没。
//!
//! 线程模型：`poll` 发现可投放的条目就调 [spawn]，这里用固定槽位限流。
//! 下载可能要几十分钟，绝不能占用轮询线程——否则下一轮 RSS 永远抓不到。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anime_core::models::{AnimeItem, TorrentStatus};
use anime_core::store::{Storage, btih_from_magnet};
use anime_core::{Error, Result, log};
use serde_json::Value;

use crate::media::Media;
use crate::qb::Qb;
use crate::runtime::Runtime;
use crate::td::Target;

/// 同时最多几条在下载。TS 的 `AnimeProcessorManager` 也是 3 个 worker。
const WORKERS: usize = 3;
/// qB 分类，方便在 qB 界面里一眼认出哪些是这个机器人加的。
const CATEGORY: &str = "xq-anime";
/// 认得出的视频后缀（小写）。
const VIDEO_EXT: [&str; 5] = ["mkv", "mp4", "avi", "ts", "m2ts"];

static SLOTS: LazyLock<(Mutex<usize>, Condvar)> =
    LazyLock::new(|| (Mutex::new(0), Condvar::new()));
static IN_FLIGHT: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// 起一个后台任务。拿不到槽位就等——阻塞的是调用方线程（轮询线程）……
/// 所以调用方必须在自己的线程里调它，**不能**在命令回调里直接调。
pub fn spawn(runtime: Arc<Runtime>, item: AnimeItem, anime_id: i64) {
    let Some(hash) = btih_from_magnet(&item.magnet) else {
        log!(&runtime.logger, Warn, "没有 btih，跳过下载：{}", item.title);
        return;
    };
    // 同一条种子只下一次：轮询可能在同一秒里重复投递（Seen 失效、手动触发等）。
    {
        let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        if !in_flight.insert(hash.clone()) {
            return;
        }
    }
    acquire_slot();
    let worker = Arc::clone(&runtime);
    let title = item.title.clone();
    let task_hash = hash.clone();
    let spawned = thread::Builder::new()
        .name("xq-anime-chain".to_owned())
        .spawn(move || {
            let result = run(&worker, &item, anime_id, &task_hash);
            if let Err(error) = result {
                log!(&worker.logger, Error, "下载链路失败（{title}）：{error}");
            }
            release(&task_hash);
        });
    if spawned.is_err() {
        release(&hash);
    }
}

/// 当前在下载/发送的条目数。
///
/// 读的是「已占槽位」的集合，不是 qB 里的种子数：暂停在 qB 队列里的不算。
pub fn active() -> usize {
    IN_FLIGHT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .len()
}

fn acquire_slot() {
    let (lock, cvar) = &*SLOTS;
    let mut active = lock.lock().unwrap_or_else(|e| e.into_inner());
    while *active >= WORKERS {
        active = cvar.wait(active).unwrap_or_else(|e| e.into_inner());
    }
    *active += 1;
}

fn release(hash: &str) {
    {
        let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        in_flight.remove(hash);
    }
    let (lock, cvar) = &*SLOTS;
    let mut active = lock.lock().unwrap_or_else(|e| e.into_inner());
    *active = active.saturating_sub(1);
    cvar.notify_one();
}

fn run(runtime: &Runtime, item: &AnimeItem, anime_id: i64, hash: &str) -> Result<()> {
    let qb = Qb::new(runtime.host);
    if !qb.available() {
        return Err(Error::Service {
            service: "download.qbittorrent".to_owned(),
            message: "插件未安装或未启动".to_owned(),
        });
    }
    let config = &runtime.config;
    // 先记账再下载：中途崩了至少知道这条被接手过（TS 也是先写 torrents）。
    if let Err(error) = runtime
        .store
        .add_torrent(&item.magnet, TorrentStatus::Downloading, &item.title)
    {
        log!(&runtime.logger, Warn, "写入种子记录失败（继续下载）：{error}");
    }
    qb.add(&item.magnet, &config.qbittorrent.save_path, CATEGORY)?;
    log!(&runtime.logger, Info, "已加种：{}（anime {anime_id}）", item.title);

    let torrent = wait_done(runtime, &qb, hash)?;
    let base = torrent
        .get("content_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| {
            torrent
                .get("save_path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
        })
        .ok_or_else(|| Error::Service {
            service: "download.qbittorrent".to_owned(),
            message: "种子里没有 content_path / save_path".to_owned(),
        })?;

    let files = qb.files(hash)?;
    let video = pick_video(&files, &base).ok_or_else(|| Error::Service {
        service: "download.qbittorrent".to_owned(),
        message: format!("种子里没有视频文件（{} 个文件）", files.len()),
    })?;

    // 转码是**尽力而为**：任何一步失败都回退到原文件，不让「发不出去」成为结果。
    let (send_path, transcoded) = prepare_video(runtime, &video);

    let target = Target::new(config.telegram.anime_channel, 0);
    let mut sent = runtime
        .td
        .send_video(target, send_path.display().to_string(), caption(item));
    // 转码产物发不出去时再试原文件：这一步也属于「尽力而为」的一环。
    if !sent && transcoded {
        log!(
            &runtime.logger,
            Warn,
            "发送转码产物失败，回退原文件：{}",
            send_path.display()
        );
        sent = runtime
            .td
            .send_video(target, video.display().to_string(), caption(item));
    }
    if !sent {
        return Err(Error::Service {
            service: "td".to_owned(),
            message: "发视频失败".to_owned(),
        });
    }
    // 发送成功后再删源：发送失败时原始文件还要留着重试。
    if transcoded && config.media.delete_source_after_remux {
        match std::fs::remove_file(&video) {
            Ok(()) => log!(
                &runtime.logger,
                Info,
                "已删除转码源文件：{}",
                video.display()
            ),
            Err(error) => log!(
                &runtime.logger,
                Warn,
                "删除源文件失败（{}）：{error}",
                video.display()
            ),
        }
    }
    mark_status(runtime, item, TorrentStatus::Done);
    log!(&runtime.logger, Info, "已发送：{}", item.title);
    Ok(())
}

/// 轮询到下载完成；超时或进入失败态就返回错误。
fn wait_done(runtime: &Runtime, qb: &Qb, hash: &str) -> Result<Value> {
    let config = &runtime.config.qbittorrent;
    let deadline = Instant::now() + Duration::from_secs(config.download_timeout_secs.max(60));
    let interval = Duration::from_secs(config.poll_interval_secs.max(1));
    loop {
        let torrents = qb.list(&[hash.to_owned()])?;
        if let Some(torrent) = torrents.into_iter().next() {
            let progress = torrent.get("progress").and_then(Value::as_f64).unwrap_or(0.0);
            let state = torrent.get("state").and_then(Value::as_str).unwrap_or("");
            if progress >= 1.0 {
                return Ok(torrent);
            }
            if matches!(state, "error" | "missingFiles" | "unknown") {
                return Err(Error::Service {
                    service: "download.qbittorrent".to_owned(),
                    message: format!("种子进入 {state} 状态"),
                });
            }
        }
        if Instant::now() >= deadline {
            return Err(Error::Service {
                service: "download.qbittorrent".to_owned(),
                message: "等待下载超时".to_owned(),
            });
        }
        thread::sleep(interval);
    }
}

/// 从 qB 的文件列表里挑最大的视频。
///
/// `base` 是种子的落盘路径：单文件种子它本身就是文件，多文件种子它是目录，
/// 这两种情况在 qB 5.x 里都叫 `content_path`，只能靠「后缀是不是视频」区分。
pub fn pick_video(files: &[Value], base: &Path) -> Option<PathBuf> {
    if is_video(base) && base.is_file() {
        return Some(base.to_path_buf());
    }
    files
        .iter()
        .filter_map(|file| {
            let name = file.get("name").and_then(Value::as_str)?;
            let size = file.get("size").and_then(Value::as_u64).unwrap_or(0);
            let path = base.join(name);
            is_video(&path).then_some((size, path))
        })
        .max_by_key(|(size, _)| *size)
        .map(|(_, path)| path)
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| VIDEO_EXT.contains(&ext.as_str()))
}

// ─── 转码 ──────────────────────────────────────────────────────────────────

/// 转码意图：兼容优先。对应 `media.ffmpeg` 的 `Intent::Compat`。
const INTENT: &str = "compat";
/// 轮询转码状态的间隔。ffmpeg 插件最快 400ms 推一次进度，1s 足够且不吵。
const JOB_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 「这个文件怎么发」的决定。
#[derive(Debug, Clone, PartialEq, Eq)]
enum SendPlan {
    /// 已经是 mp4 能直接放的组合：发原文件。
    Direct,
    /// 需要先转码；`output_name` 只是文件名，目录由调用方按配置拼。
    Transcode { output_name: String },
}

/// 纯函数：只看探测结果和源路径，决定「用哪个文件发」。
///
/// 判断标准与 ffmpeg 插件的 `MediaInfo::is_mp4_ready` 一致：视频 h264/hevc/av1，
/// 音频没有或 aac/mp3，容器是 mp4。**任何一项读不出来都按「要转码」处理**——
/// 多转一次只花时间，发出去打不开则是整条报废。
fn decide_send_path(probe: &Value, source: &Path) -> SendPlan {
    let video_ok = probe
        .get("video_codec")
        .and_then(Value::as_str)
        .is_some_and(|codec| {
            matches!(codec.to_ascii_lowercase().as_str(), "h264" | "hevc" | "av1")
        });
    let audio_ok = match probe.get("has_audio").and_then(Value::as_bool) {
        Some(false) => true,
        Some(true) => probe
            .get("audio_codec")
            .and_then(Value::as_str)
            .is_some_and(|codec| matches!(codec.to_ascii_lowercase().as_str(), "aac" | "mp3")),
        // 连有没有音频都不知道，保守转码。
        None => false,
    };
    if video_ok && audio_ok && is_mp4(source) {
        SendPlan::Direct
    } else {
        SendPlan::Transcode {
            output_name: mp4_output_name(source),
        }
    }
}

/// `第01集.mkv` → `第01集-mp4.mp4`。带 `-mp4` 是为了在工作目录里一眼看出
/// 这是转码产物，不会和原文件混淆。
fn mp4_output_name(source: &Path) -> String {
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("video");
    format!("{stem}-mp4.mp4")
}

fn is_mp4(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
}

/// 把「源文件」变成「能发给 Telegram 的文件」。
///
/// 返回 `(要发的文件, 是否是转码产物)`。**任何一步失败都不让整条链失败**：
/// 回退到原文件，宁可发一个 Telegram 可能不认的容器，也比什么都没发出去强。
fn prepare_video(runtime: &Runtime, source: &Path) -> (PathBuf, bool) {
    let original = || (source.to_path_buf(), false);

    let media = Media::new(runtime.host);
    if !media.available() {
        log!(
            &runtime.logger,
            Warn,
            "media.ffmpeg 不可用，直接发原文件：{}",
            source.display()
        );
        return original();
    }

    let probe = match media.probe(source) {
        Ok(probe) => probe,
        Err(error) => {
            log!(
                &runtime.logger,
                Warn,
                "探测失败，直接发原文件（{}）：{error}",
                source.display()
            );
            return original();
        }
    };

    let SendPlan::Transcode { output_name } = decide_send_path(&probe, source) else {
        log!(
            &runtime.logger,
            Info,
            "已是 mp4 兼容格式，直接发送：{}",
            source.display()
        );
        return original();
    };

    let work_dir = work_dir(&runtime.config.media.work_dir);
    let output = work_dir.join(output_name);
    if let Err(error) = std::fs::create_dir_all(&work_dir) {
        log!(
            &runtime.logger,
            Warn,
            "创建转码目录失败（{}），直接发原文件：{error}",
            work_dir.display()
        );
        return original();
    }

    // 先出方案再起任务：`transcode` 内部会再算一遍，但把「为什么选它」写进
    // 日志，比事后只看到「转码失败」有用得多。
    match media.plan(source, &output, INTENT) {
        Ok(reply) => {
            let plan = &reply["plan"];
            log!(
                &runtime.logger,
                Info,
                "转码方案：{}｜{}｜{}",
                plan["mode"].as_str().unwrap_or("?"),
                plan["encoder"].as_str().unwrap_or("?"),
                plan["reason"].as_str().unwrap_or("")
            );
        }
        Err(error) => {
            log!(
                &runtime.logger,
                Warn,
                "转码方案计算失败，直接发原文件（{}）：{error}",
                source.display()
            );
            return original();
        }
    }

    let job_id = match media.transcode(source, &output, INTENT) {
        Ok(job_id) => job_id,
        Err(error) => {
            log!(
                &runtime.logger,
                Warn,
                "起转码任务失败，直接发原文件（{}）：{error}",
                source.display()
            );
            return original();
        }
    };
    log!(
        &runtime.logger,
        Info,
        "开始转码（{job_id}）：{} → {}",
        source.display(),
        output.display()
    );

    match wait_transcode(&media, &job_id, runtime.config.media.timeout_secs) {
        Ok(()) => (output, true),
        Err(error) => {
            // 失败留下的半成品可能占几百 MB，顺手清掉。
            let _ = std::fs::remove_file(&output);
            log!(
                &runtime.logger,
                Warn,
                "转码失败，回退原文件（{}）：{error}",
                source.display()
            );
            original()
        }
    }
}

/// 轮询到任务结束。失败 / 取消 / 超时都返回错误，由调用方决定回退。
fn wait_transcode(media: &Media, job_id: &str, timeout_secs: u64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs.max(1));
    loop {
        let status = media.job_status(job_id)?;
        let state = status.get("state").and_then(Value::as_str).unwrap_or("");
        match state {
            "done" => return Ok(()),
            "failed" | "canceled" => {
                let reason = status
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("没有给出原因");
                return Err(Error::Service {
                    service: "media.ffmpeg".to_owned(),
                    message: format!("转码任务 {job_id} 以 {state} 结束：{reason}"),
                });
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(Error::Service {
                service: "media.ffmpeg".to_owned(),
                message: format!("转码任务 {job_id} 超过 {timeout_secs}s 未结束"),
            });
        }
        thread::sleep(JOB_POLL_INTERVAL);
    }
}

/// 转码产物的落盘目录：配置优先，空则系统 temp 下的 `xq-anime`。
fn work_dir(configured: &str) -> PathBuf {
    let configured = configured.trim();
    if configured.is_empty() {
        std::env::temp_dir().join("xq-anime")
    } else {
        PathBuf::from(configured)
    }
}

/// 频道文案。TS 用 Markdown + 标签 + 链接，这里先保持一条能读的纯文本。
pub fn caption(item: &AnimeItem) -> String {
    let name = item.names.first().map(String::as_str).unwrap_or(&item.title);
    match item.episode.as_deref() {
        Some(episode) if !episode.is_empty() => format!("{name}｜第 {episode} 集"),
        _ => name.to_owned(),
    }
}

fn mark_status(runtime: &Runtime, item: &AnimeItem, status: TorrentStatus) {
    let filter = serde_json::json!({ "title": anime_core::title::clean_title(&item.title) });
    let update = serde_json::json!({
        "$set": { "status": status, "updatedAt": chrono::Utc::now() }
    });
    if let Err(error) = runtime.store.update("torrents", filter, update, false, false) {
        log!(&runtime.logger, Warn, "更新种子状态失败：{error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_the_largest_video_and_ignores_extras() {
        let files = vec![
            json!({ "name": "第01集.mkv", "size": 900 }),
            json!({ "name": "第01集.mp4", "size": 300 }),
            json!({ "name": "说明.txt", "size": 1 }),
        ];
        let picked = pick_video(&files, Path::new("D:/dl/番剧")).expect("应当挑到视频");
        assert!(picked.ends_with("第01集.mkv"), "{picked:?}");
    }

    #[test]
    fn single_file_content_path_is_used_directly() {
        let dir = std::env::temp_dir().join("xq-chain-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("movie.mkv");
        std::fs::write(&file, b"x").unwrap();
        let picked = pick_video(&[], &file).expect("单文件种子直接用 content_path");
        assert_eq!(picked, file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caption_uses_name_and_episode() {
        let mut item = AnimeItem {
            title: "[组] 某番 - 05".to_owned(),
            pub_date: String::new(),
            magnet: String::new(),
            link: String::new(),
            team: "组".to_owned(),
            fansub: vec![],
            names: vec!["某番".to_owned()],
            source: None,
            episode: Some("05".to_owned()),
        };
        assert_eq!(caption(&item), "某番｜第 05 集");
        item.episode = None;
        assert_eq!(caption(&item), "某番");
    }

    #[test]
    fn compatible_mp4_is_sent_directly() {
        let probe = json!({ "video_codec": "h264", "audio_codec": "aac", "has_audio": true });
        let source = Path::new("D:/dl/第01集.mp4");
        assert_eq!(decide_send_path(&probe, source), SendPlan::Direct);
    }

    #[test]
    fn mkv_needs_transcoding_and_gets_mp4_suffix() {
        let probe = json!({ "video_codec": "h264", "audio_codec": "aac", "has_audio": true });
        let source = Path::new("D:/dl/第01集.mkv");
        assert_eq!(
            decide_send_path(&probe, source),
            SendPlan::Transcode {
                output_name: "第01集-mp4.mp4".to_owned()
            }
        );
    }

    #[test]
    fn missing_probe_fields_fall_back_to_transcoding() {
        // 字段缺一半时不能赌「大概兼容」：宁可多转一次。
        let source = Path::new("D:/dl/第01集.mp4");
        assert!(matches!(
            decide_send_path(&json!({}), source),
            SendPlan::Transcode { .. }
        ));
        // has_audio 缺失、只有视频编码：仍然保守。
        assert!(matches!(
            decide_send_path(&json!({ "video_codec": "h264" }), source),
            SendPlan::Transcode { .. }
        ));
        // 音频不兼容（flac）同样要转。
        assert!(matches!(
            decide_send_path(
                &json!({ "video_codec": "h264", "audio_codec": "flac", "has_audio": true }),
                source
            ),
            SendPlan::Transcode { .. }
        ));
    }
}
