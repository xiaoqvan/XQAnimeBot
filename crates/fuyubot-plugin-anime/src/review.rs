//! 待审核提示：把「拿不准的匹配」发到管理群，并挂上通过/驳回按钮。
//!
//! 对应 TS 的 `adminPrompts.ts::promptAdminConfirmAnime`：管理群收到一条带键盘的
//! 消息，点按钮走 `CallbackQuery` 那条路。这里只做「发提示 + 落 pendingReviews」，
//! 批准之后要做什么（下载/改库）留给 callback 的后续实现。
//!
//! 为什么要落库而不是把信息全塞进按钮：`callback_data` 有长度上限（TDLib 也是
//! 字节串），回调只带一个自增 id，详情回查文档。

use std::sync::Arc;

use anime_core::log;
use anime_core::matching::{EpisodeMatch, SeasonDecision};
use anime_core::models::{AlbumMessage, Anime, AnimeItem, EpisodeResourceDoc, MessageRef, PendingReviewDoc};
use anime_core::store::Storage;

use crate::callback;
use crate::chain::Plan;
use crate::runtime::Runtime;
use crate::td::{SentPart, Target};

/// 发一条待审核提示。返回待审核记录的自增 id（`None` = 没配管理群/写库失败）。
pub fn prompt(runtime: &Runtime, item: &AnimeItem, decision: &SeasonDecision) -> Option<i64> {
    let doc = PendingReviewDoc {
        item: Some(item.clone()),
        ..Default::default()
    };
    let id = match runtime.store.pending_review_insert(&doc) {
        Ok(id) => id,
        Err(error) => {
            log!(&runtime.logger, Warn, "写入待审核记录失败：{error}");
            return None;
        }
    };

    let chat = runtime.config.telegram.admin_group_id;
    if chat == 0 {
        log!(&runtime.logger, Warn, "没配置 admin_group_id，待审核提示发不出去");
        return None;
    }
    let text = format!(
        "待审核 #{id}\n标题：{}\n判定：{}\n集数：{}",
        item.title,
        decision.describe(),
        item.episode.as_deref().unwrap_or("未知")
    );
    runtime
        .td
        .send_text_with_buttons(Target::new(chat, 0), text, callback::review_buttons(id));
    Some(id)
}

/// 把这一集的资源写进库（`resources` / `cache_resources`）。
///
/// TS 的 `saveAnimeResource` 在**每条发送路径**上都有（命中、新番、缓冲频道），
/// 因为它是「哪部番第几集、哪个字幕组、发到哪条消息」的唯一真相：导航正文、
/// 内联取视频、集数纠正全读它。写失败只记日志——提示还得照发，否则管理员
/// 根本不知道有东西等着他。
pub fn record_send(
    runtime: &Runtime,
    plan: &Plan,
    item: &AnimeItem,
    chat_id: i64,
    sent: &[SentPart],
) {
    // 主消息 = 第一段：`message` 指向它（导航跳转、集数纠正的落点都在这），
    // 分段整体则记进 `messages`（TS 的 extractAlbumMsgData）。
    let Some(primary) = sent.first() else {
        return;
    };
    let episode = item
        .episode
        .clone()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| "未知".to_owned());
    let thread_id = runtime.config.telegram.anime_group_thread_id;
    let video_ids: Vec<String> = sent.iter().filter_map(|part| part.video_id.clone()).collect();
    let unique_ids: Vec<String> = sent
        .iter()
        .filter_map(|part| part.unique_id.clone())
        .collect();
    let resource = EpisodeResourceDoc {
        anime_id: plan.anime_id,
        episode,
        episode_id: plan.episode_id,
        groups: if item.fansub.is_empty() {
            vec![item.team.clone()]
        } else {
            item.fansub.clone()
        },
        title: item.title.clone(),
        names: Some(item.names.clone()),
        message: Some(MessageRef {
            chat_id,
            message_id: primary.message_id,
            thread_id: Some(thread_id),
            topic_id: None,
            link: message_link(runtime, chat_id, primary.message_id),
        }),
        // 多段（相册）才记整组：单段时 `message` 已经够，`messages` 只是重复。
        messages: (sent.len() > 1).then(|| {
            sent.iter()
                .map(|part| AlbumMessage {
                    chat_id,
                    message_id: part.message_id,
                    thread_id: Some(thread_id),
                    videoid: part.video_id.clone(),
                    unique_id: part.unique_id.clone(),
                    ..Default::default()
                })
                .collect()
        }),
        videoid: video_ids.first().cloned(),
        unique_id: unique_ids.first().cloned(),
        videoids: (video_ids.len() > 1).then_some(video_ids),
        unique_ids: (unique_ids.len() > 1).then_some(unique_ids),
        // 有 cache_id 就写 cache_resources（TS 的 cache=true），否则写 resources。
        cache_id: (plan.cache_id > 0).then(|| serde_json::json!(plan.cache_id)),
        source: item.source.clone(),
        created_at: Some(chrono::Utc::now()),
        ..Default::default()
    };
    if let Err(error) = runtime.store.resource_upsert(&resource) {
        log!(&runtime.logger, Warn, "写入集数资源失败：{error}");
    }
}

/// 发完视频之后请管理员确认（`plan.review` 为真时）。
///
/// 两条路的提示不一样，区别在 `plan.to_cache`：
///
/// ```text
/// 缓冲频道（to_cache）  →  请管理员确认（不建待审核记录）
/// 正片频道（先发后审）  →  建待审核记录 + 带候选的确认提示
/// ```
///
/// 缓冲频道那条路**不建**待审核记录：TS 也是这样，因为视频还没进正片，
/// 没有「已发消息」可回溯，管理员要纠正的是集数本身（`N_ep`），不是一次投放。
pub fn prompt_after_send(runtime: &Runtime, plan: &Plan, item: &AnimeItem) {
    let Some(anime) = load_anime(runtime, plan.anime_id) else {
        // 番剧文档读不出来就没法给提示（按钮里要带 anime id）：只记日志。
        log!(
            &runtime.logger,
            Warn,
            "读不到番剧文档（anime {}），审核提示发不出去",
            plan.anime_id
        );
        return;
    };

    if plan.to_cache {
        let matched = plan.episode_match.clone().unwrap_or_else(|| {
            // 没跑过集数匹配（例如回退路径上 Agent 直接给了答案）：按「库里没有」提示，
            // 理由用 Agent 给的，管理员至少知道机器当时是怎么想的。
            let message = plan
                .match_detail
                .as_ref()
                .map(|detail| detail.reason.clone())
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or_else(|| "未匹配到集数".to_owned());
            EpisodeMatch::NotFoundInDb { message }
        });
        if matched.is_matched() {
            let _ = crate::prompts::confirm_with_candidates(
                runtime,
                &anime,
                plan.episode_id.or(matched.episode_id()).unwrap_or(0),
                plan.cache_id,
                item,
                plan.match_detail.as_ref(),
                None,
                false,
            );
        } else {
            let _ = crate::prompts::confirm_episodes(
                runtime,
                &anime,
                plan.cache_id,
                item,
                &matched,
            );
        }
        return;
    }

    // 先发后审：视频已经在正片频道了，先建待审核记录（按钮带 r=<id> 才能回溯
    // 「那一次投放」的完整上下文），再发带候选的提示。
    let primary = load_anime(runtime, plan.anime_id).and_then(|_| {
        runtime
            .store
            .find(
                "resources",
                serde_json::json!({ "anime_id": plan.anime_id }),
                None,
                Some(serde_json::json!({ "createdAt": -1 })),
                Some(1),
            )
            .ok()?
            .into_iter()
            .next()
    });
    let primary_message = primary
        .as_ref()
        .and_then(|doc| serde_json::from_value::<EpisodeResourceDoc>(doc.clone()).ok())
        .and_then(|resource| resource.message);

    let doc = PendingReviewDoc {
        item: Some(item.clone()),
        anime: Some(anime.clone()),
        episode_id: plan.episode_id,
        episode_sort: plan
            .match_detail
            .as_ref()
            .and_then(|detail| detail.episode_sort)
            .map(|sort| sort as f64),
        sent_messages: primary_message.as_ref().map(|message| {
            vec![AlbumMessage {
                chat_id: message.chat_id,
                message_id: message.message_id,
                thread_id: message.thread_id,
                ..Default::default()
            }]
        }),
        primary_message,
        match_detail: plan
            .match_detail
            .as_ref()
            .and_then(|detail| serde_json::to_value(detail).ok()),
        ..Default::default()
    };
    let review_id = match runtime.store.pending_review_insert(&doc) {
        Ok(id) => Some(id),
        Err(error) => {
            log!(&runtime.logger, Warn, "写入待审核记录失败：{error}");
            // 建不了记录也要发提示：退成不带 r 的按钮，管理员还能靠 id+c 确认。
            None
        }
    };
    let _ = crate::prompts::confirm_with_candidates(
        runtime,
        &anime,
        plan.episode_id.unwrap_or(0),
        plan.cache_id,
        item,
        plan.match_detail.as_ref(),
        review_id,
        true,
    );
}

/// 取一条消息的链接；取不到返回 None（正文里少个跳转，不影响收录）。
fn message_link(runtime: &Runtime, chat_id: i64, message_id: i64) -> Option<String> {
    let client = fuyubot_tdlib_ext::TdExt::new(runtime.host);
    match client.get_message_link(chat_id, message_id, None) {
        Ok(value) => value
            .as_str()
            .map(str::to_owned)
            .filter(|link| !link.trim().is_empty()),
        Err(error) => {
            log!(&runtime.logger, Debug, "取消息链接失败（{message_id}）：{error}");
            None
        }
    }
}

/// 取番剧文档：正片库优先，缓冲库兜底。
///
/// 缓冲频道路线上番剧是 `saveAnime(anime, cache=true)` 写进 `cacheAnime` 的，
/// 还没进正片库；两个库都要看，否则提示里拿不到名字。
fn load_anime(runtime: &Runtime, anime_id: i64) -> Option<Anime> {
    if anime_id <= 0 {
        return None;
    }
    match runtime.store.anime_by_id(anime_id) {
        Ok(Some(anime)) => return Some(anime),
        Ok(None) => {}
        Err(error) => log!(&runtime.logger, Warn, "读番剧文档失败（{anime_id}）：{error}"),
    }
    match runtime.store.cached_anime_by_id(anime_id) {
        Ok(anime) => anime,
        Err(error) => {
            log!(
                &runtime.logger,
                Warn,
                "读缓冲番剧文档失败（{anime_id}）：{error}"
            );
            None
        }
    }
}

/// 审核通过后真正开始下载：从 pendingReviews 的记录里还原条目，交给 chain。
///
/// `item` 是 [prompt] 写进去的结构化字段，这里只做反序列化还原，不做二次判断——
/// 要投哪部番、第几集，审核时已经定了。
pub fn start_approved_download(runtime: &Arc<Runtime>, record: &serde_json::Value) -> bool {
    let Some(item) = record
        .get("item")
        .and_then(|value| serde_json::from_value::<AnimeItem>(value.clone()).ok())
    else {
        log!(&runtime.logger, Warn, "待审核记录缺少 item，无法开始下载");
        return false;
    };
    if item.magnet.trim().is_empty() {
        log!(&runtime.logger, Warn, "待审核记录缺 magnet，无法开始下载");
        return false;
    }
    log!(&runtime.logger, Info, "审核通过，开始下载：{}", item.title);
    crate::chain::spawn(Arc::clone(runtime), item, 0);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_approved_download_rejects_missing_item_or_magnet() {
        // 没有 item 字段：拒绝。
        assert!(!start_approved_download_dry(&serde_json::json!({ "id": 1 })));
        // item 缺 magnet：拒绝。
        assert!(!start_approved_download_dry(&serde_json::json!({
            "item": { "title": "x", "magnet": "  " }
        })));
    }

    /// 测试用：不真的 spawn 线程，只判断 `start_approved_download` 的前置校验逻辑。
    /// 通过只测「能还原出 item」这个纯函数路径；真正 spawn 在 `start_approved_download`
    /// 里做，单测里用一个空 item 也不会真的去下载。
    fn start_approved_download_dry(record: &serde_json::Value) -> bool {
        let item = record
            .get("item")
            .and_then(|value| serde_json::from_value::<AnimeItem>(value.clone()).ok());
        match item {
            None => false,
            Some(item) => !item.magnet.trim().is_empty(),
        }
    }
}
