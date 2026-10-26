//! 管理群审核提示：TS `anime/adminPrompts.ts` 的四个提示。
//!
//! 四个提示对应四种「机器拿不准」的情形，管理员看到的是不同按钮：
//!
//! | 情形 | 函数 | 按钮 → 回调 |
//! |---|---|---|
//! | 搜不到番剧 | [`provide_anime_info`] | 点击提供 → `N_anime?c=` |
//! | 集数越界/缺 | [`confirm_episodes`] | 正确 → `Y_anime?id=&c=` ／ 提供正确 → `N_ep?c=&id=` |
//! | 日期不符 | [`confirm_episodes`] | 正确 → `Y_anime?id=&c=` |
//! | 匹配置信度可溯源 | [`confirm_with_candidates`] | 正确／错误 → `Y_anime?r=` ／ `F_anime?r=` |
//!
//! ## 为什么提示要分这么细
//!
//! 一个「对不对」按钮是不够的：搜不到番剧时管理员要**提供条目**，集数错了要**提供集数**，
//! 而置信度够的只是**确认**。把三件事塞进同一条消息，管理员就得先猜这次该干什么。
//!
//! ## 消息发到哪
//!
//! 管理群的**导航话题**（`nav_group_thread_id`）——与 TS 一致（它用的是
//! `NAV_GROUP_THREAD_ID` 而不是 `ANIME_GROUP_THREAD_ID`）。审核和导航是同一条线的
//! 两道工序，放在一个话题里翻记录方便。

use anime_core::ai::MatchResult;
use anime_core::matching::EpisodeMatch;
use anime_core::models::{Anime, AnimeItem};
use fuyubot_simplify::ReplyButton;

use crate::callback::{self, CallbackTarget};
use crate::runtime::Runtime;
use crate::td::Target;

/// 提示发到管理群的哪个话题。
fn admin_target(runtime: &Runtime) -> Target {
    Target::new(runtime.config.telegram.admin_group_id, 0)
        .with_topic(runtime.config.telegram.nav_group_thread_id)
}

/// 条目在 Bangumi 上的展示名（中文名优先，没有就用原名）。
/// 展示名：中文名优先（空串不算有），没有就用原名。
///
/// 公开是因为除了提示语，深链回复（`/start collection-`）也要显示番剧名——
/// 两处用同一套规则，免得一处改了另一处忘了。
pub fn display(anime: &Anime) -> &str {
    match anime.name_cn.as_deref() {
        Some(name) if !name.trim().is_empty() => name,
        _ => anime.name.as_deref().unwrap_or("(未命名)"),
    }
}

/// 番剧链接（bgm.tv 条目页）。
fn subject_link(anime: &Anime) -> String {
    format!("https://bgm.tv/subject/{}", anime.id)
}

/// 搜不到番剧信息：请管理员手动提供（`N_anime?c=<cacheId>`）。
///
/// 这是唯一一个「机器完全没答案」的提示——按钮不是确认，而是**进入手动绑定流程**：
/// 管理员点了之后回复消息给出正确的条目链接，bot 解析回复继续（见 callback 的状态机）。
pub fn provide_anime_info(runtime: &Runtime, cache_id: i64, item: &AnimeItem) -> bool {
    let text = format!(
        "番剧: {}\n集数: {}\n\n未搜索到的动漫信息\n请手动提供动漫信息",
        item.title,
        item.episode.as_deref().unwrap_or("未获取到")
    );
    runtime.td.send_text_with_buttons(
        admin_target(runtime),
        text,
        vec![vec![ReplyButton::callback(
            "点击提供",
            callback::data::provide_anime(cache_id),
        )]],
    )
}

/// 集数匹配出问题：越界、日期不符、解析不出（`Y_anime` / `N_ep`）。
///
/// 两种失败给不同按钮，这是本函数唯一的分支逻辑：
/// - **日期不符**（`DateMismatch`）：机器其实有答案（找到了章节 id），只是怀疑拿错季，
///   所以给「正确」——管理员确认即可，不用重填；
/// - **其他失败**：库里根本没有这个集数，管理员得**手动提供集数 id**，走 `N_ep`。
///
/// 已匹配（`Matched`）时不发消息：没有问题要问。
pub fn confirm_episodes(
    runtime: &Runtime,
    anime: &Anime,
    cache_id: i64,
    item: &AnimeItem,
    matched: &EpisodeMatch,
) -> bool {
    // 日期不符时机器有答案，用它；否则没有 episode_id 可带。
    let episode_id = match matched {
        EpisodeMatch::DateMismatch { episode_id, .. } => Some(*episode_id),
        _ => None,
    };

    let base = format!(
        "当前番剧为{}\n\n动漫信息：\n\n**名称：** [{}]({})\n**ID：** {}\n匹配集数: {}\n出现问题：{}\n\n",
        item.title,
        display(anime),
        subject_link(anime),
        anime.id,
        item.episode.as_deref().unwrap_or("未知"),
        matched.describe(),
    );

    let (text, buttons) = match episode_id {
        Some(episode_id) => (
            format!("{base}请确认是否正确"),
            vec![vec![ReplyButton::callback(
                "正确",
                callback::data::confirm(CallbackTarget::by_episode(episode_id, cache_id)),
            )]],
        ),
        None => (
            format!("{base}请提供正确的集数id"),
            vec![vec![ReplyButton::callback(
                "提供正确",
                callback::data::provide_episode(cache_id, anime.id),
            )]],
        ),
    };

    runtime
        .td
        .send_text_with_buttons(admin_target(runtime), text, buttons)
}

/// 确认自动搜索到的番剧是否正确（`Y_anime?id=&c=` / `F_anime?id=&c=`）。
///
/// 没有待审核记录时用它——按钮只能带 `episodeId` + `cacheId`，信息量比带 `r` 少。
/// 新流程一律走 [`confirm_with_candidates`]（带 `r`）。
///
/// 保留它只是为了和 TS 对齐（TS 的 `promptAdminConfirmAnime` 同样已无人调用，
/// 只剩文档引用）：旧按钮还在飞的时候，解析层认得出 `Y_anime?id=&c=` 这种数据，
/// 提示层就得有对应的那份文案。
#[allow(dead_code)]
pub fn confirm_anime(
    runtime: &Runtime,
    anime: &Anime,
    episode_id: i64,
    cache_id: i64,
    item: &AnimeItem,
) -> bool {
    let text = format!(
        "当前番剧为{}\n\n搜索到的动漫信息：\n\n**名称：** [{}]({})\n**ID：** {}\n剧集: [{}]({})\n\n请确认是否正确",
        item.title,
        display(anime),
        subject_link(anime),
        anime.id,
        episode_id,
        episode_link(episode_id),
    );
    let target = CallbackTarget::by_episode(episode_id, cache_id);
    runtime.td.send_text_with_buttons(
        admin_target(runtime),
        text,
        vec![vec![
            ReplyButton::callback("正确", callback::data::confirm(target)),
            ReplyButton::callback("错误", callback::data::reject(target)),
        ]],
    )
}

/// 带匹配详情的确认（可溯源纠正）：`Y_anime?r=` / `F_anime?r=`。
///
/// 与 [`confirm_anime`] 的差别不只是多了置信度文案：
/// - 按钮优先带**待审核记录 id**（`r`）——凭它能回查完整上下文（item/anime/已发消息），
///   管理员点「错误」时 bot 才知道要纠正的是哪一次匹配；
/// - `post_send` 区分「先发后审」和「待审核」：前者视频**已经**在正片频道了，
///   提示里要提醒管理员去频道看，而不是当成还没发。
pub fn confirm_with_candidates(
    runtime: &Runtime,
    anime: &Anime,
    episode_id: i64,
    cache_id: i64,
    item: &AnimeItem,
    detail: Option<&MatchResult>,
    review_id: Option<i64>,
    post_send: bool,
) -> bool {
    let target = match review_id {
        Some(id) => CallbackTarget::by_review(id),
        None => CallbackTarget::by_episode(episode_id, cache_id),
    };

    let mut text = format!(
        "{}当前番剧为{}\n\n匹配到的动漫信息：\n\n**名称：** [{}]({})\n**ID：** {}\n剧集: [{}]({})\n",
        if post_send {
            "🔄 **先发后审**\n\n"
        } else {
            "⏳ **待审核**\n\n"
        },
        item.title,
        display(anime),
        subject_link(anime),
        anime.id,
        episode_id,
        episode_link(episode_id),
    );
    if let Some(detail) = detail {
        text.push_str(&format!(
            "\n**匹配置信度：** {}%\n**匹配原因：** {}\n",
            (detail.confidence * 100.0).round(),
            detail.reason
        ));
    }
    text.push_str("\n请确认是否正确");

    runtime.td.send_text_with_buttons(
        admin_target(runtime),
        text,
        vec![vec![
            ReplyButton::callback("正确", callback::data::confirm(target)),
            ReplyButton::callback("错误", callback::data::reject(target)),
        ]],
    )
}

/// 章节链接（bgm.tv 单集页）。
fn episode_link(episode_id: i64) -> String {
    format!("https://bgm.tv/ep/{episode_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anime() -> Anime {
        Anime {
            id: 400602,
            name: Some("葬送のフリーレン".to_owned()),
            name_cn: Some("葬送的芙莉莲".to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn display_prefers_chinese_name() {
        assert_eq!(display(&anime()), "葬送的芙莉莲");
        let no_cn = Anime {
            name: Some("Sousou no Frieren".to_owned()),
            ..Default::default()
        };
        assert_eq!(display(&no_cn), "Sousou no Frieren");
        // 空串的中文名不算有，退回原名
        let blank_cn = Anime {
            name: Some("原名".to_owned()),
            name_cn: Some("  ".to_owned()),
            ..Default::default()
        };
        assert_eq!(display(&blank_cn), "原名");
    }

    #[test]
    fn subject_link_uses_bangumi_id() {
        assert_eq!(subject_link(&anime()), "https://bgm.tv/subject/400602");
    }

    #[test]
    fn provide_anime_info_button_routes_to_n_anime() {
        let data = callback::data::provide_anime(9);
        assert_eq!(data, "N_anime?c=9");
        assert_eq!(
            callback::parse_anime_callback(&data),
            Some(callback::AnimeCallback::ProvideAnime { cache_id: 9 })
        );
    }

    #[test]
    fn date_mismatch_offers_confirm_while_missing_episode_asks_for_input() {
        // 日期不符：机器有 episode_id，给「正确」
        let mismatch = EpisodeMatch::DateMismatch {
            message: "放送日期不符".to_owned(),
            diff_days: 30,
            episode_id: 1227091,
        };
        assert_eq!(mismatch.episode_id(), Some(1227091));

        // 库里没有：没有 episode_id，只能请管理员提供
        let missing = EpisodeMatch::NotFoundInDb {
            message: "第 13 集不在章节列表里".to_owned(),
        };
        assert_eq!(missing.episode_id(), None);
        assert!(!missing.is_matched());
    }

    #[test]
    fn confirm_with_review_id_uses_short_callback() {
        // 有待审核 id 时按钮只带 r，比 id+c 短——callback_data 有长度上限。
        let target = CallbackTarget::by_review(42);
        let data = callback::data::confirm(target);
        assert_eq!(data, "Y_anime?r=42");
        assert!(data.len() < "Y_anime?id=1227091&c=9".len());
    }

    #[test]
    fn post_send_prefix_differs_from_pending() {
        // 两个前缀只是文案差别，但管理员要一眼分清「已发出去了」和「还没发」。
        let post = format!("{}", "🔄 **先发后审**");
        let pending = format!("{}", "⏳ **待审核**");
        assert_ne!(post, pending);
    }
}
