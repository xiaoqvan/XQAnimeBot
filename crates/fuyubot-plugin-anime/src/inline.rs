//! 内联查询（inline mode）：在任意聊天里打 `@机器人 关键词` 直接取导航/视频。
//!
//! 对应 TS 的 `inline/animeNavigation.ts` + `inline/episodeVideo.ts`，以及
//! `index.ts` 里那张 `inlineHandlers` 注册表（两个 matcher 打分、取分高的那个）。
//!
//! ## 宿主侧怎么走通（与计划文档的旧结论不同）
//!
//! `docs/plan-parity-newdbtest.md` 阶段 8 原来写的是「做不了」：ABI 里没有 inline
//! 注册入口，`fuyubot-tdlib-ext` 只能发不能收。这个结论**只对了一半**——
//! `HostRuntime::publish_update` 除了兜底的 `"update"` 通道，还会按 `@type`
//! **单独发一遍**，所以订阅 `updateNewInlineQuery` 就能收到查询；回包用
//! `TdExt::answer_inline_query`（`td_api.tl` 的 `answerInlineQuery`）。整条链不需要
//! 宿主改一行代码。
//!
//! ```text
//! updateNewInlineQuery id:int64 sender_user_id:int53 user_location ... query:string offset:string
//! answerInlineQuery inline_query_id:int64 is_personal:Bool button:... results:vector<InputInlineQueryResult>
//!                   cache_time:int32 next_offset:string
//! ```
//!
//! ## 结果形状：为什么是 article 而不是 video
//!
//! `inputInlineQueryResultVideo` 的 `video_url` / `thumbnail_url` 是**给客户端预览用的
//! HTTP URL**，而我们手上只有 Telegram 的文件 id（`message.content.video.video.remote.id`）
//! ——把文件 id 塞进 `video_url` 会被服务端按「URL 非法」退回来。所以每条结果都用
//! `inputInlineQueryResultArticle` 卡片，真正要发出去的内容放在
//! `input_message_content` 里（`inputMessageVideo` + `inputFileRemote{id}`）：**选卡片、
//! 发视频**，不用重新上传，也不用先下载。
//!
//! 卡片缩略图用番剧封面（`anime.image`，是个真 URL，能显示）；拿不到就留空。
//! TS 那边把 `cover.photo.remote.id` 当成 `thumbnail_url` 传，靠它自己的
//! `inlineAdapter` 再翻译——那层适配器不在这个仓库里，这里不猜它的行为。
//!
//! ## 与 TS 的差异
//!
//! - TS 的 `InlineResult` 是 tdl 那层自定义类型，这里直接拼 TDLib 的
//!   `inputInlineQueryResult*`（字段逐个对照 1.8.66 / 1.8.68 的 `td_api.tl`，
//!   这两个版本的这些构造函数完全一致）。
//! - 数字查询（`12345`）按 TS 的分数走 `episodeVideo`（100 > 90）；但**该章节一条
//!   资源都没有时**，退回按番剧 id 查导航——纯数字既可能是章节 id 也可能是番剧 id，
//!   TS 在这个分支只回一句「暂无资源」，白瞎了一次查询。
//! - 每个 handler 只回一条 `answerInlineQuery`（TDLib 一个查询只有最后一次应答生效）。

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;
use std::thread;

use anime_core::log;
use anime_core::models::{Anime, EpisodeResourceDoc};
use anime_core::store::Storage;
use fuyubot_abi::{HostApi, SubscriptionHandle};
use fuyubot_simplify::{InputFile, Media, Simplify, Text, content_json};
use fuyubot_tdlib_ext::TdExt;
use serde_json::{Map, Value, json};

use crate::runtime::Runtime;

/// 用户在内联框里打字时宿主推的更新类型。
pub const INLINE_EVENT: &str = "updateNewInlineQuery";

/// 一条查询最多回几条结果。
///
/// TS 不限条数（服务端会截），但每条都要查一次库、可能还要 `getMessage`，
/// 打字时候选框每变一个字就是一整轮——20 条足够翻，也压得住延迟。
const MAX_RESULTS: usize = 20;

/// 番剧导航结果缓存 5 分钟（TS 的 `cache_time: 300`）。
const NAV_CACHE_SECONDS: i32 = 300;
/// 集数资源结果缓存 10 分钟（TS 的 `cache_time: 600`）。
const EP_CACHE_SECONDS: i32 = 600;
/// 「这一集暂时没资源」那张卡片的缓存（TS 的 `cache_time: 180`）。
const EMPTY_CACHE_SECONDS: i32 = 180;

/// 一条 `updateNewInlineQuery` 里我们关心的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineQuery {
    /// 回话用的 id（`answerInlineQuery.inline_query_id`）。
    pub id: i64,
    /// 发起查询的人；0 表示更新里没带。
    pub sender_user_id: i64,
    /// 用户在 `@机器人` 后面打的内容（**未** trim）。
    pub query: String,
}

/// 从 `updateNewInlineQuery` 的负载里取出查询。
///
/// 手写的回放 / 单测 JSON 常常不带 `@type`，那种也认；带了但不是内联查询的
/// （例如 `updateNewInlineCallbackQuery`）返回 `None`。
pub fn query_of(update: &Value) -> Option<InlineQuery> {
    if let Some(kind) = update.get("@type").and_then(Value::as_str)
        && kind != INLINE_EVENT
    {
        return None;
    }
    Some(InlineQuery {
        id: update.get("id")?.as_i64()?,
        sender_user_id: update
            .get("sender_user_id")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        query: update
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
    })
}

/// 内联查询由哪个 handler 应答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handler {
    /// 番剧导航（文字关键词 / 番剧 id）。
    AnimeNavigation,
    /// 按章节 id 取这一集的视频。
    EpisodeVideo,
}

/// 把查询解析成章节 id：纯数字，或 `bgm.tv/ep/<id>` 链接（TS 的 `parseEpisodeId`）。
pub fn episode_id_of(query: &str) -> Option<i64> {
    let text = query.trim();
    if text.is_empty() {
        return None;
    }
    if text.chars().all(|ch| ch.is_ascii_digit()) {
        return text.parse().ok();
    }

    let lower = text.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return None;
    }
    let rest = ["bgm.tv/ep/", "bangumi.tv/ep/"]
        .iter()
        .find_map(|needle| lower.split_once(needle).map(|(_, rest)| rest.to_owned()))?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    // 路径必须到此为止（`/ep/123` 或 `/ep/123/`），`/ep/123abc` 不算。
    let tail = &rest[digits.len()..];
    if !(tail.is_empty() || tail.starts_with('/')) {
        return None;
    }
    digits.parse().ok()
}

/// 某个 handler 对这条查询的「置信度」，0 表示不接（TS matcher 的返回值）。
pub fn score(handler: Handler, query: &str) -> i32 {
    let query = query.trim();
    match handler {
        Handler::EpisodeVideo => {
            if episode_id_of(query).is_some() {
                100
            } else {
                0
            }
        }
        Handler::AnimeNavigation => {
            if query.is_empty() {
                0
            } else if query.chars().all(|ch| ch.is_ascii_digit()) {
                90
            } else if query.chars().count() >= 2 {
                70
            } else {
                0
            }
        }
    }
}

/// 选分数最高的 handler；同分时按注册顺序取前一个（TS 也是先注册先赢）。
pub fn pick(query: &str) -> Option<Handler> {
    let mut best: Option<(Handler, i32)> = None;
    for handler in [Handler::AnimeNavigation, Handler::EpisodeVideo] {
        let score = score(handler, query);
        if score > 0 && best.is_none_or(|(_, top)| score > top) {
            best = Some((handler, score));
        }
    }
    best.map(|(handler, _)| handler)
}

/// 一条结果卡片的公共部分（`inputInlineQueryResultArticle`）。
///
/// `reply_markup` 是必填字段：内联结果不挂按钮就传**空键盘**
/// （`replyMarkupInlineKeyboard` + 空 `rows`），TDLib 没有单独的「无按钮」值。
pub fn article_result(
    id: &str,
    url: &str,
    title: &str,
    description: &str,
    thumbnail_url: Option<&str>,
    content: Value,
) -> Value {
    let mut object = Map::new();
    object.insert("@type".to_owned(), json!("inputInlineQueryResultArticle"));
    object.insert("id".to_owned(), json!(clip_id(id)));
    object.insert("url".to_owned(), json!(url));
    object.insert("title".to_owned(), json!(title));
    object.insert("description".to_owned(), json!(description));
    object.insert("thumbnail_url".to_owned(), json!(thumbnail_url.unwrap_or("")));
    // 宽高未知：0 就是「不知道」，客户端自己按原图比例排。
    object.insert("thumbnail_width".to_owned(), json!(0));
    object.insert("thumbnail_height".to_owned(), json!(0));
    object.insert(
        "reply_markup".to_owned(),
        json!({ "@type": "replyMarkupInlineKeyboard", "rows": [] }),
    );
    object.insert("input_message_content".to_owned(), content);
    Value::Object(object)
}

/// 纯文本内容（`inputMessageText`）。
pub fn text_content(text: Value) -> Value {
    json!({ "@type": "inputMessageText", "text": text, "clear_draft": false })
}

/// 一段视频的展示参数（从原消息里读出来的真实值）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VideoSize {
    pub duration: i32,
    pub width: i32,
    pub height: i32,
    /// 原消息是 `r18` 时保留遮罩。
    pub spoiler: bool,
}

/// 视频内容（`inputMessageVideo` + `inputFileRemote`）：直接用 Telegram 的文件 id。
///
/// `caption` 是**已经渲染好**的 `formattedText`——标题文本在外面统一过一次
/// Markdown 解析，这里不再重复跑 `parseTextEntities`。
pub fn video_content(video_id: &str, caption: Value, size: VideoSize) -> Value {
    let media = Media::video(InputFile::remote(video_id))
        .duration(size.duration)
        .size(size.width, size.height)
        .streaming(true)
        .spoiler(size.spoiler)
        // 这里的文本只是占位：真正的 caption 由下面的 render 闭包返回。
        .caption(Text::plain(""));
    // `content_json` 是 simplify 唯一一份 `InputMessageContent` 形状实现，
    // 别再手搓一遍（`inputMessageVideo.video` → `inputVideo.photo` 这类层级很容易写错）。
    content_json(&media, &|_: &Text| caption.clone())
}

/// 从一条 `messageVideo` 的 `content` 里读视频 id 与尺寸（TS 的 `extractVideoIdFromMessage`）。
///
/// 结构（`td_api.tl`）：`messageVideo.video:video`，`video.video:file`，
/// `file.remote.id` 就是可以直接复用的文件 id。
pub fn video_from_message(message: &Value) -> Option<(String, VideoSize)> {
    let content = message.get("content")?;
    if content.get("@type").and_then(Value::as_str) != Some("messageVideo") {
        return None;
    }
    let video = content.get("video")?;
    let id = video
        .get("video")
        .and_then(|file| file.get("remote"))
        .and_then(|remote| remote.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())?
        .to_owned();
    let int = |name: &str| {
        video
            .get(name)
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .clamp(0, i32::MAX as i64) as i32
    };
    let size = VideoSize {
        duration: int("duration"),
        width: int("width"),
        height: int("height"),
        spoiler: content
            .get("has_spoiler")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    Some((id, size))
}

/// 结果 id 的上限：TDLib 的 `id` 字段不能超过 64 字节（TS 也是 `.slice(0, 64)`）。
pub fn clip_id(id: &str) -> String {
    id.chars().take(64).collect()
}

/// 一条应答：结果集 + 缓存策略。
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub results: Vec<Value>,
    pub cache_time: i32,
    pub is_personal: bool,
}

impl Answer {
    /// 空应答：**必须回一次**，否则客户端一直转圈。
    ///
    /// 认不出的查询（比如只打了一个字）走这里：不缓存、只给发起人。
    fn empty() -> Self {
        Self {
            results: Vec::new(),
            cache_time: 0,
            is_personal: true,
        }
    }
}

/// 查询 → 应答（纯逻辑入口，handler 选择也在这里）。
pub fn answer(runtime: &Runtime, query: &InlineQuery) -> Answer {
    let raw = query.query.trim();
    let Some(handler) = pick(raw) else {
        return Answer::empty();
    };
    match handler {
        Handler::EpisodeVideo => {
            let Some(episode_id) = episode_id_of(raw) else {
                return Answer::empty();
            };
            let answer = answer_episode(runtime, episode_id);
            if !answer.results.is_empty() {
                return answer;
            }
            // 纯数字既可能是章节 id 也可能是番剧 id（TS 只查章节，白瞎一次查询）：
            // 这一集没有资源就按番剧 id 再找一次导航。
            let navigation = answer_navigation(runtime, raw);
            if !navigation.results.is_empty() {
                return navigation;
            }
            not_found(runtime, episode_id)
        }
        Handler::AnimeNavigation => answer_navigation(runtime, raw),
    }
}

/// 章节号和番剧号都没查到东西时的兜底卡片（TS 的「EP X 暂无资源」）。
fn not_found(runtime: &Runtime, episode_id: i64) -> Answer {
    let url = format!("https://bgm.tv/ep/{episode_id}");
    let api = Simplify::new(runtime.host);
    Answer {
        results: vec![article_result(
            &format!("ep_none_{episode_id}"),
            &url,
            &format!("EP {episode_id} 暂无资源"),
            "数据库没有该章节资源",
            None,
            text_content(api.formatted(&Text::markdown(&url))),
        )],
        cache_time: EMPTY_CACHE_SECONDS,
        is_personal: false,
    }
}

/// 番剧导航：一条汇总卡 + 每部番一张卡片（正文取导航正文首段）。
fn answer_navigation(runtime: &Runtime, raw: &str) -> Answer {
    let query = raw.trim();
    let mut animes = Vec::new();
    if query.chars().all(|ch| ch.is_ascii_digit()) {
        if let Ok(id) = query.parse::<i64>() {
            match runtime.store.anime_by_id(id) {
                Ok(Some(anime)) => animes.push(anime),
                Ok(None) => {}
                Err(error) => log!(&runtime.logger, Warn, "内联查番剧 {id} 失败：{error}"),
            }
        }
    } else {
        match runtime.store.search_anime(query) {
            Ok(rows) => animes = rows,
            Err(error) => log!(&runtime.logger, Warn, "内联搜番剧「{query}」失败：{error}"),
        }
    }
    animes.truncate(MAX_RESULTS.saturating_sub(1));
    if animes.is_empty() {
        return Answer::empty();
    }

    let api = Simplify::new(runtime.host);
    let render = |text: &str| api.formatted(&Text::markdown(text));

    let mut results = Vec::with_capacity(animes.len() + 1);
    results.push(article_result(
        &format!("nav_search_{query}_{}", animes.len()),
        animes
            .first()
            .and_then(|anime| nav_link(anime))
            .unwrap_or("https://bgm.tv"),
        &format!("🔍 搜索结果（找到 {} 个匹配项）", animes.len()),
        &format!("搜索关键词：{query}"),
        None,
        text_content(render(&search_summary(query, &animes))),
    ));

    for (index, anime) in animes.iter().enumerate() {
        let title = anime.display_name().to_owned();
        let description = format!("ID: {}", anime.id);
        let text = crate::nav::pages_for(runtime, anime)
            .ok()
            .and_then(|pages| pages.into_iter().next())
            .unwrap_or_else(|| {
                format!(
                    "导航消息: {}",
                    anime
                        .nav_message
                        .as_ref()
                        .and_then(|nav| nav.link.as_deref())
                        .unwrap_or("无链接")
                )
            });
        let subject_url = format!("https://bgm.tv/subject/{}", anime.id);
        results.push(article_result(
            &format!("nav_{}_{index}", anime.id),
            nav_link(anime).unwrap_or(&subject_url),
            &title,
            &description,
            anime.image.as_deref(),
            text_content(render(&text)),
        ));
    }

    Answer {
        results,
        cache_time: NAV_CACHE_SECONDS,
        is_personal: false,
    }
}

/// 导航消息的链接（空串按「没有」处理：`url` 是 `inputInlineQueryResultArticle`
/// 的必填字段，传空会被服务端退回来）。
fn nav_link(anime: &Anime) -> Option<&str> {
    anime
        .nav_message
        .as_ref()
        .and_then(|nav| nav.link.as_deref())
        .filter(|link| !link.trim().is_empty())
}

/// 汇总卡的正文（TS `buildSearchSummaryResult` 的 `summaryLines`）。
fn search_summary(query: &str, animes: &[Anime]) -> String {
    let mut lines = vec![format!("🔍 搜索结果（找到 {} 个匹配项）", animes.len())];
    if !query.is_empty() {
        lines.push(format!("搜索关键词：{query}"));
    }
    for anime in animes {
        lines.push(String::new());
        lines.push(format!("🎬 {}", anime.display_name()));
        if let Some(name) = anime.name.as_deref().filter(|name| !name.trim().is_empty()) {
            lines.push(name.to_owned());
        }
        if let Some(link) = anime
            .nav_message
            .as_ref()
            .and_then(|nav| nav.link.as_deref())
            .filter(|link| !link.trim().is_empty())
        {
            lines.push(format!("🔗 [查看详情]({link})"));
        }
    }
    lines.join("\n")
}

/// 按章节 id 取这一集的全部资源。
///
/// 一条资源一张卡片：有视频文件 id 就发视频，没有就把消息链接当文本发出去
/// （TS 也是这个降级）。视频 id 优先用库里记的 `videoid`，缺了才去 `getMessage` 读
/// ——老数据没填 `videoid`，但消息还在频道里。
fn answer_episode(runtime: &Runtime, episode_id: i64) -> Answer {
    let rows = match runtime.store.find(
        "resources",
        json!({ "episodeId": episode_id }),
        Some(json!({ "_id": 0 })),
        Some(json!({ "createdAt": -1 })),
        Some(MAX_RESULTS),
    ) {
        Ok(rows) => rows,
        Err(error) => {
            log!(&runtime.logger, Warn, "内联查章节 {episode_id} 的资源失败：{error}");
            return Answer::empty();
        }
    };

    let api = Simplify::new(runtime.host);
    let render = |text: &str| api.formatted(&Text::markdown(text));
    let td = TdExt::new(runtime.host);
    let fallback_url = format!("https://bgm.tv/ep/{episode_id}");

    let mut results = Vec::with_capacity(rows.len());
    for (index, row) in rows.into_iter().enumerate() {
        let Ok(resource) = serde_json::from_value::<EpisodeResourceDoc>(row) else {
            continue;
        };
        let link = resource
            .message
            .as_ref()
            .and_then(|message| message.link.clone())
            .filter(|link| !link.trim().is_empty());
        let url = link.clone().unwrap_or_else(|| fallback_url.clone());
        let title = if resource.title.trim().is_empty() {
            format!("EP {episode_id}")
        } else {
            resource.title.clone()
        };
        let groups: Vec<&str> = resource
            .groups
            .iter()
            .map(String::as_str)
            .filter(|group| !group.trim().is_empty())
            .collect();
        let description = if groups.is_empty() {
            format!("EP{episode_id}")
        } else {
            format!("EP{episode_id}｜{}", groups.join(" / "))
        };
        let thumbnail = anime_image(runtime, resource.anime_id);

        let video = resolve_video(&td, &resource);
        let content = match video {
            Some((video_id, size)) => {
                let caption = match link.as_deref() {
                    Some(link) => format!("[{title}]({link})"),
                    None => title.clone(),
                };
                video_content(&video_id, render(&caption), size)
            }
            None => text_content(render(link.as_deref().unwrap_or(&fallback_url))),
        };
        results.push(article_result(
            &format!("ep_{episode_id}_{index}"),
            &url,
            &title,
            &description,
            thumbnail.as_deref(),
            content,
        ));
    }

    if results.is_empty() {
        // 一条资源都没有：交给调用方决定是退回导航还是回「暂无资源」。
        return Answer::empty();
    }
    Answer {
        results,
        cache_time: EP_CACHE_SECONDS,
        is_personal: false,
    }
}

/// 这一集所属番剧的封面 URL（内联卡片的缩略图）。
///
/// 每条资源都查一次库有点浪费，但同一集通常只有一两个 `anime_id`，命中率极高；
/// 换成缓存反而要处理失效，不值得。
fn anime_image(runtime: &Runtime, anime_id: i64) -> Option<String> {
    runtime
        .store
        .anime_by_id(anime_id)
        .ok()
        .flatten()
        .and_then(|anime| anime.image)
        .filter(|image| !image.trim().is_empty())
}

/// 视频文件 id + 尺寸：先看库里记的，缺了再读消息。
fn resolve_video(td: &TdExt, resource: &EpisodeResourceDoc) -> Option<(String, VideoSize)> {
    let stored = resource
        .videoid
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    if let Some(video_id) = stored {
        // 尺寸只有消息里有；拿不到就交给客户端按 0 处理（能播，只是没有预热布局）。
        let size = resource
            .message
            .as_ref()
            .and_then(|message| td.get_message(message.chat_id, message.message_id).ok())
            .and_then(|message| video_from_message(&message).map(|(_, size)| size))
            .unwrap_or_default();
        return Some((video_id.to_owned(), size));
    }

    // 单条消息
    if let Some(message) = resource.message.as_ref()
        && message.chat_id != 0
        && message.message_id != 0
        && let Ok(raw) = td.get_message(message.chat_id, message.message_id)
        && let Some(found) = video_from_message(&raw)
    {
        return Some(found);
    }
    // 相册各段：优先用记下来的 videoid，没有再逐条读
    for part in resource.messages.iter().flatten() {
        if let Some(video_id) = part.videoid.as_deref().map(str::trim).filter(|id| !id.is_empty()) {
            return Some((video_id.to_owned(), VideoSize::default()));
        }
        if part.chat_id == 0 || part.message_id == 0 {
            continue;
        }
        if let Ok(raw) = td.get_message(part.chat_id, part.message_id)
            && let Some(found) = video_from_message(&raw)
        {
            return Some(found);
        }
    }
    None
}

/// 把应答发回 Telegram。失败只记日志——内联查询超时是常态，不该刷错误栈。
fn reply(runtime: &Runtime, query: &InlineQuery, answer: &Answer) {
    let td = TdExt::new(runtime.host);
    let result = td.answer_inline_query(
        query.id,
        Value::Array(answer.results.clone()),
        answer.cache_time,
        answer.is_personal,
        None,
    );
    if let Err(error) = result {
        log!(
            &runtime.logger,
            Warn,
            "应答内联查询失败（query {}，{} 条结果）：{error}",
            query.id,
            answer.results.len()
        );
    }
}

/// 订阅 `updateNewInlineQuery`，在**自己的线程**里查库 + 应答。
///
/// 为什么不在回调里直接干：内联查询里有一次 `searchAnime`、每条结果可能还有一次
/// `getMessage`，全等完再应答会把宿主的更新投递线程按住，紧接着的按键就排队了。
/// 每个查询开一个短线程，跑完就结束。
pub fn spawn(host: HostApi, runtime: Arc<Runtime>) -> Option<InlineSubscription> {
    let mut owned: Box<Box<dyn Fn(&InlineQuery) + Send + Sync>> =
        Box::new(Box::new(move |query: &InlineQuery| {
            let worker = Arc::clone(&runtime);
            // 线程要拿走一个 `Arc`，失败日志还得用另一个——`Arc` 不是 `Copy`。
            let logger = Arc::clone(&worker);
            let job = query.clone();
            let spawned = thread::Builder::new()
                .name("xq-anime-inline".to_owned())
                .spawn(move || {
                    let answer = answer(&worker, &job);
                    reply(&worker, &job, &answer);
                });
            if spawned.is_err() {
                log!(&logger.logger, Debug, "内联查询线程起不来，丢弃 query {}", query.id);
            }
        }));
    let user_data = (&raw mut *owned).cast::<c_void>();
    let handle = host.on(INLINE_EVENT, None, inline_trampoline, user_data);
    handle.is_valid().then_some(InlineSubscription {
        host,
        handle,
        _handler: owned,
    })
}

/// 一个活着的内联订阅：**drop 就是退订**（与 `CallbackSubscription` 同一套语义）。
pub struct InlineSubscription {
    host: HostApi,
    handle: SubscriptionHandle,
    /// 只负责**持有**回调；递出去的 `user_data` 指着这里的地址。
    _handler: Box<Box<dyn Fn(&InlineQuery) + Send + Sync>>,
}

impl Drop for InlineSubscription {
    fn drop(&mut self) {
        if self.handle.is_valid() {
            self.host.off(self.handle);
        }
    }
}

/// C 回调：把负载解成 [InlineQuery] 再交给闭包。解不出来就返回 0（不打断投递）。
unsafe extern "C-unwind" fn inline_trampoline(
    user_data: *mut c_void,
    payload: *const c_char,
) -> i32 {
    if user_data.is_null() || payload.is_null() {
        return -1;
    }
    // SAFETY: `user_data` 是 `spawn` 里那个 Box，订阅活多久它就活多久。
    let handler = unsafe { &*(user_data as *const Box<dyn Fn(&InlineQuery) + Send + Sync>) };
    // SAFETY: 宿主保证 `payload` 是 NUL 结尾的临时字符串。
    let text = unsafe { CStr::from_ptr(payload) }.to_string_lossy();
    let Ok(update) = serde_json::from_str::<Value>(&text) else {
        return 0;
    };
    if let Some(query) = query_of(&update) {
        handler(&query);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_of_reads_inline_update() {
        let update = json!({
            "@type": "updateNewInlineQuery",
            "id": 42,
            "sender_user_id": 7,
            "query": " 芙莉莲 ",
            "offset": "",
        });
        let query = query_of(&update).expect("应认出内联查询");
        assert_eq!(query.id, 42);
        assert_eq!(query.sender_user_id, 7);
        assert_eq!(query.query, " 芙莉莲 ");
    }

    #[test]
    fn query_of_ignores_other_updates() {
        assert!(query_of(&json!({"@type": "updateNewCallbackQuery", "id": 1})).is_none());
        assert!(query_of(&json!({"@type": "updateNewInlineQuery"})).is_none());
    }

    #[test]
    fn episode_id_from_digits_and_links() {
        assert_eq!(episode_id_of(" 12345 "), Some(12345));
        assert_eq!(episode_id_of("https://bgm.tv/ep/1001"), Some(1001));
        assert_eq!(episode_id_of("http://bangumi.tv/ep/1001/"), Some(1001));
        assert_eq!(episode_id_of("https://bgm.tv/ep/1001/comment"), Some(1001));
        // 不是 bgm 的链接、没有数字、路径粘着字母，都不算
        assert_eq!(episode_id_of("https://example.com/ep/1001"), None);
        assert_eq!(episode_id_of("https://bgm.tv/ep/"), None);
        assert_eq!(episode_id_of("https://bgm.tv/ep/1001abc"), None);
        assert_eq!(episode_id_of("bgm.tv/ep/1001"), None, "缺 scheme 不算");
        assert_eq!(episode_id_of(""), None);
    }

    #[test]
    fn pick_prefers_episode_for_numbers() {
        // TS 的分数：数字 → episodeVideo 100 > animeNavigation 90
        assert_eq!(pick("12345"), Some(Handler::EpisodeVideo));
        assert_eq!(pick("https://bgm.tv/ep/1"), Some(Handler::EpisodeVideo));
        // 文字 → 只有 animeNavigation 接，且至少两个字符
        assert_eq!(pick("芙莉莲"), Some(Handler::AnimeNavigation));
        assert_eq!(pick("芙"), None);
        assert_eq!(pick("   "), None);
    }

    #[test]
    fn article_result_carries_required_fields() {
        let result = article_result(
            "nav_1_0",
            "https://bgm.tv/subject/1",
            "标题",
            "ID: 1",
            Some("https://example.com/a.jpg"),
            text_content(json!({ "@type": "formattedText", "text": "hi", "entities": [] })),
        );
        assert_eq!(result["@type"], "inputInlineQueryResultArticle");
        assert_eq!(result["id"], "nav_1_0");
        assert_eq!(result["thumbnail_url"], "https://example.com/a.jpg");
        assert_eq!(result["thumbnail_width"], 0);
        assert_eq!(result["reply_markup"]["@type"], "replyMarkupInlineKeyboard");
        assert_eq!(result["reply_markup"]["rows"], json!([]));
        assert_eq!(
            result["input_message_content"]["@type"],
            "inputMessageText"
        );
        assert_eq!(
            result["input_message_content"]["text"]["text"],
            "hi"
        );
    }

    #[test]
    fn article_result_without_thumbnail_sends_empty_string() {
        let result = article_result("x", "https://bgm.tv", "t", "", None, text_content(json!({})));
        assert_eq!(result["thumbnail_url"], "");
    }

    #[test]
    fn clip_id_keeps_64_chars() {
        let long = "外".repeat(80);
        assert_eq!(clip_id(&long).chars().count(), 64);
        assert_eq!(clip_id("short"), "short");
    }

    #[test]
    fn video_content_uses_remote_file_id() {
        let content = video_content(
            "file-123",
            json!({ "@type": "formattedText", "text": "[第 1 集](https://t.me/c/1/2)", "entities": [] }),
            VideoSize {
                duration: 1440,
                width: 1920,
                height: 1080,
                spoiler: true,
            },
        );
        assert_eq!(content["@type"], "inputMessageVideo");
        assert_eq!(content["video"]["@type"], "inputVideo");
        assert_eq!(content["video"]["video"]["@type"], "inputFileRemote");
        assert_eq!(content["video"]["video"]["id"], "file-123");
        assert_eq!(content["video"]["duration"], 1440);
        assert_eq!(content["video"]["width"], 1920);
        assert_eq!(content["video"]["height"], 1080);
        assert_eq!(content["video"]["supports_streaming"], true);
        assert_eq!(content["has_spoiler"], true);
        assert_eq!(
            content["caption"]["text"],
            "[第 1 集](https://t.me/c/1/2)"
        );
    }

    #[test]
    fn video_from_message_reads_remote_id_and_size() {
        let message = json!({
            "id": 9,
            "content": {
                "@type": "messageVideo",
                "has_spoiler": true,
                "video": {
                    "duration": 24,
                    "width": 1280,
                    "height": 720,
                    "video": { "remote": { "id": "remote-9", "unique_id": "u9" } }
                }
            }
        });
        let (id, size) = video_from_message(&message).expect("应读出视频");
        assert_eq!(id, "remote-9");
        assert_eq!(size.duration, 24);
        assert_eq!(size.width, 1280);
        assert_eq!(size.height, 720);
        assert!(size.spoiler);
    }

    #[test]
    fn video_from_message_ignores_other_content() {
        assert!(video_from_message(&json!({"content": {"@type": "messageText"}})).is_none());
        assert!(video_from_message(&json!({})).is_none());
    }

    #[test]
    fn search_summary_lists_names_and_links() {
        let anime = Anime {
            id: 400602,
            name: Some("葬送のフリーレン".to_owned()),
            name_cn: Some("葬送的芙莉莲".to_owned()),
            nav_message: Some(anime_core::models::MessageRef {
                chat_id: -100,
                message_id: 5,
                thread_id: None,
                topic_id: None,
                link: Some("https://t.me/c/1/5".to_owned()),
            }),
            ..Default::default()
        };
        let text = search_summary("芙莉莲", &[anime]);
        assert!(text.contains("找到 1 个匹配项"));
        assert!(text.contains("搜索关键词：芙莉莲"));
        assert!(text.contains("🎬 葬送的芙莉莲"));
        assert!(text.contains("葬送のフリーレン"));
        assert!(text.contains("[查看详情](https://t.me/c/1/5)"));
    }
}
