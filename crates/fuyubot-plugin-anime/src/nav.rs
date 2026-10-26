//! 导航消息：频道里那条「番剧总览」。
//!
//! TS 的结构是：番剧卡发到导航频道 → 记住 `navMessage{chat_id,message_id,thread_id}`
//! 写回 `anime` 文档；之后每发一集，重新生成正文（`navmegtext`）→ 改首条图注 →
//! 把装不下的资源区作为**分页文本消息**发在首条下面，并把分页消息记进
//! `navVideoMessage`（`page` = 第几页，正文里的 `[第N页](link)` 就指向它）。
//!
//! 这一版补齐了正文本身（[`text::navmegtext`]）和分页消息的维护：
//!
//! ```text
//! refresh(anime_id) ──► 重新生成正文 ──► 改首条 caption（1024 上限）
//!                                    └──► 逐页：已有就 editMessageText，没有就新发一条
//! ```
//!
//! 发消息要拿 message_id，必须等 `messageSendSucceeded`（阻塞），所以只能在后台线程调。

use anime_core::bangumi::{Episode, Subject};
use anime_core::card::bangumi::BangumiCard;
use anime_core::log;
use anime_core::models::{Anime, EpisodeResourceDoc};
use anime_core::store::Storage;
use anime_core::{Error, Result};
use serde_json::json;

use crate::runtime::Runtime;
use crate::td::Target;
use crate::text::{self, GroupedResources, ResourceEntry};

/// 已存在的导航消息定位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavRef {
    pub chat_id: i64,
    pub message_id: i64,
}

/// 导航卡上的文案（**只在没有 `navmegtext` 时用**——例如 `/addanime` 建档那一次
/// 还没生成正文，先用一句话占住位置）。
pub fn caption(subject: &Subject) -> String {
    let mut text = format!("{}｜{}", subject.display_name(), subject.platform);
    if subject.eps > 0 {
        text.push_str(&format!("｜全 {} 集", subject.eps));
    }
    text
}

/// 读一部番剧的资源，按字幕组分组（TS 的 `getResourcesByAnimeId`）。
///
/// 分组键取 `groups[0]`（TS 用整个 groups 数组的第一个元素作键）；没有字幕组的
/// 资源归到「未知」——宁可显示一个不好看的组名，也别把资源丢掉。
pub fn grouped_resources(runtime: &Runtime, anime_id: i64) -> Result<GroupedResources> {
    let rows = runtime.store.find(
        "resources",
        json!({ "anime_id": anime_id }),
        None,
        Some(json!({ "episode": 1 })),
        None,
    )?;
    let mut grouped = GroupedResources::new();
    for row in rows {
        let Ok(resource) = serde_json::from_value::<EpisodeResourceDoc>(row) else {
            continue;
        };
        let group = resource
            .groups
            .first()
            .cloned()
            .filter(|group| !group.trim().is_empty())
            .unwrap_or_else(|| "未知".to_owned());
        grouped.entry(group).or_default().push(ResourceEntry {
            episode: resource.episode,
            link: resource.message.as_ref().and_then(|message| message.link.clone()),
        });
    }
    Ok(grouped)
}

/// 生成这一部番的导航正文：首条 + 若干分页。
///
/// 分页链接从 `navVideoMessage` 里取（`page` → `link`）；还没发过分页消息时
/// 页码就是纯文字 `[第N页]`，发过之后下次刷新才有链接——与 TS 一致。
pub fn pages_for(runtime: &Runtime, anime: &Anime) -> Result<Vec<String>> {
    let grouped = grouped_resources(runtime, anime.id)?;
    let sections = text::format_grouped_resources(&grouped);
    if sections.is_empty() {
        return Ok(text::navmegtext(anime, &[], &[]));
    }
    let page_count = text::resource_pages(anime, &sections).len();
    let page_links: Vec<String> = (0..page_count)
        .map(|index| {
            let page = index as i64 + 1;
            match anime
                .nav_video_message
                .iter()
                .find(|entry| entry.page == page)
                .and_then(|entry| entry.link.as_deref())
                .filter(|link| !link.trim().is_empty())
            {
                Some(link) => format!("[第{page}页]({link})"),
                None => format!("第{page}页"),
            }
        })
        .collect();
    Ok(text::navmegtext(anime, &sections, &page_links))
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
    // 建档这一次还没有资源，正文就是「主信息 + 空资源区」——首次刷新（发第一集）
    // 会把它换成完整正文。
    let anime = subject.to_anime();
    let text = pages_for(runtime, &anime)
        .unwrap_or_else(|error| {
            log!(&runtime.logger, Warn, "生成导航正文失败，用简短文案：{error}");
            vec![caption(subject)]
        })
        .remove(0);
    let target = Target::new(chat_id, 0).with_topic(thread_id);
    let message_id = if runtime.config.telegram.wait_send {
        runtime
            .td
            .send_photo_wait(target, path.display().to_string(), &text)
            .ok_or_else(|| Error::Service {
                service: "td".to_owned(),
                message: "导航卡发送未确认（等不到 messageSendSucceeded）".to_owned(),
            })?
    } else {
        // 不等确认就拿不到 message_id：导航卡发出去了，但记不了账。
        let _ = runtime
            .td
            .send_photo_now(target, path.display().to_string(), &text);
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

/// 刷新导航：重新生成正文，改首条图注，补发/改写分页消息。
///
/// 这是每发一集都要走的一步（TS 的 `sendMegToNavAnime`）。两件容易做错的事：
///
/// - **首条是图片**，只能改 caption（1024 上限）——正文的首段本来就是按这个上限
///   生成的，所以改 caption 是对的；分页消息是纯文本（4096 上限），走
///   `editMessageText`。
/// - 分页消息**复用**而不是重发：按 `page` 在 `navVideoMessage` 里找，找到就改，
///   找不到才新发一条并记进去。重发会让频道里堆满同名旧页。
pub fn refresh(runtime: &Runtime, anime_id: i64) -> Result<()> {
    let Some(anime) = runtime.store.anime_by_id(anime_id)? else {
        return Ok(());
    };
    let Some(nav) = anime.nav_message.clone() else {
        // 还没发过导航（建档时没配导航频道，或还没发卡）：这一步只能跳过。
        return Ok(());
    };
    let pages = pages_for(runtime, &anime)?;
    let Some(main) = pages.first() else {
        return Ok(());
    };
    runtime.td.edit_caption(nav.chat_id, nav.message_id, main);

    let thread_id = runtime.config.telegram.nav_group_thread_id;
    for (index, page) in pages.iter().enumerate().skip(1) {
        let number = index as i64;
        let existing = anime
            .nav_video_message
            .iter()
            .find(|entry| entry.page == number);
        match existing {
            Some(entry) => {
                runtime.td.edit_text(entry.chat_id, entry.message_id, page);
            }
            None => {
                let target = Target::new(nav.chat_id, nav.message_id).with_topic(thread_id);
                let Some(message_id) = runtime.td.send_text_wait(target, page) else {
                    log!(&runtime.logger, Warn, "分页消息未确认，跳过（第 {number} 页）");
                    continue;
                };
                let link = message_link(runtime, nav.chat_id, message_id);
                let entry = json!({
                    "page": number,
                    "chat_id": nav.chat_id,
                    "message_id": message_id,
                    "thread_id": thread_id,
                    "link": link,
                });
                if let Err(error) = runtime.store.update(
                    "anime",
                    json!({ "id": anime_id }),
                    json!({
                        "$push": { "navVideoMessage": entry },
                        "$set": { "updatedAt": chrono::Utc::now() },
                    }),
                    false,
                    false,
                ) {
                    log!(&runtime.logger, Warn, "记录分页消息失败（第 {number} 页）：{error}");
                }
            }
        }
    }
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

    #[test]
    fn page_numbering_starts_at_one_for_the_first_page_message() {
        // 首条是图片（page 不占号），分页从 1 开始——正文里的 [第1页] 要能对上。
        let anime = Anime {
            id: 1,
            nav_video_message: vec![anime_core::models::NavMessage {
                page: 1,
                chat_id: -100,
                message_id: 10,
                ..Default::default()
            }],
            ..Default::default()
        };
        let found = anime
            .nav_video_message
            .iter()
            .find(|entry| entry.page == 1)
            .expect("第 1 页应当能查到");
        assert_eq!(found.message_id, 10);
    }
}
