//! 命中之后的那一段：下载 → 等完成 → 挑文件 → 发频道。
//!
//! 对应 TS 的 `qBittorrent/download.ts` + `anime/sendAnime.ts` 里最小的一条直线，
//! **故意省掉了**：转码（MKV 烧字幕）、分段上传、导航消息、审核按钮、检查点。
//! 这些每一项都有自己的状态机，混进来会把「先跑通一条直线」这件事拖没。
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

    let sent = runtime.td.send_video(
        Target::new(config.telegram.anime_channel, 0),
        video.display().to_string(),
        caption(item),
    );
    if !sent {
        return Err(Error::Service {
            service: "td".to_owned(),
            message: "发视频失败".to_owned(),
        });
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
}
