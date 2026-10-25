//! 三源共用的时间戳与「这条要不要」判断。
//!
//! 时间格式是 TS 版直接 `toString` 拼出来的中文串（`2026年10月25日 09:05PM`），
//! 这个串会原样写进 MongoDB、也会出现在 Telegram 文案里，所以**必须逐字对齐**，
//! 包括「0 点写成 12 点」和「小时补零到两位」这两个容易被忽略的细节。
//!
//! 时区上有分歧，照抄 TS：
//! - bangumi.moe 的 `pubDate` 是 UTC，TS 手动 +8 小时后取 UTC 字段 → [`format_pub_date_utc8`]；
//! - dmhy / acgnx 直接用本地时区的 `getHours()` → [`format_pub_date_local`]。

use std::sync::LazyLock;

use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDateTime, TimeZone, Timelike};
use regex::Regex;

/// 北京时间（UTC+8）。bangumi.moe 用这个，不依赖机器时区。
fn beijing() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("UTC+8 永远合法")
}

/// 把源里的时间串解析成带时区的时间。
///
/// 依次尝试 RFC 2822（RSS 标准）、RFC 3339、几种常见的裸时间格式、中文格式。
/// 全都失败返回 `None`——调用方决定是「保留原文」还是「当成最旧」。
pub fn parse_datetime(raw: &str) -> Option<DateTime<FixedOffset>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc2822(raw) {
        return Some(parsed);
    }
    // RFC 2822 会校验「星期与日期是否相符」。源里星期写错（或干脆不写）时降级：
    // 去掉星期前缀再按 `25 Oct 2026 13:00:00 +0000` 解析。
    if let Some((_weekday, rest)) = raw.split_once(',')
        && let Ok(parsed) = DateTime::parse_from_str(rest.trim(), "%d %b %Y %H:%M:%S %z")
    {
        return Some(parsed);
    }
    if let Ok(parsed) = DateTime::parse_from_str(raw, "%d %b %Y %H:%M:%S %z") {
        return Some(parsed);
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        return Some(parsed);
    }
    // 已经是我们自己产出的中文格式（MongoDB 里读回来时会遇到）。
    if let Some(parsed) = parse_chinese(raw) {
        return Some(parsed);
    }
    for format in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y/%m/%d %H:%M:%S",
        "%Y/%m/%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, format) {
            return naive.and_local_timezone(beijing()).single();
        }
    }
    None
}

/// 中文格式 `2026年10月25日 09:05PM`（TS 版自己拼出来的那种）。
pub fn parse_chinese(raw: &str) -> Option<DateTime<FixedOffset>> {
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(\d{4})年(\d{2})月(\d{2})日\s+(\d{1,2}):(\d{2})(AM|PM)")
            .expect("内置正则必须合法")
    });
    let caps = PATTERN.captures(raw)?;
    let number = |index: usize| -> Option<u32> { caps.get(index)?.as_str().parse().ok() };
    let year = number(1)? as i32;
    let month = number(2)?;
    let day = number(3)?;
    let hour = number(4)?;
    let minute = number(5)?;
    let hour24 = match &caps[6] {
        "PM" if hour != 12 => hour + 12,
        "AM" if hour == 12 => 0,
        _ => hour,
    };
    beijing()
        .with_ymd_and_hms(year, month, day, hour24.min(23), minute.min(59), 0)
        .single()
}

/// bangumi.moe：先按 UTC 解析，再 +8 小时，输出中文格式。
///
/// 解析不了就原样返回（TS 版也是 `return pubDateString`，并且打一条 warn）。
pub fn format_pub_date_utc8(raw: &str) -> String {
    match parse_datetime(raw) {
        Some(parsed) => stamp(parsed.with_timezone(&beijing())),
        None => raw.trim().to_owned(),
    }
}

/// dmhy / acgnx：按**本地时区**取字段（TS 版就是这么写的，跨时区会不一样，故意保留）。
pub fn format_pub_date_local(raw: &str) -> String {
    match parse_datetime(raw) {
        Some(parsed) => stamp(parsed.with_timezone(&Local).fixed_offset()),
        None => raw.trim().to_owned(),
    }
}

/// 排序用的毫秒时间戳。解析不了给 0（等价于「最旧」），不 panic。
pub fn parse_pub_date_ms(raw: &str) -> i64 {
    parse_datetime(raw)
        .map(|parsed| parsed.timestamp_millis())
        .unwrap_or(0)
}

/// `2026年10月25日 09:05PM`
fn stamp(at: DateTime<FixedOffset>) -> String {
    let hour = at.hour();
    let ampm = if hour >= 12 { "PM" } else { "AM" };
    let hour12 = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    format!(
        "{}年{:02}月{:02}日 {:02}:{:02}{}",
        at.year(),
        at.month(),
        at.day(),
        hour12,
        at.minute(),
        ampm
    )
}

/// 完结/合集标记。命中就跳过——合集整季几十 GB，不是我们要的单集流。
pub fn is_fin_collection_title(title: &str) -> bool {
    static BRACKETED_FIN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\[[^\]]*fin[^\]]*\]").expect("内置正则必须合法"));
    let lower = title.to_lowercase();
    if !lower.contains("fin") {
        return false;
    }
    BRACKETED_FIN.is_match(title) || lower.contains("合集") || lower.ends_with("fin")
}

/// 多集打包：`[01-12]` / `[1~12]`，或「12集 全集」这类写法。
pub fn is_multi_episode_collection_title(title: &str) -> bool {
    static BRACKETED_RANGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\[[^\]]*\d{1,3}\s*[-~–—]\s*\d{1,3}[^\]]*\]").expect("内置正则必须合法")
    });
    static SEQUENCE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d{1,3}[集话卷]").expect("内置正则必须合法"));
    static COLLECTION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"合集|全集").expect("内置正则必须合法"));

    if BRACKETED_RANGE.is_match(title) {
        return true;
    }
    let lower = title.to_lowercase();
    SEQUENCE.is_match(&lower) && COLLECTION.is_match(&lower)
}

/// 不要的字幕形态：繁体内嵌、外挂、繁日双语…
pub fn contains_disallowed_subtitle_keyword(title: &str) -> bool {
    const KEYWORDS: [&str; 5] = ["繁體內嵌", "外挂", "繁日字幕", "繁日雙語", "繁體"];
    let normalized: String = title
        .to_lowercase()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    KEYWORDS
        .iter()
        .any(|keyword| normalized.contains(&keyword.to_lowercase()))
}

/// 合并前的统一判断：给一个理由就跳过。
///
/// 返回 `Some(原因)` 而不是 `bool`，是为了日志能说清「为什么没发这条」——
/// 排查「番剧没更新」时全靠它。
pub fn skip_reason<'a>(title: &str, blacklist: &'a [String]) -> Option<&'a str> {
    if blacklist
        .iter()
        .map(|name| name.trim())
        .any(|name| !name.is_empty() && title.contains(name))
    {
        return Some("命中黑名单");
    }
    if is_fin_collection_title(title) {
        return Some("合集/Fin 标记");
    }
    if is_multi_episode_collection_title(title) {
        return Some("多集打包");
    }
    if contains_disallowed_subtitle_keyword(title) {
        return Some("字幕形态不支持");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc2822_converts_to_beijing_time() {
        // 13:00 UTC → 21:00 北京时间。
        assert_eq!(
            format_pub_date_utc8("Sun, 25 Oct 2026 13:00:00 +0000"),
            "2026年10月25日 09:00PM"
        );
    }

    #[test]
    fn midnight_renders_as_twelve_am() {
        // TS 的 `hours ? hours : 12`：0 点写 12 点，这点照抄。
        assert_eq!(
            format_pub_date_utc8("Sun, 25 Oct 2026 16:05:00 +0000"),
            "2026年10月26日 12:05AM"
        );
    }

    #[test]
    fn unparsable_date_keeps_original_text() {
        assert_eq!(format_pub_date_utc8("昨天"), "昨天");
        assert_eq!(parse_pub_date_ms("昨天"), 0);
    }

    #[test]
    fn chinese_stamp_round_trips() {
        let parsed = parse_datetime("2026年10月25日 09:05PM").unwrap();
        assert_eq!(
            parsed.timestamp(),
            parse_datetime("2026-10-25T21:05:00+08:00")
                .unwrap()
                .timestamp()
        );
    }

    #[test]
    fn collection_and_fin_are_rejected() {
        assert!(is_fin_collection_title("[ANi] 某番 [01-12][Fin]"));
        // `Fin` 判定要求标题里真的有 fin 子串（TS 版先做这个短路），
        // 光有「全集」是 `is_title_allowed` 的「合集」规则负责的，别混。
        assert!(!is_fin_collection_title("某番 全集"));
        assert!(is_fin_collection_title("[喵萌奶茶屋] 某番 Fin"));
        assert!(!is_fin_collection_title("[ANi] Finding Carter [05]"));
        assert!(is_multi_episode_collection_title(
            "[ANi] 某番 [01-12][1080P]"
        ));
        assert!(is_multi_episode_collection_title("[ANi] 某番 12集 全集"));
        assert!(!is_multi_episode_collection_title("[ANi] 某番 [05][1080P]"));
    }

    #[test]
    fn subtitle_keywords_ignore_spaces() {
        assert!(contains_disallowed_subtitle_keyword(
            "[ANi] 某番 [05][繁 體]"
        ));
        assert!(contains_disallowed_subtitle_keyword(
            "[ANi] 某番 [05][外挂字幕]"
        ));
        assert!(!contains_disallowed_subtitle_keyword(
            "[ANi] 某番 [05][简体内嵌]"
        ));
    }

    #[test]
    fn skip_reason_reports_the_actual_cause() {
        let blacklist = vec!["不想看的番".to_owned()];
        assert_eq!(
            skip_reason("不想看的番 [05]", &blacklist),
            Some("命中黑名单")
        );
        assert_eq!(
            skip_reason("[ANi] 某番 [01-12]", &blacklist),
            Some("多集打包")
        );
        assert_eq!(skip_reason("[ANi] 某番 [05]", &blacklist), None);
        // 空黑名单项不能把一切都拦掉。
        assert_eq!(skip_reason("[ANi] 某番 [05]", &[String::new()]), None);
    }
}
