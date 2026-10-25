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
    let message_id = runtime
        .td
        .send_photo_wait(target, path.display().to_string(), caption(subject))
        .ok_or_else(|| Error::Service {
            service: "td".to_owned(),
            message: "导航卡发送未确认（等不到 messageSendSucceeded）".to_owned(),
        })?;

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

/// 把一条已发送的视频记进 `navVideoMessage`（导航里的资源列表）。
///
/// `page` 是导航里的分页序号：TS 每页放固定条数，这里先按「第几条」自增，
/// 等导航编辑（editMessageText）落地后再按每页容量换算。
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
    let entry = json!({
        "page": page,
        "chat_id": chat_id,
        "message_id": message_id,
        "thread_id": runtime.config.telegram.anime_group_thread_id,
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
    fn caption_uses_cn_name_platform_and_episode_count() {
        assert_eq!(caption(&subject(28)), "葬送的芙莉莲｜TV｜全 28 集");
    }

    #[test]
    fn caption_skips_unknown_episode_count() {
        assert_eq!(caption(&subject(0)), "葬送的芙莉莲｜TV");
    }
}
