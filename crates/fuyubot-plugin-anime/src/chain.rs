//! 命中之后的那一段：下载 → 等完成 → 挑文件 → 转码 → 发频道。
//!
//! 对应 TS 的 `qBittorrent/download.ts` + `anime/sendAnime.ts` 里最小的一条直线。
//! 转码只做「换个容器 / 转成兼容编码 / 烧中文字幕」这一层，具体参数由
//! `media.ffmpeg` 的 compat 方案决定；**故意省掉了**：分段上传、导航消息、
//! 审核按钮、检查点。这些每一项都有自己的状态机，混进来会把「先跑通一条直线」
//! 这件事拖没。
//!
//! 线程模型：`poll` 发现可投放的条目就调 [spawn]，这里用固定槽位限流。
//! 下载可能要几十分钟，绝不能占用轮询线程——否则下一轮 RSS 永远抓不到。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anime_core::ai::MatchResult;
use anime_core::matching::EpisodeMatch;
use anime_core::models::{AnimeItem, TorrentStatus};
use anime_core::store::{Storage, btih_from_magnet};
use anime_core::{Error, Result, log};
use serde_json::Value;

use crate::media::Media;
use crate::qb::Qb;
use crate::runtime::Runtime;
use crate::td::{SentPart, Target, VideoMeta, VideoPart};

/// 同时最多几条在下载。TS 的 `AnimeProcessorManager` 也是 3 个 worker。
const WORKERS: usize = 3;
/// qB 分类，方便在 qB 界面里一眼认出哪些是这个机器人加的。
const CATEGORY: &str = "xq-anime";
/// 封面取第几秒。太靠前的帧常常是黑场或制作委员会的 logo（TS 也是 10 秒）。
const COVER_SECOND: u64 = 10;
/// 认得出的视频后缀（小写）。
const VIDEO_EXT: [&str; 5] = ["mkv", "mp4", "avi", "ts", "m2ts"];

static SLOTS: LazyLock<(Mutex<usize>, Condvar)> =
    LazyLock::new(|| (Mutex::new(0), Condvar::new()));
static IN_FLIGHT: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// 这一条投放「发到哪、发完之后干什么」。
///
/// 以前 `spawn` 只带一个 `anime_id`，隐含「命中了、集数也对了、发正片频道」——
/// 这一条假设装不下 TS 的三条路：集数没对齐要发**缓冲频道**并请管理员纠正，
/// 新番高置信度要**先发后审**（视频已经在正片频道了，审核在后面）。
/// 所以把「目标」和「发完的动作」显式化成计划，由调用方挑。
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// 命中的番剧（Bangumi subject id）。`0` = 没确定是哪部番。
    pub anime_id: i64,
    /// 命中的章节 id（集数没对齐时为 `None`）。
    pub episode_id: Option<i64>,
    /// `cacheItem` 的自增 id：审核提示的按钮要带它，管理员点「提供」时才能回查条目。
    pub cache_id: i64,
    /// 发**缓冲频道**而不是正片频道。
    ///
    /// 缓冲频道是「还没确认过」的暂存区：视频发过去、种子状态置 `等待纠正`，
    /// 管理员确认后才转进正片。发错正片是撤不回的，发错缓冲只是多一步纠正。
    pub to_cache: bool,
    /// 发完之后提示管理员（缓冲频道的确认 / 正片频道的先发后审）。
    pub review: bool,
    /// 集数匹配结论：提示里要说明「哪里没对上」。
    pub episode_match: Option<EpisodeMatch>,
    /// LLM 匹配详情：提示里要展示置信度与理由（可溯源纠正）。
    pub match_detail: Option<MatchResult>,
}

impl Plan {
    /// 老行为：命中且集数对齐，发正片频道 + 记导航，不进审核。
    pub fn channel(anime_id: i64) -> Self {
        Self {
            anime_id,
            ..Default::default()
        }
    }

    /// 缓冲频道：发到管理群等纠正，发完请管理员确认。
    ///
    /// `episode_match` 决定提示的两个分支：日期不符（机器有答案）给「正确」，
    /// 其余（库里没这集）给「提供正确集数」。
    pub fn cache(
        anime_id: i64,
        cache_id: i64,
        episode_match: Option<EpisodeMatch>,
        match_detail: Option<MatchResult>,
    ) -> Self {
        Self {
            anime_id,
            cache_id,
            to_cache: true,
            review: true,
            episode_match,
            match_detail,
            ..Default::default()
        }
    }

    /// 先发后审：正片频道照发，发完建待审核记录 + 带候选的确认提示。
    pub fn post_send(
        anime_id: i64,
        cache_id: i64,
        episode_id: Option<i64>,
        match_detail: Option<MatchResult>,
    ) -> Self {
        Self {
            anime_id,
            cache_id,
            episode_id,
            review: true,
            match_detail,
            ..Default::default()
        }
    }
}

/// 起一个后台任务：命中且集数对齐，发正片频道。
///
/// 拿不到槽位就等——阻塞的是调用方线程（轮询线程）……
/// 所以调用方必须在自己的线程里调它，**不能**在命令回调里直接调。
pub fn spawn(runtime: Arc<Runtime>, item: AnimeItem, anime_id: i64) {
    spawn_with(runtime, item, Plan::channel(anime_id));
}

/// 按 [Plan] 起一个后台任务。
pub fn spawn_with(runtime: Arc<Runtime>, item: AnimeItem, plan: Plan) {
    // dmhy 的 enclosure 往往是 .torrent 地址而不是 magnet:，拿不到 btih。
    // 这不影响下载（qB 自己能算 hash），只影响去重键——所以退化用标题当键。
    let known_hash = btih_from_magnet(&item.magnet);
    let task_key = known_hash
        .clone()
        .unwrap_or_else(|| checkpoint_key(&item));
    // 同一条种子只下一次：轮询可能在同一秒里重复投递（Seen 失效、手动触发等）。
    {
        let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        if !in_flight.insert(task_key.clone()) {
            return;
        }
    }
    acquire_slot();
    let worker = Arc::clone(&runtime);
    let title = item.title.clone();
    let release_key = task_key.clone();
    let spawned = thread::Builder::new()
        .name("xq-anime-chain".to_owned())
        .spawn(move || {
            let result = run(&worker, &item, &plan, known_hash.as_deref());
            if let Err(error) = result {
                log!(&worker.logger, Error, "下载链路失败（{title}）：{error}");
                let _ = worker.store.checkpoint_finish(
                    &checkpoint_key(&item),
                    anime_core::models::CheckpointStatus::Failed,
                );
            }
            release(&release_key);
        });
    if spawned.is_err() {
        release(&task_key);
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

/// 超过上传上限就分段，否则原样返回单元素。
///
/// 分段失败只是「不分段」：宁可发一个可能超限的文件，也别什么都不发。
fn split_for_upload(runtime: &Runtime, path: &Path) -> Vec<PathBuf> {
    let max = runtime.config.media.max_upload_bytes;
    let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if max == 0 || size <= max {
        return vec![path.to_path_buf()];
    }
    let media = Media::new(runtime.host);
    let duration_ms = media
        .probe(path)
        .ok()
        .and_then(|info| info.get("duration_ms").and_then(Value::as_u64))
        .unwrap_or(0);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("video");
    let out_dir = work_dir(&runtime.config.media.work_dir).join(format!("{stem}-parts"));
    log!(
        &runtime.logger,
        Info,
        "文件 {} MB 超过上限，开始分段：{}",
        size / 1024 / 1024,
        path.display()
    );
    match media.split(path, &out_dir, max, duration_ms) {
        Ok(parts) => {
            log!(&runtime.logger, Info, "已切成 {} 段", parts.len());
            parts
        }
        Err(error) => {
            log!(&runtime.logger, Warn, "分段失败，按原文件发送：{error}");
            vec![path.to_path_buf()]
        }
    }
}

/// 发完就删分段文件和目录——它们只是上传用的中间产物。
fn cleanup_parts(runtime: &Runtime, parts: &[PathBuf]) {
    for part in parts {
        if let Err(error) = std::fs::remove_file(part) {
            log!(&runtime.logger, Warn, "删除分段失败（{}）：{error}", part.display());
        }
    }
    if let Some(dir) = parts.first().and_then(|part| part.parent()) {
        let _ = std::fs::remove_dir(dir);
    }
}

/// 检查点主键：清理后的标题。同一条种子重复入队时只会更新同一条记录。
pub fn checkpoint_key(item: &AnimeItem) -> String {
    anime_core::title::clean_title(&item.title)
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

/// 没有 btih 的种子（dmhy 给的是 .torrent 地址）：加种后从 qB 列表里找新出现的那条。
///
/// 不按标题匹配：qB 的 `name` 会被它自己改（去掉扩展名、截断），按标题找经常找不着。
/// 用「加种前后的 hash 差集」最稳，最多等 30 秒（qB 要下载 .torrent 才能算出 hash）。
fn add_and_discover(runtime: &Runtime, qb: &Qb, item: &AnimeItem) -> Result<String> {
    let before: HashSet<String> = qb
        .list(&[])?
        .iter()
        .filter_map(|torrent| torrent.get("hash").and_then(Value::as_str).map(str::to_owned))
        .collect();
    qb.add(
        &item.magnet,
        &runtime.config.qbittorrent.save_path,
        CATEGORY,
    )?;
    for _ in 0..30 {
        thread::sleep(Duration::from_secs(1));
        let now = qb.list(&[])?;
        if let Some(hash) = now.iter().find_map(|torrent| {
            let hash = torrent.get("hash").and_then(Value::as_str)?;
            (!before.contains(hash)).then(|| hash.to_owned())
        }) {
            return Ok(hash);
        }
    }
    Err(Error::Service {
        service: "download.qbittorrent".to_owned(),
        message: "加种后 30 秒内没在 qB 列表里看到新种子".to_owned(),
    })
}

/// 探测出视频的真实参数（`duration` / `width` / `height`）。
///
/// 探测失败（ffmpeg 不在 / 文件怪）就返回全 0：Telegram 自己会重算，
/// 只是客户端里先显示成未知尺寸。宁可这样也别因为拿不到参数就不发。
fn video_meta(runtime: &Runtime, path: &Path, spoiler: bool) -> VideoMeta {
    let media = Media::new(runtime.host);
    let probe = match media.probe(path) {
        Ok(probe) => probe,
        Err(error) => {
            log!(&runtime.logger, Debug, "探测视频参数失败（{}）：{error}", path.display());
            return VideoMeta { spoiler, ..Default::default() };
        }
    };
    VideoMeta {
        duration: (probe.get("duration_ms").and_then(Value::as_u64).unwrap_or(0) / 1000) as i32,
        width: probe.get("width").and_then(Value::as_u64).unwrap_or(0) as i32,
        height: probe.get("height").and_then(Value::as_u64).unwrap_or(0) as i32,
        spoiler,
        // 封面由 [video_part] 另外抽：探测失败时也不该放弃封面。
        cover: None,
    }
}

/// 一段视频的完整发送描述：真实参数 + 抽出来的封面帧。
fn video_part(runtime: &Runtime, path: &Path, spoiler: bool, index: usize) -> VideoPart {
    let mut meta = video_meta(runtime, path, spoiler);
    meta.cover = extract_cover(runtime, path, index);
    VideoPart {
        path: path.display().to_string(),
        meta,
    }
}

/// 抽一帧做封面（TS 取第 10 秒）。失败返回 `None`。
///
/// 没有封面不是失败：Telegram 自己会从视频里抓一帧，只是抓哪一帧由它决定，
/// 常常抓到黑场。所以这里是「尽力而为」，失败只记日志。
fn extract_cover(runtime: &Runtime, path: &Path, index: usize) -> Option<String> {
    let media = Media::new(runtime.host);
    if !media.available() {
        return None;
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("video");
    let output = work_dir(&runtime.config.media.work_dir).join(format!("cover_{stem}_{index}.jpg"));
    match media.thumbnail(path, &output, COVER_SECOND) {
        Ok(_) => Some(output.display().to_string()),
        Err(error) => {
            log!(
                &runtime.logger,
                Warn,
                "抽封面失败（{}）：{error}",
                path.display()
            );
            None
        }
    }
}

/// 逐条发送时的文案：多段才加「（n/N）」。
///
/// 相册路线不加后缀——相册里的各项是一集拆出来的，序号没有意义
/// （TS 也是整组只给首项带文案）。
fn part_text(base: &str, index: usize, total: usize) -> String {
    if total > 1 {
        format!("{base}（{}/{}）", index + 1, total)
    } else {
        base.to_owned()
    }
}

/// 封面是临时产物，与分段一样发完就删。
fn cleanup_covers(runtime: &Runtime, parts: &[VideoPart]) {
    for path in parts.iter().filter_map(|part| part.meta.cover.as_deref()) {
        if let Err(error) = std::fs::remove_file(path) {
            log!(&runtime.logger, Warn, "删除封面失败（{path}）：{error}");
        }
    }
}

/// 缓冲频道的目标会话。
///
/// TS 的 `sendMegToCache` 固定发 `ADMIN_GROUP_ID` 的 `ANIME_GROUP_THREAD_ID` 话题——
/// 缓冲和正片在同一个群里，区别只在话题。这里允许单独配一个 `cache_channel`
/// （有的部署会把缓冲单独开一个群）；没配就退回 TS 的那套（管理群 + 番剧话题）。
pub fn cache_target(telegram: &anime_core::config::TelegramConfig) -> Target {
    let chat_id = if telegram.cache_channel != 0 {
        telegram.cache_channel
    } else {
        telegram.admin_group_id
    };
    Target::new(chat_id, 0).with_topic(telegram.anime_group_thread_id)
}
fn run(
    runtime: &Runtime,
    item: &AnimeItem,
    plan: &Plan,
    known_hash: Option<&str>,
) -> Result<()> {
    let qb = Qb::new(runtime.host);
    if !qb.available() {
        return Err(Error::Service {
            service: "download.qbittorrent".to_owned(),
            message: "插件未安装或未启动".to_owned(),
        });
    }
    let config = &runtime.config;
    // 检查点先落：进程崩在这里，重启后还能从「下载」这一步重放。
    let key = checkpoint_key(item);
    if let Err(error) = runtime.store.checkpoint_start(
        &key,
        anime_core::models::CheckpointKind::Rss,
        "下载",
        &serde_json::json!({ "kind": "rss", "item": item }),
    ) {
        log!(&runtime.logger, Warn, "写检查点失败（继续下载）：{error}");
    }
    // 先记账再下载：中途崩了至少知道这条被接手过（TS 也是先写 torrents）。
    if let Err(error) = runtime
        .store
        .add_torrent(&item.magnet, TorrentStatus::Downloading, &item.title)
    {
        log!(&runtime.logger, Warn, "写入种子记录失败（继续下载）：{error}");
    }
    // 有 btih 就直接用；没有（dmhy 的 .torrent 地址）就让 qB 算完再从列表里找出来。
    let hash = match known_hash {
        Some(hash) => {
            qb.add(&item.magnet, &config.qbittorrent.save_path, CATEGORY)?;
            hash.to_owned()
        }
        None => add_and_discover(runtime, &qb, item)?,
    };
    log!(
        &runtime.logger,
        Info,
        "已加种：{}（anime {}）",
        item.title,
        plan.anime_id
    );

    let torrent = wait_done(runtime, &qb, &hash)?;
    let _ = runtime.store.checkpoint_stage(&key, "转码");
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

    let files = qb.files(&hash)?;
    let video = pick_video(&files, &base).ok_or_else(|| Error::Service {
        service: "download.qbittorrent".to_owned(),
        message: format!("种子里没有视频文件（{} 个文件）", files.len()),
    })?;

    // 转码是**尽力而为**：任何一步失败都回退到原文件，不让「发不出去」成为结果。
    let (send_path, transcoded) = prepare_video(runtime, &video);

    let _ = runtime.store.checkpoint_stage(&key, "发送");
    // 上传中：从这里到「发送成功确认」可能要几十分钟（TG 上传 + 转码），
    // 状态停在「下载完成」会让 /progress 看起来像卡在下载。
    mark_status(runtime, item, TorrentStatus::Uploading);
    // 超过上传上限就按大小分段（TG 约 2GB）。分段是流复制，秒级。
    let parts = split_for_upload(runtime, &send_path);
    let total = parts.len();
    let target = if plan.to_cache {
        cache_target(&config.telegram)
    } else {
        Target::new(config.telegram.anime_channel, 0).with_topic(config.telegram.anime_group_thread_id)
    };
    // 视频文案：有番剧文档就给完整的 AnimeText（原名/中文名/发布组/四个链接），
    // 没有（还没确定是哪部番）才退回「番名｜第 N 集」。
    let anime = (plan.anime_id > 0)
        .then(|| runtime.store.anime_by_id(plan.anime_id).ok().flatten())
        .flatten();
    let base_text = match (&anime, plan.to_cache) {
        // 缓冲频道只用标题（TS 的 `sendMegToCache`）：那里是暂存区，视频还没确定
        // 是哪部番，挂上「导航·收藏·标记看过」那四个链接反而是错的。
        (_, true) | (None, _) => caption(item),
        (Some(anime), false) => crate::text::anime_text(anime, item, plan.episode_id.unwrap_or(0)),
    };
    let spoiler = anime.as_ref().and_then(|anime| anime.r18).unwrap_or(false);

    // 每段都要探测真实参数 + 抽封面：没有它们客户端里这一集是「时长 0:00」的灰块。
    let metas: Vec<VideoPart> = parts
        .iter()
        .enumerate()
        .map(|(index, part)| video_part(runtime, part, spoiler, index))
        .collect();
    let mut sent: Vec<SentPart> = if !config.telegram.wait_send {
        // 不等确认：逐段投递（回放 / 无 TDLib 环境），拿不到 id 也拿不到 videoid。
        for (index, part) in metas.iter().enumerate() {
            let text = part_text(&base_text, index, total);
            runtime.td.send_video_now(target, part, &text);
        }
        Vec::new()
    } else if (2..=10).contains(&total) {
        // 分段 = 相册（TS 的 sendMessageAlbum）：一组消息，只有首项带文案。
        // 超过 10 段（TG 的相册上限）就退化成逐条发。
        runtime
            .td
            .send_video_album_wait(target, &metas, &base_text)
            .unwrap_or_default()
    } else {
        metas
            .iter()
            .enumerate()
            .filter_map(|(index, part)| {
                let text = part_text(&base_text, index, total);
                runtime.td.send_video_wait(target, part, &text)
            })
            .collect()
    };
    // 不等确认时拿不到结果，不知道成败——只有「等了却一个都没成」才算失败。
    let send_failed = config.telegram.wait_send && sent.is_empty();
    if send_failed {
        log!(
            &runtime.logger,
            Warn,
            "发送失败：{}（{} 段）",
            item.title,
            total
        );
    }
    // 不等确认时后台线程还在上传，文件不能删（删了上传必失败）；
    // 这条路线是「回放 / 没接 TDLib」时才用，宁可留着临时文件也别发坏。
    if config.telegram.wait_send {
        if total > 1 {
            cleanup_parts(runtime, &parts);
        }
        cleanup_covers(runtime, &metas);
    }
    // 转码产物发不出去时再试原文件：这一步也属于「尽力而为」的一环。
    if send_failed && total == 1 && transcoded {
        log!(
            &runtime.logger,
            Warn,
            "发送转码产物失败，回退原文件：{}",
            send_path.display()
        );
        let part = video_part(runtime, &video, spoiler, 0);
        if let Some(again) = runtime
            .td
            .send_video_wait(target, &part, &base_text)
        {
            sent.push(again);
        }
        cleanup_covers(runtime, std::slice::from_ref(&part));
    }
    // 主消息 = 第一段：导航、资源、消息链接都以它为准（TS 的 primaryMeg）。
    let message_id = sent.first().map(|part| part.message_id).unwrap_or(0);
    if config.telegram.wait_send && message_id == 0 {
        return Err(Error::Service {
            service: "td".to_owned(),
            message: "发视频失败或未确认".to_owned(),
        });
    }
    // 资源是「哪部番第几集、发到哪条消息」的唯一真相，每次发送都要记
    // （TS 的 saveAnimeResource 在每条路径上都有）。
    if plan.anime_id > 0 && message_id > 0 {
        crate::review::record_send(runtime, plan, item, target.chat_id, &sent);
        // 只有正片频道才刷新导航：缓冲频道里的视频还没确认过，写进导航等于把
        // 未审核的东西公开出去（TS 也是 `sendMegToAnime` 之后才 `sendMegToNavAnime`）。
        if !plan.to_cache && let Err(error) = crate::nav::refresh(runtime, plan.anime_id) {
            log!(&runtime.logger, Warn, "刷新导航失败：{error}");
        }
    }
    // 发完之后要不要请管理员确认，交给 review——那里才知道提示该长什么样。
    if plan.review {
        crate::review::prompt_after_send(runtime, plan, item);
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
    // 缓冲频道的种子停在「等待纠正」：管理员还没确认集数，这条不算完。
    mark_status(
        runtime,
        item,
        if plan.to_cache {
            TorrentStatus::PendingFix
        } else {
            TorrentStatus::Done
        },
    );

    // 清理：发送完还留着种子会一直做种；源文件被删的情况下 qB 还会报 missingFiles。
    if config.qbittorrent.delete_torrent_after_send {
        match qb.delete(&hash, true) {
            Ok(()) => log!(&runtime.logger, Info, "已从 qB 删除种子与文件：{}", item.title),
            Err(error) => log!(&runtime.logger, Warn, "从 qB 删除种子失败：{error}"),
        }
    }
    let _ = runtime
        .store
        .checkpoint_finish(&key, anime_core::models::CheckpointStatus::Done);
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
/// 字幕抽不出来 / 烧字幕转码失败 → 去掉字幕再转一次；再失败才回退原文件，
/// 宁可发一个 Telegram 可能不认的容器，也比什么都没发出去强。
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

    // 内嵌字幕先抽成外挂 ass，再带着它转码。抽不出来只是「不烧字幕」，不是失败。
    let subtitle = extract_subtitle(runtime, &media, &probe, source, &work_dir);

    // 先烧字幕转一次；滤镜跑挂（缺 libass / 字体异常 / 抽出的文件坏了）就去掉
    // 字幕再转一次——对齐 TS 的「失败回退 compat」，绝不让整条链失败。
    let mut outcome = transcode_once(runtime, &media, source, &output, subtitle.as_deref());
    if let Err(error) = &outcome
        && subtitle.is_some()
    {
        log!(
            &runtime.logger,
            Warn,
            "带字幕转码失败，去掉字幕重试（{}）：{error}",
            source.display()
        );
        let _ = std::fs::remove_file(&output);
        outcome = transcode_once(runtime, &media, source, &output, None);
    }
    // 抽出来的 .ass 是临时产物，不管成败都清掉。
    if let Some(path) = &subtitle {
        let _ = std::fs::remove_file(path);
    }

    match outcome {
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

/// 出方案 → 起任务 → 等结束。任何一步失败都往上抛，由 [prepare_video] 决定回退。
fn transcode_once(
    runtime: &Runtime,
    media: &Media,
    source: &Path,
    output: &Path,
    subtitle: Option<&Path>,
) -> Result<()> {
    // 把「为什么选它」写进日志，比事后只看到「转码失败」有用得多。
    let reply = media.plan(source, output, INTENT, subtitle)?;
    let plan = &reply["plan"];
    log!(
        &runtime.logger,
        Info,
        "转码方案：{}｜{}｜{}",
        plan["mode"].as_str().unwrap_or("?"),
        plan["encoder"].as_str().unwrap_or("?"),
        plan["reason"].as_str().unwrap_or("")
    );

    let job_id = media.transcode(source, output, INTENT, subtitle)?;
    log!(
        &runtime.logger,
        Info,
        "开始转码（{job_id}）{}：{} → {}",
        subtitle
            .map(|path| format!("，烧录字幕 {}", path.display()))
            .unwrap_or_default(),
        source.display(),
        output.display()
    );
    wait_transcode(media, &job_id, runtime.config.media.timeout_secs)
}

/// 有内嵌字幕就先抽成外挂 ass。抽不出来只记日志并返回 `None`（不烧字幕）。
fn extract_subtitle(
    runtime: &Runtime,
    media: &Media,
    probe: &Value,
    source: &Path,
    work_dir: &Path,
) -> Option<PathBuf> {
    if !probe
        .get("has_subtitle")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let output = work_dir.join(subtitle_output_name(source));
    match media.extract_subtitle(source, &output) {
        Ok(Some(path)) => {
            log!(&runtime.logger, Info, "已抽出内嵌字幕：{}", path.display());
            Some(path)
        }
        Ok(None) => {
            log!(
                &runtime.logger,
                Warn,
                "没有可抽出的字幕流，按不烧字幕转码：{}",
                source.display()
            );
            None
        }
        Err(error) => {
            log!(
                &runtime.logger,
                Warn,
                "抽字幕失败，按不烧字幕转码（{}）：{error}",
                source.display()
            );
            None
        }
    }
}

/// 字幕产物名：只留字母数字和 `-`/`_`，其余全换成 `_`。
///
/// 这个名字会进 ffmpeg 的 `-vf subtitles=`，逗号/分号/方括号在滤镜语法里
/// 都是元字符；先把能换的都换掉，剩下 work_dir 那一段才需要转义。
fn subtitle_output_name(source: &Path) -> String {
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("video");
    let safe: String = stem
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}-sub.ass")
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
    fn part_text_only_numbers_multi_part_sends() {
        assert_eq!(part_text("某番｜第 01 集", 0, 1), "某番｜第 01 集");
        assert_eq!(part_text("某番｜第 01 集", 0, 3), "某番｜第 01 集（1/3）");
        assert_eq!(part_text("某番｜第 01 集", 2, 3), "某番｜第 01 集（3/3）");
    }

    #[test]
    fn album_is_only_used_within_telegram_limits() {
        // 相册 2–10 项：单段走 sendMessage，超过 10 段退化成逐条发。
        let uses_album = |total: usize| (2..=10).contains(&total);
        assert!(!uses_album(1));
        assert!(uses_album(2) && uses_album(10));
        assert!(!uses_album(11));
    }

    #[test]
    fn subtitle_output_name_avoids_filter_metacharacters() {
        let name = subtitle_output_name(Path::new("D:/dl/[组] 第01集, it's.mkv"));
        assert_eq!(name, "_组__第01集__it_s-sub.ass");
        assert!(!name.contains([',', '[', ']', '\'', ' ']), "{name}");
        // 没有文件名时也要能给出合法名字，不能 panic。
        assert_eq!(subtitle_output_name(Path::new("")), "video-sub.ass");
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
