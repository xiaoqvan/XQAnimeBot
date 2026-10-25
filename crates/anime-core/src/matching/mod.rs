//! 番剧匹配：**这条种子属于哪部番、哪一集，能不能直接投**。
//!
//! 两层，都是纯函数，不查库也不联网：
//!
//! ```text
//! season   这条种子属于库里的哪部番（对应 TS 的 hasAnimeSend）
//! episode  属于那部番的第几集，放送日期对不对得上（对应 TS 的 matchBangumiEpisode）
//! ```
//!
//! [`match_item`] 把两层串起来，直接产出「可以投 / 要复核 / 是新番」的结论。
//!
//! ## 为什么分成两层
//!
//! TS 里这两件事隔着一层数据库查询和一次 LLM 调用，导致**匹配逻辑没法单测**——
//! 线上出问题只能靠翻日志猜。分层的代价只是多一个 `AnimeCandidate` 结构体，
//! 换来的是一整套 fixture 单测（阈值、歧义、日期窗口全都能固定住）。
//!
//! ## AI 在哪
//!
//! 不在这个 crate 里。TS 的 `aiEpisodeSearch` / `matchAnimeSubject` 需要 LLM 凭据，
//! 属于插件壳的职责；本模块只接受一个 [`AiEpisode`]，有值就优先采信——和 TS 的
//! 「AI 优先，自动匹配兜底」顺序一致。

pub mod episode;
pub mod normalize;
pub mod season;

use chrono::NaiveDate;

use crate::models::{AnimeItem, EpisodeMeta};

pub use episode::{
    AiEpisode, AiredWindow, EpisodeMatch, EpisodeSource, LEAD_DAYS, MAX_FUTURE_DAYS, TRAIL_DAYS,
    match_episode, parse_airdate, today_local,
};
pub use normalize::{compact_for_match, normalize_for_match};
pub use season::{
    AnimeCandidate, EpisodeFit, MatchKind, MatchPolicy, SeasonDecision, SeasonMatch, decide_season,
    decide_season_with, episode_total, fit_episode, rank_seasons,
};

/// 一条种子的完整匹配结论。
#[derive(Debug, Clone)]
pub struct ItemMatch {
    pub season: SeasonDecision,
    /// 只有当季命中时才有值——没确定是哪部番，谈「第几集」没有意义。
    pub episode: Option<EpisodeMatch>,
    /// 集数是否超出番剧总集数。只是信号（第 13 集 / 库里写 12 集），不拦截。
    pub episode_fit: EpisodeFit,
}

impl ItemMatch {
    /// 可以自动投：番剧匹配上了，集数也匹配上了。
    pub fn is_ready(&self) -> bool {
        matches!(self.season, SeasonDecision::Matched(_))
            && self.episode.as_ref().is_some_and(EpisodeMatch::is_matched)
    }

    /// 一句话说明，直接进日志或审核消息。
    pub fn describe(&self) -> String {
        let mut parts = vec![self.season.describe()];
        if let Some(episode) = &self.episode {
            parts.push(episode.describe());
        }
        if let EpisodeFit::Beyond { episode, total } = self.episode_fit {
            parts.push(format!(
                "注意：第 {} 集超出总集数 {}，可能是新一季",
                episode::format_number(episode),
                episode::format_number(total)
            ));
        }
        parts.join("；")
    }
}

/// 用默认阈值匹配。
pub fn match_item(
    item: &AnimeItem,
    candidates: &[AnimeCandidate],
    episodes: &[EpisodeMeta],
    today: NaiveDate,
    ai: Option<AiEpisode>,
) -> ItemMatch {
    match_item_with(
        item,
        candidates,
        episodes,
        today,
        ai,
        MatchPolicy::default(),
    )
}

/// 用指定阈值匹配。`episodes` 应当是**命中那部番**的章节列表（调用方按 season 结论再查）。
pub fn match_item_with(
    item: &AnimeItem,
    candidates: &[AnimeCandidate],
    episodes: &[EpisodeMeta],
    today: NaiveDate,
    ai: Option<AiEpisode>,
    policy: MatchPolicy,
) -> ItemMatch {
    let season = decide_season_with(&item.names, candidates, policy);
    let candidate = season
        .anime_id()
        .and_then(|id| candidates.iter().find(|candidate| candidate.id == id));

    let episode_fit = match candidate
        .and_then(|candidate| candidate.episode.as_deref())
        .and_then(episode_total)
    {
        Some(total) => fit_episode(Some(total), item.episode.as_deref()),
        None => EpisodeFit::Unknown,
    };

    let episode = match &season {
        SeasonDecision::Matched(_) => {
            // 时间窗的两端：番剧起始放送日 + 章节列表最后一集的放送日。
            let window = AiredWindow::from_strings(
                candidate.and_then(|candidate| candidate.airing_start.as_deref()),
                last_airdate(episodes),
            );
            Some(match_episode(
                episodes,
                item.episode.as_deref(),
                &window,
                today,
                ai,
            ))
        }
        _ => None,
    };

    ItemMatch {
        season,
        episode,
        episode_fit,
    }
}

/// 章节列表里最后一集的放送日（按 `sort`、`id` 取最大的一集）。
///
/// 注意这里**不筛正片**：TS 也是拿排序后的最后一集。

fn last_airdate(episodes: &[EpisodeMeta]) -> Option<&str> {
    episodes
        .iter()
        .max_by(|a, b| {
            a.sort
                .partial_cmp(&b.sort)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.id.cmp(&b.id))
        })
        .and_then(|episode| episode.airdate.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EpisodeMeta;

    fn candidate(id: i64, name_cn: &str, total: &str) -> AnimeCandidate {
        AnimeCandidate {
            id,
            name_cn: Some(name_cn.to_owned()),
            episode: Some(total.to_owned()),
            airing_start: Some("2026年1月5日".to_owned()),
            ..Default::default()
        }
    }

    fn episodes() -> Vec<EpisodeMeta> {
        (1..=12)
            .map(|index| EpisodeMeta {
                id: i64::from(100 + index),
                sort: f64::from(index),
                ep: f64::from(index),
                r#type: Some(0),
                airdate: Some(
                    (NaiveDate::from_ymd_opt(2026, 1, 5).unwrap()
                        + chrono::TimeDelta::days(i64::from(index - 1) * 7))
                    .format("%Y-%m-%d")
                    .to_string(),
                ),
                ..Default::default()
            })
            .collect()
    }

    fn item(name: &str, episode: &str) -> AnimeItem {
        AnimeItem {
            title: format!("[喵萌奶茶屋] {name} [{episode}][1080P]"),
            pub_date: String::new(),
            magnet: "magnet:?xt=urn:btih:00".to_owned(),
            link: String::new(),
            team: "喵萌奶茶屋".to_owned(),
            fansub: vec!["喵萌奶茶屋".to_owned()],
            names: vec![name.to_owned()],
            source: None,
            episode: Some(episode.to_owned()),
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 25).unwrap()
    }

    #[test]
    fn ready_item_matches_both_layers() {
        let library = vec![candidate(1, "葬送的芙莉莲", "12")];
        let outcome = match_item(
            &item("葬送的芙莉莲", "05"),
            &library,
            &episodes(),
            today(),
            None,
        );
        assert!(outcome.is_ready(), "{}", outcome.describe());
        let EpisodeMatch::Matched { episode_id, .. } = outcome.episode.unwrap() else {
            panic!("应当匹配到章节");
        };
        assert_eq!(episode_id, 105);
        assert_eq!(outcome.episode_fit, EpisodeFit::Inside);
    }

    #[test]
    fn new_anime_has_no_episode_layer() {
        let library = vec![candidate(1, "葬送的芙莉莲", "12")];
        let outcome = match_item(
            &item("完全没听过的番", "05"),
            &library,
            &episodes(),
            today(),
            None,
        );
        assert!(matches!(outcome.season, SeasonDecision::New));
        assert!(outcome.episode.is_none());
        assert!(!outcome.is_ready());
    }

    #[test]
    fn out_of_range_episode_is_flagged_but_not_blocked() {
        // 库里有 12 集，来个第 13 集：season 命中，episode 找不到章节，
        // 同时 fit 会给出 Beyond 信号（可能是新一季的种子）。
        let library = vec![candidate(1, "葬送的芙莉莲", "12")];
        let outcome = match_item(
            &item("葬送的芙莉莲", "13"),
            &library,
            &episodes(),
            today(),
            None,
        );
        assert!(matches!(outcome.season, SeasonDecision::Matched(_)));
        assert!(matches!(
            outcome.episode,
            Some(EpisodeMatch::NotFoundInDb { .. })
        ));
        assert_eq!(
            outcome.episode_fit,
            EpisodeFit::Beyond {
                episode: 13.0,
                total: 12.0
            }
        );
        assert!(
            outcome.describe().contains("超出总集数"),
            "{}",
            outcome.describe()
        );
    }
}
