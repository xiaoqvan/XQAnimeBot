//! 番剧匹配（season / subject 级）：**这条种子属于库里的哪部番**。
//!
//! 对应 TS 的 `database/query.ts::hasAnimeSend`——先用 `$in` 精确匹配，失败再把库里
//! 所有番剧拉出来做归一化后的「包含」判断。TS 是「谁先命中算谁」（按 `updatedAt`
//! 倒序遍历），这里改成**打分排序**：
//!
//! ```text
//! 完全相等（归一化后）        1000 分
//! 去掉空白后相等               950 分
//! 包含关系（互为子串）  600 + 300 × 短/长，前缀额外 +40
//! ```
//!
//! 打分不是为了「更好看」，而是为了三件事：同分时能按 `updatedAt` 选新的、能识别
//! 「两个候选咬得太近」而转人工、能把「只命中两个字的短名」压到阈值以下。
//!
//! ## 与 TS 的两处刻意偏差
//!
//! 1. **包含关系要求较短的一侧至少 3 个字符**（[`MIN_CONTAINS_LEN`]）。TS 里
//!    `"死神"` 会命中 `"死神 千年血战篇"`，把新一季的种子投到上一季的频道——
//!    这正是「宁可漏也不发错」要避免的。两字名走精确匹配仍然能用。
//! 2. **纯 ASCII 名的包含关系要求词边界**。否则 `ani` 会命中 `anime`、
//!    `fate` 会命中 `fatezero`（后者是真番名，但不是同一部）。
//!
//! 偏离只影响「自动投放」这一段：分数不够会落到 [`SeasonDecision::NeedsReview`]，
//! 交给人或 AI 复核，而不是直接丢弃。

use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;

use super::episode::leading_number;
use super::normalize::{compact_for_match, normalize_for_match};
use crate::models::Anime;

/// 「包含」命中时，较短一侧的最小字符数。见模块文档第 1 条偏差。
pub const MIN_CONTAINS_LEN: usize = 3;

/// 前缀命中的额外加分：`葬送的芙莉莲` vs `葬送的芙莉莲 第二季` 属于典型情况，
/// 加一点分让它稳过阈值，同时不至于越过「完全相等」。
pub const PREFIX_BONUS: u32 = 40;

/// 库内番剧的匹配视图。
///
/// 故意比 [`crate::models::Anime`] 窄：匹配只用到这几个字段，插件壳查询时可以只取
/// 这几列，单测里也能手工造，不用凑齐整个文档。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimeCandidate {
    /// Bangumi subject id。
    pub id: i64,
    pub name: Option<String>,
    pub name_cn: Option<String>,
    /// 多语言别名（bangumi.moe 的 zh_cn / ja / en 都在这里）。
    pub names: Vec<String>,
    /// 总集数（`"12"` / `"12.5"` / `"全12话"` / 空）。
    pub episode: Option<String>,
    /// 起始放送日（`2026年1月16日`），集数对齐的时间窗要用。
    pub airing_start: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl AnimeCandidate {
    /// 从库文档投影出匹配视图。
    pub fn from_anime(anime: &Anime) -> Self {
        Self {
            id: anime.id,
            name: anime.name.clone(),
            name_cn: anime.name_cn.clone(),
            names: anime.names.clone(),
            episode: anime.episode.clone(),
            airing_start: anime.airing_start.clone(),
            updated_at: anime.updated_at,
        }
    }

    /// 全部可用于匹配的名字：`name`、`name_cn`、`names`（顺序照抄 TS 的
    /// `[anime.name, anime.name_cn, ...anime.names]`）。
    pub fn db_names(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for name in self
            .name
            .as_deref()
            .into_iter()
            .chain(self.name_cn.as_deref())
            .chain(self.names.iter().map(String::as_str))
        {
            let name = name.trim();
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
        out
    }
}

/// 命中方式。分数由它决定，也用于日志里解释「为什么匹配上」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// 归一化后完全相等。
    Exact,
    /// 只差空白（`葬送的 芙莉莲` vs `葬送的芙莉莲`）。
    Compact,
    /// 互为子串（`葬送的芙莉莲` vs `葬送的芙莉莲 第二季`）。
    Contains,
}

impl MatchKind {
    /// 该命中方式的基础分，包含关系另加长度比例分。
    pub fn base_score(self) -> u32 {
        match self {
            Self::Exact => 1000,
            Self::Compact => 950,
            Self::Contains => 600,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "完全相等",
            Self::Compact => "忽略空格后相等",
            Self::Contains => "包含关系",
        }
    }
}

/// 一次命中。
#[derive(Debug, Clone, PartialEq)]
pub struct SeasonMatch {
    pub anime_id: i64,
    pub kind: MatchKind,
    /// 综合分，越高越可信。
    pub score: u32,
    /// 命中的输入名（BT 标题解析出的候选项，原样保留便于日志）。
    pub input: String,
    /// 命中的库内名。
    pub matched_name: String,
    /// 输入名在 `names` 里的序号（0 为最优先）。同分时它决定先后。
    pub input_rank: usize,
}

/// 匹配阈值。默认值是拿真实番名量出来的，改之前先看 [`SeasonDecision`] 的说明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchPolicy {
    /// 低于这个分数不进 `Matched`，转人工/AI 复核。
    pub min_score: u32,
    /// 前两名（且属于不同番剧）分差小于等于它 → 视为分不清。`0` = 不做歧义判定。
    pub ambiguous_gap: u32,
    /// 包含关系要求较短一侧至少多少字符。
    pub min_contains_len: usize,
    /// 纯 ASCII 名的包含关系是否要求词边界（`ani` 不能命中 `anime`）。
    pub require_word_boundary: bool,
}

impl MatchPolicy {
    /// 与 TS `hasAnimeSend` 完全一致的策略：包含即命中、不设阈值、不做歧义复核。
    ///
    /// 这是「行为对齐」用的档位；[MatchPolicy::default] 是加固过的档位（宁可漏也不发错）。
    /// 两者都保留，由调用方按产品口味选，别在代码里二选一写死。
    pub const fn ts() -> Self {
        Self {
            min_score: 0,
            ambiguous_gap: 0,
            min_contains_len: 1,
            require_word_boundary: false,
        }
    }
}

impl Default for MatchPolicy {
    fn default() -> Self {
        // 760 = 包含关系在长度比 0.4 且带前缀奖励时的分数（600 + 120 + 40）。
        // 也就是说「短名至少覆盖长名的 40%」才自动过。
        Self {
            min_score: 760,
            ambiguous_gap: 40,
            min_contains_len: MIN_CONTAINS_LEN,
            require_word_boundary: true,
        }
    }
}

/// 匹配结论。
#[derive(Debug, Clone, PartialEq)]
pub enum SeasonDecision {
    /// 分够高且没有够格的竞争者 → 按「已有番剧」处理。
    Matched(SeasonMatch),
    /// 有近似命中但不敢自动投：分数不够（[`Self::NeedsReview`] 的 `also` 通常为空）
    /// 或两个候选咬得太近。TS 版在这里会直接走 `handleNewAnime`（AI 匹配 + 待审核），
    /// 语义一致——**复核总比投错便宜**。
    NeedsReview {
        best: SeasonMatch,
        also: Option<SeasonMatch>,
    },
    /// 库里根本没有 → 新番候选。
    New,
}

impl SeasonDecision {
    /// 命中的番剧 id（`New` 时为空）。
    pub fn anime_id(&self) -> Option<i64> {
        match self {
            Self::Matched(hit) => Some(hit.anime_id),
            Self::NeedsReview { best, .. } => Some(best.anime_id),
            Self::New => None,
        }
    }

    /// 一句话说明，直接进日志或审核消息。
    pub fn describe(&self) -> String {
        match self {
            Self::Matched(hit) => format!(
                "匹配『{}』← {}（{}，{} 分）",
                hit.matched_name,
                hit.input,
                hit.kind.as_str(),
                hit.score
            ),
            Self::NeedsReview { best, also: None } => format!(
                "近似命中『{}』← {}（{} 分，低于阈值，需复核）",
                best.matched_name, best.input, best.score
            ),
            Self::NeedsReview {
                best,
                also: Some(runner),
            } => format!(
                "候选分不清：『{}』{} 分 vs『{}』{} 分（需复核）",
                best.matched_name, best.score, runner.matched_name, runner.score
            ),
            Self::New => "库中无匹配（按新番处理）".to_owned(),
        }
    }
}

/// 集数与番剧总集数的关系。只作为**信号**，不做硬拦截：真正的越界判断靠放送日期窗口
/// （见 [`super::episode`]），这里只是让审核者一眼看到「第 13 集，但库里写 12 集」。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EpisodeFit {
    Inside,
    Beyond {
        episode: f64,
        total: f64,
    },
    /// 总集数或集数解析不出来（`SP`、`剧场版`、`未知`）。
    Unknown,
}

/// 解析总集数：`"12"` / `"12.5"` / `"全12话"` 都取第一个数字。
pub fn episode_total(text: &str) -> Option<f64> {
    static NUMBER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d+(?:\.\d+)?").expect("内置正则必须合法"));
    NUMBER
        .find(text)
        .and_then(|found| found.as_str().parse().ok())
}

/// 判断 BT 集数是否落在番剧总集数内。
pub fn fit_episode(total: Option<f64>, bt_episode: Option<&str>) -> EpisodeFit {
    let (Some(total), Some(text)) = (total, bt_episode) else {
        return EpisodeFit::Unknown;
    };
    let Some(episode) = leading_number(text) else {
        return EpisodeFit::Unknown;
    };
    // 浮点比较留一点余量：`12` 与 `12.0` 必须算相等。
    if episode <= total + f64::EPSILON {
        EpisodeFit::Inside
    } else {
        EpisodeFit::Beyond { episode, total }
    }
}

/// 给所有（输入名 × 库内名）组合打分，同一部番只保留最高的一条，按分数倒序返回。
///
/// 纯函数：不查库、不联网，所以优先级、阈值、歧义判定全都能用 fixture 单测。
pub fn rank_seasons(names: &[String], candidates: &[AnimeCandidate]) -> Vec<SeasonMatch> {
    rank_seasons_with(names, candidates, MatchPolicy::default())
}

/// 用指定策略打分排序（`min_contains_len` / 词边界 / 阈值都在 `policy` 里）。
pub fn rank_seasons_with(
    names: &[String],
    candidates: &[AnimeCandidate],
    policy: MatchPolicy,
) -> Vec<SeasonMatch> {
    let mut matches: Vec<SeasonMatch> = Vec::new();

    for (rank, raw_input) in names.iter().enumerate() {
        let input = normalize_for_match(raw_input);
        if input.is_empty() {
            continue;
        }
        let input_compact = compact_for_match(raw_input);

        for candidate in candidates {
            let mut best: Option<SeasonMatch> = None;
            for db_raw in candidate.db_names() {
                let db = normalize_for_match(db_raw);
                if db.is_empty() {
                    continue;
                }
                let Some((kind, score)) = compare(&input, &input_compact, &db, policy) else {
                    continue;
                };
                let hit = SeasonMatch {
                    anime_id: candidate.id,
                    kind,
                    score,
                    input: raw_input.clone(),
                    matched_name: db_raw.to_owned(),
                    input_rank: rank,
                };
                if best
                    .as_ref()
                    .is_none_or(|current| hit.score > current.score)
                {
                    best = Some(hit);
                }
            }
            let Some(hit) = best else { continue };
            // 同一部番只留一条：库里有 name / name_cn / names 好几份名字，
            // 不去重的话候选列表里同一部番会出现三四次，歧义判定就失效了。
            match matches
                .iter_mut()
                .find(|existing| existing.anime_id == hit.anime_id)
            {
                Some(existing) => {
                    if hit.score > existing.score {
                        *existing = hit;
                    }
                }
                None => matches.push(hit),
            }
        }
    }

    let updated_at = |id: i64| {
        candidates
            .iter()
            .find(|candidate| candidate.id == id)
            .and_then(|candidate| candidate.updated_at)
    };
    matches.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            // 同分：输入名更靠前（更优先）的赢。
            .then(a.input_rank.cmp(&b.input_rank))
            // 再同：最近更新过的赢（TS 的 `sort({updatedAt: -1})` 同义）。
            .then_with(|| updated_at(b.anime_id).cmp(&updated_at(a.anime_id)))
            .then(b.anime_id.cmp(&a.anime_id))
    });
    matches
}

/// 用默认阈值给出结论。
pub fn decide_season(names: &[String], candidates: &[AnimeCandidate]) -> SeasonDecision {
    decide_season_with(names, candidates, MatchPolicy::default())
}

/// 用指定阈值给出结论。
pub fn decide_season_with(
    names: &[String],
    candidates: &[AnimeCandidate],
    policy: MatchPolicy,
) -> SeasonDecision {
    let ranked = rank_seasons_with(names, candidates, policy);
    let Some(best) = ranked.first().cloned() else {
        return SeasonDecision::New;
    };

    if best.score < policy.min_score {
        return SeasonDecision::NeedsReview {
            also: ranked.get(1).cloned(),
            best,
        };
    }

    // 分够高，但第二名的分数咬得太近且是另一部番 → 分不清，别猜。
    if policy.ambiguous_gap > 0
        && let Some(runner) = ranked.get(1).cloned()
        && runner.anime_id != best.anime_id
        && runner.score >= policy.min_score
        && best.score - runner.score <= policy.ambiguous_gap
    {
        return SeasonDecision::NeedsReview {
            best,
            also: Some(runner),
        };
    }

    SeasonDecision::Matched(best)
}

/// 比较一对（输入名，库内名），返回命中方式与分数。
fn compare(
    input: &str,
    input_compact: &str,
    db: &str,
    policy: MatchPolicy,
) -> Option<(MatchKind, u32)> {
    if input == db {
        return Some((MatchKind::Exact, MatchKind::Exact.base_score()));
    }

    let db_compact = compact_for_match(db);
    if input_compact == db_compact {
        return Some((MatchKind::Compact, MatchKind::Compact.base_score()));
    }

    let input_len = input_compact.chars().count();
    let db_len = db_compact.chars().count();
    // 对齐档位（TS）允许单字/两字名直接命中；默认档位要求 3 个字符。
    let min_len = policy.min_contains_len.max(1);
    if input_len < min_len || db_len < min_len {
        return None;
    }

    let (shorter, longer) = if db_compact.contains(input_compact) {
        (input_compact, db_compact.as_str())
    } else if input_compact.contains(&db_compact) {
        (db_compact.as_str(), input_compact)
    } else {
        return None;
    };

    // 英文番名的子串命中必须落在词边界上，否则 `ani` 会命中 `anime`。
    if policy.require_word_boundary
        && shorter.is_ascii()
        && longer.is_ascii()
        && !ascii_word_hit(longer, shorter)
    {
        return None;
    }

    let ratio = shorter.chars().count() as f64 / longer.chars().count() as f64;
    let mut score = MatchKind::Contains.base_score() + (ratio * 300.0).round() as u32;
    if longer.starts_with(shorter) {
        score += PREFIX_BONUS;
    }
    Some((MatchKind::Contains, score))
}

/// `shorter` 是否以「独立单词」的形式出现在 `longer` 里（两侧都不是字母数字）。
fn ascii_word_hit(longer: &str, shorter: &str) -> bool {
    let hay = longer.as_bytes();
    let needle = shorter.as_bytes();
    let mut from = 0;
    while let Some(offset) = longer[from..].find(shorter) {
        let start = from + offset;
        let end = start + needle.len();
        let before_ok = start == 0 || !hay[start - 1].is_ascii_alphanumeric();
        let after_ok = end == hay.len() || !hay[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        // `shorter` 是纯 ASCII，+1 一定落在字符边界上。
        from = start + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn candidate(id: i64, name_cn: &str) -> AnimeCandidate {
        AnimeCandidate {
            id,
            name_cn: Some(name_cn.to_owned()),
            ..Default::default()
        }
    }

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn ts_policy_accepts_two_char_containment() {
        // TS 的 hasAnimeSend 是「包含即命中」：死神 → 死神 千年血战篇。
        // 默认档位（3 字符 + 阈值）会把它压到复核，这是刻意的加固，不是 bug。
        let library = vec![candidate(1, "死神 千年血战篇")];
        let strict = decide_season(&names(&["死神"]), &library);
        assert!(
            !matches!(strict, SeasonDecision::Matched(_)),
            "默认档位不该自动投放：{strict:?}"
        );

        let ts = decide_season_with(&names(&["死神"]), &library, MatchPolicy::ts());
        let SeasonDecision::Matched(hit) = ts else {
            panic!("TS 档位应当直接命中：{ts:?}");
        };
        assert_eq!(hit.anime_id, 1);
        assert_eq!(hit.kind, MatchKind::Contains);
    }

    #[test]
    fn ts_policy_has_no_ascii_word_boundary() {
        // 英文短名不做词边界判断（TS 行为）；默认档位仍然要求，避免 ani → anime。
        let library = vec![candidate(1, "anime")];
        let ts = decide_season_with(&names(&["ani"]), &library, MatchPolicy::ts());
        assert!(matches!(ts, SeasonDecision::Matched(_)), "{ts:?}");

        let strict = decide_season(&names(&["ani"]), &library);
        assert!(!matches!(strict, SeasonDecision::Matched(_)), "{strict:?}");
    }

    #[test]
    fn exact_match_survives_fullwidth_and_case() {
        let library = vec![candidate(1, "SPY×FAMILY")];
        let decision = decide_season(&names(&["ｓｐｙ×ｆａｍｉｌｙ"]), &library);
        let SeasonDecision::Matched(hit) = decision else {
            panic!("全角小写应当精确命中：{decision:?}");
        };
        assert_eq!(hit.anime_id, 1);
        assert_eq!(hit.kind, MatchKind::Exact);
        assert_eq!(hit.score, 1000);
    }

    #[test]
    fn space_difference_is_compact_match() {
        let library = vec![candidate(1, "葬送的芙莉莲")];
        let SeasonDecision::Matched(hit) = decide_season(&names(&["葬送的 芙莉莲"]), &library)
        else {
            panic!("只差空格应当命中");
        };
        assert_eq!(hit.kind, MatchKind::Compact);
        assert!(hit.score >= 950);
    }

    #[test]
    fn season_suffix_still_matches_but_scores_lower() {
        let library = vec![candidate(1, "葬送的芙莉莲 第二季")];
        let SeasonDecision::Matched(hit) = decide_season(&names(&["葬送的芙莉莲"]), &library)
        else {
            panic!("带季名的库内名应当被短名命中");
        };
        assert_eq!(hit.kind, MatchKind::Contains);
        assert!(hit.score > 760 && hit.score < 1000, "实际 {}", hit.score);
    }

    #[test]
    fn two_char_name_does_not_latch_onto_longer_title() {
        // 这是与 TS 的刻意偏差：TS 会把「死神」的种子投到「死神 千年血战篇」上。
        let library = vec![candidate(1, "死神 千年血战篇")];
        assert_eq!(
            decide_season(&names(&["死神"]), &library),
            SeasonDecision::New
        );
        // 完全等于库内名时仍然能中。
        let SeasonDecision::Matched(hit) = decide_season(&names(&["死神 千年血战篇"]), &library)
        else {
            panic!("完全相等应当命中");
        };
        assert_eq!(hit.kind, MatchKind::Exact);
    }

    #[test]
    fn ascii_names_need_word_boundary() {
        let library = vec![candidate(1, "Anime")];
        assert_eq!(
            decide_season(&names(&["ani"]), &library),
            SeasonDecision::New
        );
        let library = vec![candidate(1, "Fate/Zero")];
        let SeasonDecision::Matched(hit) = decide_season(&names(&["Fate"]), &library) else {
            panic!("`Fate` 是 `Fate/Zero` 的独立单词，应当命中");
        };
        assert_eq!(hit.anime_id, 1);
    }

    #[test]
    fn equal_scores_from_two_anime_need_review() {
        let library = vec![candidate(1, "葬送的芙莉莲"), candidate(2, "葬送的芙莉莲")];
        let decision = decide_season(&names(&["葬送的芙莉莲"]), &library);
        let SeasonDecision::NeedsReview { best, also } = decision else {
            panic!("两部同名番应当转复核：{decision:?}");
        };
        assert_eq!(best.score, 1000);
        // 同分时按 id 倒序，所以第二名是另一部同名片，重点是「另一部」而不是具体 id。
        let runner_up = also.expect("必须有并列候选");
        assert_ne!(runner_up.anime_id, best.anime_id);
        assert_eq!(runner_up.score, 1000);
    }

    #[test]
    fn recent_update_wins_when_scores_tie() {
        let older = AnimeCandidate {
            updated_at: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
            ..candidate(1, "葬送的芙莉莲")
        };
        let newer = AnimeCandidate {
            updated_at: Some(Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap()),
            ..candidate(2, "葬送的芙莉莲")
        };
        let ranked = rank_seasons(&names(&["葬送的芙莉莲"]), &[older, newer]);
        assert_eq!(ranked[0].anime_id, 2, "同分时应选最近更新的");
    }

    #[test]
    fn earlier_input_name_wins_when_everything_else_ties() {
        let library = vec![candidate(1, "葬送的芙莉莲"), candidate(2, "Frieren")];
        let ranked = rank_seasons(&names(&["葬送的芙莉莲", "Frieren"]), &library);
        assert_eq!(ranked[0].input_rank, 0, "优先名应当排前面");
        assert_eq!(ranked[0].anime_id, 1);
    }

    #[test]
    fn episode_total_and_fit() {
        assert_eq!(episode_total("12"), Some(12.0));
        assert_eq!(episode_total("全12话"), Some(12.0));
        assert_eq!(episode_total("12.5"), Some(12.5));
        assert_eq!(episode_total(""), None);

        assert_eq!(fit_episode(Some(12.0), Some("12")), EpisodeFit::Inside);
        assert_eq!(
            fit_episode(Some(12.0), Some("13")),
            EpisodeFit::Beyond {
                episode: 13.0,
                total: 12.0
            }
        );
        assert_eq!(fit_episode(None, Some("13")), EpisodeFit::Unknown);
        assert_eq!(fit_episode(Some(12.0), Some("SP01")), EpisodeFit::Unknown);
    }

    #[test]
    fn empty_library_is_new() {
        assert_eq!(decide_season(&names(&["某番"]), &[]), SeasonDecision::New);
        assert_eq!(
            decide_season(&[], &[candidate(1, "某番")]),
            SeasonDecision::New
        );
    }
}
