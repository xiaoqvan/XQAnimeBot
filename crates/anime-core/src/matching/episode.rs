//! 集数对齐：**BT 的这一集，对应 Bangumi 的哪个章节**。
//!
//! 逐条对照 TS 的 `utils/matcher.ts::matchBangumiEpisode`，只把 AI 那一段摘出去：
//!
//! ```text
//! 1. 没有集数                     → InvalidInput
//! 2. AI 匹配有结果                → Matched（优先级最高，插件壳负责调 LLM）
//! 3. 去掉 v2 / END 后缀，取开头数字 → 解析不出 → NotFoundInDb
//! 4. 按 (sort, id) 排序，找 sort 相同且 type==0 的章节
//! 5. 找到就查放送日期窗口，找不到就带着集数范围报 NotFoundInDb
//! ```
//!
//! 窗口规则（照抄 TS）：`起始放送日 - 8 天` 到 `最后一集放送日 + 30 天`；
//! 拿不到起止日期时退化成「不能超过未来 8 天」。前后不对称是有原因的——开播前
//! 会有偷跑，所以往前松 8 天；完结后补种、合集、修正版会陆续冒出来，所以往后松 30 天。
//!
//! 时间一律用 [`NaiveDate`]（只比日期，不比时刻）：TS 那套 `new Date()` 的比较里
//! 时区、凌晨边界都在制造假阳性，而放送日期本来就只精确到天。

use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;

use crate::models::EpisodeMeta;

/// 开播前允许的提前量（天）。
pub const LEAD_DAYS: i64 = 8;
/// 完结后允许的滞后量（天）。
pub const TRAIL_DAYS: i64 = 30;
/// 拿不到起止日期时，最多允许落在未来几天内。
pub const MAX_FUTURE_DAYS: i64 = 8;

/// AI 给出的匹配结果（插件壳调完 LLM 再灌进来）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiEpisode {
    pub episode_id: i64,
    pub sort: f64,
}

/// 命中来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeSource {
    Ai,
    Auto,
}

/// 集数匹配结果。四个分支对应 TS 的 `EpisodeMatchResult` 联合类型。
#[derive(Debug, Clone, PartialEq)]
pub enum EpisodeMatch {
    Matched {
        episode_id: i64,
        /// Bangumi 原始集数（`ep`），可能是 12.5 这种。
        ep: f64,
        /// 排序用的集数（`sort`），正片就是 1..N。
        sort: f64,
        source: EpisodeSource,
    },
    /// 集数在章节列表里找不到（例如第 13 集不在 12 集的列表里）。
    NotFoundInDb { message: String },
    /// 放送日期严重不符——多半是「拿错季了」。
    DateMismatch {
        message: String,
        diff_days: i64,
        episode_id: i64,
    },
    /// BT 标题没解析出可用集数。
    InvalidInput { message: String },
}

impl EpisodeMatch {
    pub fn is_matched(&self) -> bool {
        matches!(self, Self::Matched { .. })
    }

    pub fn episode_id(&self) -> Option<i64> {
        match self {
            Self::Matched { episode_id, .. } | Self::DateMismatch { episode_id, .. } => {
                Some(*episode_id)
            }
            _ => None,
        }
    }

    /// 一句话说明，直接进日志或审核消息。
    pub fn describe(&self) -> String {
        match self {
            Self::Matched {
                episode_id,
                sort,
                source,
                ..
            } => format!(
                "匹配章节 {episode_id}（sort={}，{}）",
                format_number(*sort),
                match source {
                    EpisodeSource::Ai => "AI 判定",
                    EpisodeSource::Auto => "自动对齐",
                }
            ),
            Self::NotFoundInDb { message }
            | Self::DateMismatch { message, .. }
            | Self::InvalidInput { message } => message.clone(),
        }
    }
}

/// 放送时间窗。起止任一缺失就退化成「未来 8 天」检查。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AiredWindow {
    pub start: Option<NaiveDate>,
    pub last: Option<NaiveDate>,
}

impl AiredWindow {
    /// 从「番剧起始放送日 + 最后一集放送日」两段原始字符串构造。
    /// 解析不出来的那一侧留空，不影响另一侧。
    pub fn from_strings(start: Option<&str>, last: Option<&str>) -> Self {
        Self {
            start: start.and_then(parse_airdate),
            last: last.and_then(parse_airdate),
        }
    }

    /// 允许区间：`[start - 8d, last + 30d]`。起止齐全时才有效。
    pub fn allowed_range(&self) -> Option<(NaiveDate, NaiveDate)> {
        let (start, last) = (self.start?, self.last?);
        Some((
            start - chrono::TimeDelta::days(LEAD_DAYS),
            last + chrono::TimeDelta::days(TRAIL_DAYS),
        ))
    }
}

/// 解析放送日期：`2026年1月16日` / `2026-01-16` / `2026/01/16` / RFC3339 全吃。
///
/// TS 用了两条正则（中文一条、ISO 一条），但第一条的 `\D+` 本来就跨得过分隔符，
/// 第二条是死代码——这里合成一条。
pub fn parse_airdate(text: &str) -> Option<NaiveDate> {
    static YMD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d{4})\D+(\d{1,2})\D+(\d{1,2})").expect("内置正则必须合法"));
    if let Some(captures) = YMD.captures(text) {
        let year: i32 = captures[1].parse().ok()?;
        let month: u32 = captures[2].parse().ok()?;
        let day: u32 = captures[3].parse().ok()?;
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
            return Some(date);
        }
    }
    // 兜底：纯数字时间戳之外的 ISO 空格格式（`2026-01-16 00:00:00`）已被上面的正则覆盖，
    // 这里只处理带时区偏移、正则也没兜住的形态。
    text.parse::<NaiveDate>().ok()
}

/// 取字符串开头的数字，等价 JS 的 `parseFloat`：`"05"`→5、`"13.5"`→13.5、`"SP01"`→`None`。
///
/// 注意 `SP01` 返回 `None` 是**刻意的**：特别篇的集数语义和正片不同，交给 AI 或人工，
/// 不能拿 `01` 去对正片第 1 集。
pub fn leading_number(text: &str) -> Option<f64> {
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        match ch {
            '0'..='9' => {
                seen_digit = true;
                end = index + ch.len_utf8();
            }
            '.' if seen_digit && !seen_dot => {
                seen_dot = true;
                end = index + ch.len_utf8();
            }
            // 前导空白跳过；数字开始后遇到非数字就停。
            c if c.is_whitespace() && !seen_digit => continue,
            _ => break,
        }
    }
    if !seen_digit {
        return None;
    }
    // 末尾孤零零的小数点（`"05."`）去掉再解析；`parse::<f64>` 不接受首尾空白，
    // 前导空白在扫描时被跳过了但还留在切片里，所以这里也要 trim。
    let trimmed = text[..end].trim().trim_end_matches('.');
    trimmed.parse().ok()
}

/// 集数匹配主函数。
///
/// - `episodes`：该番剧的全部章节（不需要预排序，函数内部会排）。
/// - `bt_episode`：BT 标题解析出的集数，如 `05` / `05v2` / `SP01` / `未知`。
/// - `window`：放送时间窗，见 [`AiredWindow`]。
/// - `today`：今天的日期。显式传入而不是内部取 `now()`，否则单测没法固定时间。
/// - `ai`：AI 判定结果，有值就直接信它（TS 同样「AI 优先」）。
pub fn match_episode(
    episodes: &[EpisodeMeta],
    bt_episode: Option<&str>,
    window: &AiredWindow,
    today: NaiveDate,
    ai: Option<AiEpisode>,
) -> EpisodeMatch {
    let Some(raw_episode) = bt_episode.map(str::trim).filter(|text| !text.is_empty()) else {
        return EpisodeMatch::InvalidInput {
            message: "BT 条目未包含集数".to_owned(),
        };
    };

    // AI 优先：自动对齐再准也准不过「看过番名+集数+简介」的模型，
    // 而错一集的代价是整季投错频道。
    if let Some(ai) = ai {
        return EpisodeMatch::Matched {
            episode_id: ai.episode_id,
            ep: ai.sort,
            sort: ai.sort,
            source: EpisodeSource::Ai,
        };
    }

    if episodes.is_empty() {
        return EpisodeMatch::NotFoundInDb {
            message: "数据库中没有章节列表".to_owned(),
        };
    }

    let Some(episode_number) = leading_number(&strip_episode_suffix(raw_episode)) else {
        return EpisodeMatch::NotFoundInDb {
            message: format!("AI 未能匹配且无法从标题解析出集数: {raw_episode}（需人工审核）"),
        };
    };

    let mut sorted: Vec<&EpisodeMeta> = episodes.iter().collect();
    sorted.sort_by(|a, b| {
        a.sort
            .partial_cmp(&b.sort)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });

    let Some(matched) = sorted
        .iter()
        .copied()
        .find(|episode| episode.is_main() && same_number(episode.sort, episode_number))
    else {
        return EpisodeMatch::NotFoundInDb {
            message: format!(
                "AI 未能匹配，自动匹配也未找到第 {} 集（当前季集数范围：{} 集，需人工审核）",
                format_number(episode_number),
                episode_range(&sorted)
            ),
        };
    };

    if let Some(airdate) = matched
        .airdate
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let Some(matched_date) = parse_airdate(airdate) else {
            return EpisodeMatch::InvalidInput {
                message: format!("无法解析匹配集的放送日期: {airdate}"),
            };
        };

        if let Some((earliest, latest)) = window.allowed_range() {
            if matched_date < earliest || matched_date > latest {
                let diff_days = if matched_date < earliest {
                    (earliest - matched_date).num_days()
                } else {
                    (matched_date - latest).num_days()
                };
                return EpisodeMatch::DateMismatch {
                    message: format!(
                        "放送日期不在允许区间内（{} ~ {}）: {}",
                        fmt_date(earliest),
                        fmt_date(latest),
                        fmt_date(matched_date)
                    ),
                    diff_days,
                    episode_id: matched.id,
                };
            }
        } else {
            let ahead = (matched_date - today).num_days();
            if ahead > MAX_FUTURE_DAYS {
                return EpisodeMatch::DateMismatch {
                    message: format!("放送日期在未来: {}", fmt_date(matched_date)),
                    diff_days: ahead,
                    episode_id: matched.id,
                };
            }
        }
    }

    EpisodeMatch::Matched {
        episode_id: matched.id,
        ep: matched.ep,
        sort: matched.sort,
        source: EpisodeSource::Auto,
    }
}

/// 去掉 `v2` / `END` 之后再看数字：`05v2` → `05`、`12END` → `12`。
fn strip_episode_suffix(text: &str) -> String {
    static VERSION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[vV]\d+").expect("内置正则必须合法"));
    static END: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)END").expect("内置正则必须合法"));
    let stripped = VERSION.replace_all(text, "");
    END.replace_all(&stripped, "").trim().to_owned()
}

/// 集数相等：`12` 与 `12.0` 必须算同一个。
fn same_number(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// 章节列表的主线集数范围，用于「找不到第 N 集」的提示。
fn episode_range(sorted: &[&EpisodeMeta]) -> String {
    let mains: Vec<f64> = sorted
        .iter()
        .filter(|episode| episode.is_main())
        .map(|episode| episode.sort)
        .collect();
    if mains.is_empty() {
        return "1".to_owned();
    }
    let min = mains.iter().copied().fold(f64::INFINITY, f64::min);
    let max = mains.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min = format_number(min);
    let max = format_number(max);
    if min == max {
        min
    } else {
        format!("{min}-{max}")
    }
}

/// 数字格式化：`5.0` → `5`，`12.5` → `12.5`（TS 模板字符串的行为）。
pub fn format_number(value: f64) -> String {
    if (value.fract()).abs() < 1e-9 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn fmt_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// 给日志用的 `NaiveDate` 生成（插件壳里通常就是 `Local::now().date_naive()`）。
pub fn today_local() -> NaiveDate {
    chrono::Local::now().date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(id: i64, sort: f64, airdate: &str) -> EpisodeMeta {
        EpisodeMeta {
            id,
            sort,
            ep: sort,
            airdate: Some(airdate.to_owned()),
            r#type: Some(0),
            ..Default::default()
        }
    }

    /// 12 集正片，2026-01-05 开播，每周一集（最后一集落在 2026-03-23）。
    fn season() -> Vec<EpisodeMeta> {
        let start = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        (1..=12)
            .map(|index| {
                let airdate = start + chrono::TimeDelta::days(i64::from(index - 1) * 7);
                episode(i64::from(100 + index), f64::from(index), &fmt_date(airdate))
            })
            .collect()
    }

    fn window() -> AiredWindow {
        AiredWindow::from_strings(Some("2026年1月5日"), Some("2026-03-23"))
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 25).unwrap()
    }

    #[test]
    fn leading_number_behaves_like_parse_float() {
        assert_eq!(leading_number("05"), Some(5.0));
        assert_eq!(leading_number("13.5"), Some(13.5));
        assert_eq!(leading_number(" 12 "), Some(12.0));
        assert_eq!(leading_number("SP01"), None);
        assert_eq!(leading_number("未知"), None);
    }

    #[test]
    fn parses_both_date_styles() {
        assert_eq!(
            parse_airdate("2026年1月16日"),
            NaiveDate::from_ymd_opt(2026, 1, 16)
        );
        assert_eq!(
            parse_airdate("2026-01-16"),
            NaiveDate::from_ymd_opt(2026, 1, 16)
        );
        assert_eq!(parse_airdate("2026-02-30"), None, "非法日期必须返回 None");
    }

    #[test]
    fn auto_match_by_sort() {
        let result = match_episode(&season(), Some("05"), &window(), today(), None);
        let EpisodeMatch::Matched {
            episode_id,
            ep,
            sort,
            source,
        } = result
        else {
            panic!("第 5 集应当命中：{result:?}");
        };
        assert_eq!((episode_id, ep, sort), (105, 5.0, 5.0));
        assert_eq!(source, EpisodeSource::Auto);
    }

    #[test]
    fn version_suffix_is_ignored() {
        let result = match_episode(&season(), Some("05v2"), &window(), today(), None);
        assert!(result.is_matched(), "{result:?}");
        assert_eq!(result.episode_id(), Some(105));
    }

    #[test]
    fn missing_episode_reports_the_range() {
        let result = match_episode(&season(), Some("13"), &window(), today(), None);
        let EpisodeMatch::NotFoundInDb { message } = result else {
            panic!("第 13 集不该命中：{result:?}");
        };
        assert!(message.contains("第 13 集"), "{message}");
        assert!(message.contains("1-12"), "{message}");
    }

    #[test]
    fn airdate_outside_window_is_a_mismatch() {
        let episodes = vec![episode(101, 1.0, "2026-06-01")];
        let window = AiredWindow::from_strings(Some("2026年1月5日"), Some("2026-01-05"));
        let result = match_episode(&episodes, Some("01"), &window, today(), None);
        let EpisodeMatch::DateMismatch {
            diff_days,
            episode_id,
            message,
        } = result
        else {
            panic!("应当判定为日期不符：{result:?}");
        };
        assert_eq!(diff_days, 117);
        assert_eq!(episode_id, 101);
        assert!(message.contains("放送日期不在允许区间内"), "{message}");
    }

    #[test]
    fn without_window_future_dates_are_rejected() {
        let far_future = today() + chrono::TimeDelta::days(30);
        let episodes = vec![episode(101, 1.0, &fmt_date(far_future))];
        let result = match_episode(
            &episodes,
            Some("01"),
            &AiredWindow::default(),
            today(),
            None,
        );
        let EpisodeMatch::DateMismatch { diff_days, .. } = result else {
            panic!("未来 30 天应当被拒：{result:?}");
        };
        assert_eq!(diff_days, 30);
    }

    #[test]
    fn specials_are_not_auto_matched() {
        let result = match_episode(&season(), Some("SP01"), &window(), today(), None);
        let EpisodeMatch::NotFoundInDb { message } = result else {
            panic!("特别篇应当交人工：{result:?}");
        };
        assert!(message.contains("无法从标题解析出集数"), "{message}");
    }

    #[test]
    fn ai_result_beats_auto_matching() {
        let result = match_episode(
            &season(),
            Some("05"),
            &window(),
            today(),
            Some(AiEpisode {
                episode_id: 999,
                sort: 7.0,
            }),
        );
        let EpisodeMatch::Matched {
            episode_id,
            sort,
            source,
            ..
        } = result
        else {
            panic!("AI 结果应当直接命中：{result:?}");
        };
        assert_eq!((episode_id, sort), (999, 7.0));
        assert_eq!(source, EpisodeSource::Ai);
    }

    #[test]
    fn missing_episode_string_is_invalid_input() {
        assert!(matches!(
            match_episode(&season(), None, &window(), today(), None),
            EpisodeMatch::InvalidInput { .. }
        ));
        assert!(matches!(
            match_episode(&season(), Some("   "), &window(), today(), None),
            EpisodeMatch::InvalidInput { .. }
        ));
    }

    #[test]
    fn non_main_episodes_are_skipped() {
        let mut episodes = season();
        // 把第 5 集改成 SP，正片里就没有 sort=5 了。
        episodes[4].r#type = Some(1);
        let result = match_episode(&episodes, Some("05"), &window(), today(), None);
        assert!(
            matches!(result, EpisodeMatch::NotFoundInDb { .. }),
            "{result:?}"
        );
    }
}
