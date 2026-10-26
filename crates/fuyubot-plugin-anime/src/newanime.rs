//! 新番链路：库里没有这部番时，一条 RSS 怎么走到「发出去 + 有人确认」。
//!
//! 对应 TS 的 `animeHandlers.ts::handleNewAnime`。它做两件以前完全没做的事：
//!
//! 1. **先用 LLM Agent 匹配番剧**（[`BangumiAgent`]），而不是「搜不到就完」；
//! 2. **按置信度分两条路**：
//!
//! ```text
//! 置信度 ≥ 0.9（先发后审）  建档 → 匹配章节 → 下载 → 发正片频道 → 建待审核 → 带候选提示
//! 置信度 < 0.9（回退）      搜索 → 建档(cache) → 下载 → 发缓冲频道 → 请管理员确认
//! ```
//!
//! ## 为什么先落 cacheItem 再匹配
//!
//! 匹配（尤其是 LLM）要几十秒，期间同一条种子可能又被下一轮 RSS 投递一次。
//! 先写 `cacheItem` + `torrents` 两条记录，后到的那次会被
//! [`crate::fetch`] 的去重挡在更前面；即便漏进来，审核提示里的 `c=<cacheId>`
//! 也能把两次合并到同一个纠正入口上（TS 同样是先 `addCacheItem` 再匹配）。
//!
//! ## 为什么回退路径还要发到缓冲频道
//!
//! 不发的话，低置信度的新番就只是「日志里多一行」，管理员连纠正的机会都没有——
//! 而这类条目恰恰是最需要人看的（新番、译名怪、分季乱）。缓冲频道是「发出了但
//! 还没确认」的暂存区，确认后才转进正片。
//!
//! ## 线程模型
//!
//! [spawn] 起自己的线程：Agent 要联网、Bangumi 要联网，都不能堵住轮询线程。
//! 真正的下载交给 [`crate::chain`]（那里有并发槽位），本模块只在**下载之前**
//! 把「发到哪、发完干什么」定下来。

use std::sync::Arc;
use std::thread;

use anime_core::ai::{BangumiAgent, MatchInput, MatchResult};
use anime_core::bangumi::{BgmClient, BgmTools};
use anime_core::log;
use anime_core::matching::{AiEpisode, AiredWindow, EpisodeMatch, match_episode, today_local};
use anime_core::models::{Anime, AnimeItem, TorrentStatus};
use anime_core::store::Storage;
use anime_core::{Error, Result};

use crate::chain::{self, Plan};
use crate::runtime::Runtime;

/// LLM 触发阈值：候选超过这么多个时才让模型介入（TS 传 5）。
///
/// 少于等于它的候选靠规则就能定（唯一候选 / 集数范围命中），多调一次模型只是
/// 多花几十秒和一点钱。
const LLM_THRESHOLD: usize = 5;

/// 接手一条新番候选。异步跑，失败只记日志——一条新番处理失败不该影响轮询。
pub fn spawn(runtime: Arc<Runtime>, item: AnimeItem) {
    let worker = Arc::clone(&runtime);
    let spawned = thread::Builder::new()
        .name("xq-anime-new".to_owned())
        .spawn(move || {
            if let Err(error) = handle(&worker, &item) {
                log!(&worker.logger, Warn, "新番处理失败（{}）：{error}", item.title);
            }
        });
    if let Err(error) = spawned {
        log!(&runtime.logger, Error, "新番线程起不来：{error}");
    }
}

/// 主流程：落缓存 → Agent 匹配 → 按置信度分路。
///
/// 收 `&Arc<Runtime>` 而不是 `&Runtime`：分路之后要把它整个搬进 chain 的线程，
/// 调用方（轮询线程）手上本来就持有 `Arc`，直接借一下比克隆一份字段干净。
fn handle(runtime: &Arc<Runtime>, item: &AnimeItem) -> Result<()> {
    // 先记账：并发下重复投递靠这两条记录收敛，审核提示也靠 cache_id 定位条目。
    let cache_id = runtime.store.cache_item_add(item)?;
    if let Err(error) =
        runtime
            .store
            .add_torrent(&item.magnet, TorrentStatus::PendingDownload, &item.title)
    {
        log!(&runtime.logger, Warn, "写入种子记录失败（继续）：{error}");
    }

    let detail = agent_match(runtime, item);
    log!(
        &runtime.logger,
        Info,
        "新番『{}』Agent 匹配：置信度 {:.2}｜{}",
        item.title,
        detail.as_ref().map(|d| d.confidence).unwrap_or(0.0),
        detail
            .as_ref()
            .map(|d| d.reason.as_str())
            .unwrap_or("(未跑)")
    );

    if detail.as_ref().is_some_and(MatchResult::is_confident) {
        confident(runtime, item, cache_id, &detail.expect("刚判过 is_confident"))
    } else {
        fallback(runtime, item, cache_id, detail)
    }
}

/// 高置信度：建档 → 匹配章节 → 下载 → 发正片频道 → 先发后审。
///
/// 集数没匹配上时**降级成缓冲频道路线**（TS 也是这样）：视频已经下好了但不知道
/// 该标第几集，直接进正片等于把错的东西公开出去。
fn confident(
    runtime: &Arc<Runtime>,
    item: &AnimeItem,
    cache_id: i64,
    detail: &MatchResult,
) -> Result<()> {
    let subject_id = detail.subject_id.ok_or_else(|| Error::Service {
        service: "ai".to_owned(),
        message: "Agent 置信度达标但没给 subjectId".to_owned(),
    })? as i64;
    let anime = import_anime(runtime, subject_id, false)?;
    let episodes = runtime.store.episodes_by_subject(anime.id)?;
    let matched = resolve_episode(runtime, &anime, &episodes, item, detail);

    let plan = match matched.episode_id() {
        Some(episode_id) => {
            log!(
                &runtime.logger,
                Info,
                "先发后审：{}｜{}（第 {episode_id} 章）",
                item.title,
                anime.display_name()
            );
            Plan::post_send(anime.id, cache_id, Some(episode_id), Some(detail.clone()))
        }
        None => {
            log!(
                &runtime.logger,
                Info,
                "高置信度但集数没对上，改走缓冲频道：{}｜{}",
                item.title,
                matched.describe()
            );
            // 缓冲路线要把番剧也写进 cacheAnime（TS 的 saveAnime(cache=true)），
            // 否则提示里读不到这部番。
            let _ = runtime.store.save_cached_anime(&anime);
            Plan::cache(anime.id, cache_id, Some(matched), Some(detail.clone()))
        }
    };
    chain::spawn_with(Arc::clone(runtime), item.clone(), plan);
    Ok(())
}

/// 低置信度 / 无匹配：搜索 → 建档(cache) → 发缓冲频道 → 请管理员确认。
///
/// 搜不到番剧时**不下载**（TS 也是 `promptAdminProvideAnimeInfo` 后直接 return）：
/// 下好了也不知道该叫什么、该标第几集，纯属浪费磁盘和网络。
fn fallback(
    runtime: &Arc<Runtime>,
    item: &AnimeItem,
    cache_id: i64,
    detail: Option<MatchResult>,
) -> Result<()> {
    // Agent 给了 subjectId 但置信度不够：照 TS，优先直接拿这个 id 建档。
    let mut subject = None;
    if let Some(detail) = &detail
        && let Some(subject_id) = detail.subject_id
        && detail.confidence > 0.0
    {
        let client = BgmClient::new(runtime.config.bangumi.clone());
        match client.subject(subject_id) {
            Ok(found) => subject = Some(found),
            Err(error) => log!(
                &runtime.logger,
                Warn,
                "Agent 给的条目 {} 拉不下来，改走搜索：{error}",
                subject_id
            ),
        }
    }

    let subject = match subject {
        Some(subject) => subject,
        None => {
            let keyword = item.names.first().map(String::as_str).unwrap_or(&item.title);
            let client = BgmClient::new(runtime.config.bangumi.clone());
            let found = client.search(keyword)?;
            match found.into_iter().next() {
                Some(subject) => subject,
                None => {
                    // 机器彻底没答案：请管理员手动提供条目（N_anime?c=）。
                    log!(
                        &runtime.logger,
                        Info,
                        "搜不到番剧，请管理员提供：{}",
                        item.title
                    );
                    let _ = crate::prompts::provide_anime_info(runtime, cache_id, item);
                    return Ok(());
                }
            }
        }
    };

    let anime = import_anime(runtime, subject.id as i64, true)?;
    let episodes = runtime.store.episodes_by_subject(anime.id)?;
    let matched = resolve_episode(runtime, &anime, &episodes, item, &detail.unwrap_or_default());
    log!(
        &runtime.logger,
        Info,
        "回退路径送缓冲频道：{}｜{}｜{}",
        item.title,
        anime.display_name(),
        matched.describe()
    );
    let plan = Plan::cache(
        anime.id,
        cache_id,
        Some(matched.clone()),
        None,
    );
    chain::spawn_with(Arc::clone(runtime), item.clone(), plan);
    Ok(())
}

/// 用 Agent 匹配番剧；AI 没开就返回 `None`（走搜索回退）。
///
/// 不把「AI 没开」当错误：那只是配置选择，回退路径照样能跑完整个流程。
fn agent_match(runtime: &Runtime, item: &AnimeItem) -> Option<MatchResult> {
    let tools = BgmTools::new(BgmClient::new(runtime.config.bangumi.clone()));
    let agent = BangumiAgent::new(runtime.config.ai.clone(), tools);
    if !agent.usable() {
        log!(
            &runtime.logger,
            Debug,
            "AI 未启用，新番走搜索回退：{}",
            item.title
        );
        return None;
    }
    let input = MatchInput {
        title: item.title.clone(),
        names: item.names.clone(),
        episode: item.episode.clone(),
    };
    Some(agent.match_subject(&input, LLM_THRESHOLD))
}

/// 拉 Bangumi 条目 + 章节写库。
///
/// `cache = true` 时写 `cacheAnime`（审核中的番剧）而不是 `anime`——
/// 低置信度的条目不该污染正片库，管理员确认后才转正。
fn import_anime(runtime: &Runtime, subject_id: i64, cache: bool) -> Result<Anime> {
    let client = BgmClient::new(runtime.config.bangumi.clone());
    let subject = client.subject(subject_id as u64)?;
    let episodes = client.episodes(subject_id as u64)?;
    let anime = subject.to_anime();
    if cache {
        runtime.store.save_cached_anime(&anime)?;
    } else {
        runtime.store.save_anime(&anime)?;
    }
    let metas: Vec<_> = episodes
        .iter()
        .map(|episode| episode.to_meta(subject_id as u64))
        .collect();
    let written = runtime.store.upsert_episodes(subject_id, &metas)?;
    log!(
        &runtime.logger,
        Info,
        "已建档：{}（{subject_id}，章节 {written} 条，cache={cache}）",
        anime.display_name()
    );
    Ok(anime)
}

/// 定章节 id：Agent 给过就直接用，否则走规则匹配（放送窗口 + 集数）。
///
/// Agent 的 `episodeId` 优先是因为它看过番名 + 集数 + 简介，比纯规则准；
/// 规则只作为「Agent 没给」时的兜底。
fn resolve_episode(
    runtime: &Runtime,
    anime: &Anime,
    episodes: &[anime_core::models::EpisodeMeta],
    item: &AnimeItem,
    detail: &MatchResult,
) -> EpisodeMatch {
    let window = AiredWindow::from_strings(anime.airing_start.as_deref(), None);
    let ai = detail.episode_id.map(|episode_id| AiEpisode {
        episode_id: episode_id as i64,
        sort: detail.episode_sort.unwrap_or(0) as f64,
    });
    let matched = match_episode(
        episodes,
        item.episode.as_deref(),
        &window,
        today_local(),
        ai,
    );
    if !matched.is_matched() {
        log!(
            &runtime.logger,
            Info,
            "集数未匹配：{}｜{}",
            item.title,
            matched.describe()
        );
    }
    matched
}

#[cfg(test)]
mod tests {
    use super::*;
    use anime_core::models::EpisodeMeta;

    fn item(title: &str, episode: Option<&str>) -> AnimeItem {
        AnimeItem {
            title: title.to_owned(),
            pub_date: String::new(),
            magnet: String::new(),
            link: String::new(),
            team: String::new(),
            fansub: vec![],
            names: vec!["某番".to_owned()],
            source: None,
            episode: episode.map(str::to_owned),
        }
    }

    /// Agent 给了 episode_id 时规则不该再插手：AI 优先。
    #[test]
    fn agent_episode_id_wins_over_rules() {
        let episodes = vec![EpisodeMeta {
            id: 111,
            sort: 1.0,
            ..Default::default()
        }];
        let anime = Anime {
            id: 1,
            ..Default::default()
        };
        let detail = MatchResult {
            subject_id: Some(1),
            episode_id: Some(999),
            episode_sort: Some(7),
            confidence: 0.95,
            reason: String::new(),
        };
        // 库里只有第 1 集，BT 写第 7 集——规则会判「库里没有」，但 Agent 说有，
        // 听 Agent 的。
        let matched = resolve_episode_with(&episodes, &anime, &item("x", Some("07")), &detail);
        assert_eq!(matched.episode_id(), Some(999));
    }

    /// 没有 Agent 答案时规则照常兜底：库里没有这一集就报 NotFoundInDb。
    #[test]
    fn without_agent_the_rules_still_apply() {
        let episodes = vec![EpisodeMeta {
            id: 111,
            sort: 1.0,
            ..Default::default()
        }];
        let anime = Anime {
            id: 1,
            ..Default::default()
        };
        let matched =
            resolve_episode_with(&episodes, &anime, &item("x", Some("07")), &MatchResult::default());
        assert!(!matched.is_matched(), "{matched:?}");
    }

    /// 测试替身：绕开需要宿主的 [resolve_episode]，只测「AI 优先」这段决策。
    fn resolve_episode_with(
        episodes: &[EpisodeMeta],
        anime: &Anime,
        item: &AnimeItem,
        detail: &MatchResult,
    ) -> EpisodeMatch {
        let window = AiredWindow::from_strings(anime.airing_start.as_deref(), None);
        let ai = detail.episode_id.map(|id| AiEpisode {
            episode_id: id as i64,
            sort: detail.episode_sort.unwrap_or(0) as f64,
        });
        match_episode(episodes, item.episode.as_deref(), &window, today_local(), ai)
    }

    #[test]
    fn llm_threshold_matches_ts() {
        // 候选 ≤ 5 个时规则就能定，不必调模型——TS 传的就是 5。
        assert_eq!(LLM_THRESHOLD, 5);
    }

    #[test]
    fn cache_plan_asks_admin_while_post_send_creates_review() {
        let cache = Plan::cache(1, 2, None, None);
        assert!(cache.to_cache && cache.review);
        let post = Plan::post_send(1, 2, Some(3), None);
        assert!(!post.to_cache && post.review);
        assert_eq!(post.episode_id, Some(3));
    }
}
