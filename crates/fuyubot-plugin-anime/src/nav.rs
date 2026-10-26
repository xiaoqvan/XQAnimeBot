//! 导航消息：频道里那条「番剧总览」。
//!
//! TS 的结构是：番剧卡发到导航频道 → 记住 `navMessage{chat_id,message_id,thread_id}`
//! 写回 `anime` 文档；之后每发一集，把视频消息追加进 `navVideoMessage`，再用
//! `editMessageText` 刷新导航里的资源列表。
//!
//! 这里先做前一半：**发卡 + 回写 navMessage**。后续追加资源、编辑文本都基于这条记录，
//! 没有它连「导航是哪条消息」都不知道。
//!
//! 发消息要拿 message_id，必须等 `messageSendSucceeded`（阻塞），所以只能在后台线程调。

use anime_core::bangumi::{Episode, Subject};
use anime_core::card::bangumi::BangumiCard;
use anime_core::log;
use anime_core::store::Storage;
use anime_core::{Error, Result};
use serde_json::json;

use crate::runtime::Runtime;
use crate::td::Target;

/// 已存在的导航消息定位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavRef {
    pub chat_id: i64,
    pub message_id: i64,
}

/// 导航卡上的文案。TS 用 Markdown 排一堆信息，这里先保持一行能读。
pub fn caption(subject: &Subject) -> String {
    let mut text = format!("{}｜{}", subject.display_name(), subject.platform);
    if subject.eps > 0 {
        text.push_str(&format!("｜全 {} 集", subject.eps));
    }
    text
}

/// 发一张导航卡并把 `navMessage` 写回库。
///
/// 重复调用会再发一条新消息并覆盖记录（旧消息留在频道里）——所以调用方应该先查
/// [crate::store::Storage::anime_by_id] 看有没有 `nav_message`，这里不替它做幂等。
pub fn publish(runtime: &Runtime, subject: &Subject, episodes: &[Episode]) -> Result<NavRef> {
    let chat_id = runtime.config.telegram.nav_channel;
    if chat_id == 0 {
        return Err(Error::Config(
            "未配置 telegram.nav_channel，导航消息发不出去".to_owned(),
        ));
    }
    let thread_id = runtime.config.telegram.nav_group_thread_id;
    let card = crate::card::with_emoji(BangumiCard::from_subject(subject, episodes));
    let path = crate::card::render(runtime, &card)?;
    let target = Target::new(chat_id, 0).with_topic(thread_id);
    let message_id = if runtime.config.telegram.wait_send {
        runtime
            .td
            .send_photo_wait(target, path.display().to_string(), caption(subject))
            .ok_or_else(|| Error::Service {
                service: "td".to_owned(),
                message: "导航卡发送未确认（等不到 messageSendSucceeded）".to_owned(),
            })?
    } else {
        // 不等确认就拿不到 message_id：导航卡发出去了，但记不了账。
        let _ = runtime
            .td
            .send_photo_now(target, path.display().to_string(), caption(subject));
        0
    };

    let nav = json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "thread_id": thread_id,
    });
    runtime.store.update(
        "anime",
        json!({ "id": subject.id }),
        json!({ "$set": { "navMessage": nav, "updatedAt": chrono::Utc::now() } }),
        false,
        false,
    )?;
    Ok(NavRef {
        chat_id,
        message_id,
    })
}

/// 把一条导航分页消息记进 navVideoMessage（导航里的资源列表）。
///
/// `page` 是导航里的分页序号（指向导航频道里的文本消息）。**注意**：集数资源
/// （哪部番第几集、哪个字幕组）进 `resources` 集合，不在这里——`navVideoMessage`
/// 只存「导航频道里的分页文本消息」定位，字段与 TS 的 `navMessageType` 一致，
/// 没有 `episode`。视频资源写 `resources` 在阶段 4 接。
pub fn push_episode(
    runtime: &Runtime,
    anime_id: i64,
    chat_id: i64,
    message_id: i64,
) -> Result<()> {
    let anime = runtime.store.anime_by_id(anime_id)?;
    let page = anime
        .as_ref()
        .map(|anime| anime.nav_video_message.len())
        .unwrap_or(0) as i64
        + 1;
    let link = message_link(runtime, chat_id, message_id);
    let entry = json!({
        "page": page,
        "chat_id": chat_id,
        "message_id": message_id,
        "thread_id": runtime.config.telegram.anime_group_thread_id,
        "link": link,
    });
    runtime.store.update(
        "anime",
        json!({ "id": anime_id }),
        json!({
            "$push": { "navVideoMessage": entry },
            "$set": { "updatedAt": chrono::Utc::now() },
        }),
        false,
        false,
    )?;
    // 记完就刷新导航标题：失败只记日志（refresh_caption 内部已经降级）。
    let _ = refresh_caption(runtime, anime_id);
    Ok(())
}

/// 取消息链接。失败返回 None——没有链接只是少个跳转，不影响收录。
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

/// 导航卡文案：名字｜已收录 N 集。
///
/// 完整正文（`navmegtext`，含每集资源、按字幕组分组、页码导航）在阶段 4 接：
/// 它从 `resources` 集合读集数资源，而不是从 `navVideoMessage`（那里只存分页消息）。
pub fn caption_for(anime: &anime_core::models::Anime) -> String {
    format!(
        "{}｜已收录 {} 集",
        anime.display_name(),
        anime.nav_video_message.len()
    )
}

/// 刷新导航卡的标题（已收录集数）。
///
/// 没有导航消息就什么也不做——没配导航频道的机器上也应该能跑完整条链。
pub fn refresh_caption(runtime: &Runtime, anime_id: i64) -> Result<()> {
    let Some(anime) = runtime.store.anime_by_id(anime_id)? else {
        return Ok(());
    };
    let Some(nav) = anime.nav_message.as_ref() else {
        return Ok(());
    };
    let text = caption_for(&anime);
    runtime.td.edit_caption(nav.chat_id, nav.message_id, text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(eps: u32) -> Subject {
        Subject {
            id: 400602,
            name: "葬送のフリーレン".to_owned(),
            name_cn: "葬送的芙莉莲".to_owned(),
            platform: "TV".to_owned(),
            eps,
            ..Default::default()
        }
    }

    #[test]
    fn caption_counts_nav_video_messages() {
        use anime_core::models::{Anime, NavMessage};
        let anime = Anime {
            id: 1,
            name_cn: Some("某番".to_owned()),
            nav_message: Some(Default::default()),
            nav_video_message: vec![
                NavMessage { page: 1, link: Some("https://t.me/c/1/10".to_owned()), ..Default::default() },
                NavMessage { page: 2, ..Default::default() },
            ],
            ..Default::default()
        };
        let text = caption_for(&anime);
        assert_eq!(text, "某番｜已收录 2 集", "{text}");
    }

    #[test]
    fn caption_uses_cn_name_platform_and_episode_count() {
        assert_eq!(caption(&subject(28)), "葬送的芙莉莲｜TV｜全 28 集");
    }

    #[test]
    fn caption_skips_unknown_episode_count() {
        assert_eq!(caption(&subject(0)), "葬送的芙莉莲｜TV");
    }
}
