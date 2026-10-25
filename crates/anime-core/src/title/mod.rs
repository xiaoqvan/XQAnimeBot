//! 标题处理：清洗、过滤、字幕组切分、番名/集数/来源解析。
//!
//! 入口是 [`parse_info`]——给它「一条 BT 标题 + 发布组名」，它吐出「番名候选 + 集数 + 来源」。
//! 解析不出来的字段不编造：`names` 为空就说明这条该跳过（白名单机制），
//! 集数兜底成 `未知` 交给下游人工审核。

pub mod episode;
pub mod rules;

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// 解析结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ParsedInfo {
    /// 番名候选，按匹配优先级排序（检索用 > 日文 > 中文 > 英文）。
    pub names: Vec<String>,
    /// 播放平台/片源（Baha、CR、Bilibili…），没有就是空串。
    pub source: String,
    /// 集数（`01` / `02v2` / `SP` / `剧场版` / `未知`）。
    pub episode: String,
}

impl ParsedInfo {
    /// 没有任何番名 → 这条不可用（白名单机制，宁可漏也不发错）。
    pub fn is_usable(&self) -> bool {
        self.names.iter().any(|n| !n.trim().is_empty())
    }
}

/// 解析一条 BT 标题。
///
/// 找不到发布组对应的规则时返回 `None`——TS 版同样直接 `return`，不猜。
pub fn parse_info(title: &str, team: Option<&str>) -> Option<ParsedInfo> {
    let team = team?;
    let rule = rules::rule_for(team)?;
    let mut names = rule(title);
    names.retain(|n| !n.trim().is_empty());
    let episode = episode::extract(title).unwrap_or_else(|| "未知".to_owned());
    Some(ParsedInfo {
        names,
        source: extract_source(title),
        episode,
    })
}

/// 取出标题里每个 `[]` / `【】` 的内容，按出现顺序。
///
/// 字幕组命名习惯不同，但「第几个括号里是什么」这件事是稳定的——
/// [`rules`] 里的每条规则都建立在这个函数之上。
pub fn brackets(title: &str) -> Vec<String> {
    bracket_spans(title)
        .into_iter()
        .map(|(start, end)| title[start..end].trim().to_owned())
        .collect()
}

/// 每个括号内容的**字节区间**（`[start, end)`），按出现顺序。
///
/// [`brackets`] 只要内容，集数解析要位置——两件事分开，但共用同一套扫描逻辑，
/// 避免「切一刀」和「找位置」对不上。
pub fn bracket_spans(title: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut open: Option<usize> = None;
    for (index, ch) in title.char_indices() {
        match ch {
            '[' | '【' => open = Some(index + ch.len_utf8()),
            ']' | '】' => {
                if let Some(start) = open.take()
                    && start <= index
                {
                    out.push((start, index));
                }
            }
            _ => {}
        }
    }
    out
}

/// 清洗标题：去零宽字符、把各种空白（含全角/不换行空格）压成单个半角空格。
pub fn clean_title(title: &str) -> String {
    static INVISIBLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[\u{200B}-\u{200F}\u{FEFF}]").expect("内置正则必须合法"));
    static SPACES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[\s\u{00A0}\u{1680}\u{180E}\u{2000}-\u{200A}\u{202F}\u{205F}\u{3000}]+")
            .expect("内置正则必须合法")
    });
    let without_invisible = INVISIBLE.replace_all(title, "");
    SPACES
        .replace_all(&without_invisible, " ")
        .trim()
        .to_owned()
}

/// 归一化标题，只用于**去重比较**（不用于展示）：全角转半角、去空格差异。
pub fn normalize_title(title: &str) -> String {
    static SYMBOLS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        [
            (r"／", "/"),
            (r"【", "["),
            (r"】", "]"),
            (r"（", "("),
            (r"）", ")"),
        ]
        .into_iter()
        .map(|(pattern, to)| (Regex::new(pattern).expect("内置正则必须合法"), to))
        .collect()
    });
    static INVISIBLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[\u{200B}-\u{200D}\u{FEFF}]").expect("内置正则必须合法"));
    static SPACED_SLASH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*/\s*").expect("内置正则必须合法"));
    static SPACED_DASH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*-\s*").expect("内置正则必须合法"));
    static SPACED_UNDERSCORE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*_\s*").expect("内置正则必须合法"));
    static OPEN_SPACE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[\s+").expect("内置正则必须合法"));
    static CLOSE_SPACE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s+\]").expect("内置正则必须合法"));
    static SPACES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s+").expect("内置正则必须合法"));

    let mut text = title.trim().to_lowercase();
    // 全角字母数字折成半角：`Ａ` 与 `a` 必须归一到同一个值，否则去重会漏。
    text = text
        .chars()
        .map(|ch| match ch as u32 {
            code @ 0xFF01..=0xFF5E => char::from_u32(code - 0xFEE0).unwrap_or(ch),
            _ => ch,
        })
        .collect();
    for (re, to) in SYMBOLS.iter() {
        text = re.replace_all(&text, *to).to_string();
    }
    text = INVISIBLE.replace_all(&text, "").to_string();
    text = SPACES.replace_all(&text, " ").to_string();
    text = SPACED_SLASH.replace_all(&text, "/").to_string();
    text = SPACED_DASH.replace_all(&text, "-").to_string();
    text = SPACED_UNDERSCORE.replace_all(&text, "_").to_string();
    text = OPEN_SPACE.replace_all(&text, "[").to_string();
    text = CLOSE_SPACE.replace_all(&text, "]").to_string();
    SPACES.replace_all(&text, " ").trim().to_owned()
}

/// 通用标题过滤：返回 `false` 表示这条不要。
///
/// 规则来自 TS 的 `isTitleAllowed`，顺序也照抄（先粗筛，再组内特例，最后集数区间）：
/// 排除内封/繁中/合集/无字幕、排除 720P（雪飄例外）与 Fin/MKV；
/// 幻樱只留 `GB_MP4`+`1920X1080`；带 `CHT` 标签的繁体版整条丢掉，但 `[ANi]`、
/// `[黒ネズミたち]` 两个组例外；`[1-12]` 这类区间打包也丢掉。
///
/// **这些是业务判断，不是技术过滤**，想改投放口味就改这里。
pub fn is_title_allowed(title: &str) -> bool {
    static WORD_FIN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bFin\b").expect("内置正则必须合法"));
    static WORD_MKV: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bMKV\b").expect("内置正则必须合法"));
    static CHT_TAG: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\[[^\]]*CHT[^\]]*\]").expect("内置正则必须合法"));
    static EPISODE_RANGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\[\d{1,3}-\d{1,3}\]|\(\d{1,3}-\d{1,3}\)").expect("内置正则必须合法")
    });

    if title.is_empty() {
        return false;
    }
    const DIRECT: [&str; 8] = [
        "内封",
        "繁",
        "合集",
        "無字幕",
        "粵語",
        "整理搬运",
        "無對白字幕",
        "BIG5",
    ];
    if DIRECT.iter().any(|s| title.contains(s)) {
        return false;
    }
    if !title.contains("雪飄工作室") && title.to_ascii_uppercase().contains("720P") {
        return false;
    }
    if WORD_FIN.is_match(title) || WORD_MKV.is_match(title) {
        return false;
    }
    // 幻樱字幕组：只留 GB 简体 + 1080P（它家 BIG5/720P 版本发得也多）。
    if title.contains("【幻樱字幕组】") {
        if title.contains("【BIG5_MP4】") || title.contains("【1280X720】") {
            return false;
        }
        if !title.contains("【GB_MP4】") || !title.contains("【1920X1080】") {
            return false;
        }
    }
    // 带 CHT 标签的繁体版：这两个组例外（它们 CHT 是内嵌简体，不是真繁体）。
    const CHT_BYPASS: [&str; 2] = ["[黒ネズミたち]", "[ANi]"];
    if CHT_TAG.is_match(title) && !CHT_BYPASS.iter().any(|group| title.contains(group)) {
        return false;
    }
    if EPISODE_RANGE.is_match(title) {
        return false;
    }
    true
}

/// 从标题里抽片源/平台标记（Baha / CR / Bilibili / Netflix …）。
///
/// 只做「认识就写、不认识就空」——TS 版同样是查表，不做猜测。
pub fn extract_source(title: &str) -> String {
    const SOURCES: [(&str, &str); 12] = [
        ("Baha", "Baha"),
        ("Bahamut", "Baha"),
        ("Crunchyroll", "CR"),
        ("B-Global", "B-Global"),
        ("Bilibili", "Bilibili"),
        ("哔哩哔哩", "Bilibili"),
        ("Netflix", "Netflix"),
        ("Disney+", "Disney+"),
        ("Amazon", "Amazon"),
        ("Abema", "Abema"),
        ("HIDIVE", "HIDIVE"),
        ("Viu", "Viu"),
    ];
    let upper = title.to_ascii_lowercase();
    for (needle, source) in SOURCES {
        if upper.contains(&needle.to_ascii_lowercase()) {
            return source.to_owned();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brackets_keeps_order_and_handles_both_styles() {
        let blocks = brackets("[喵萌奶茶屋] 葬送的芙莉莲【01】【1080P】");
        assert_eq!(blocks, vec!["喵萌奶茶屋", "01", "1080P"]);
    }

    #[test]
    fn clean_title_collapses_weird_spaces() {
        let cleaned = clean_title("  \u{3000}葬送\u{200b}的  芙莉莲 \u{00A0}");
        assert_eq!(cleaned, "葬送的 芙莉莲");
    }

    #[test]
    fn normalize_is_for_comparison_only() {
        assert_eq!(
            normalize_title("Ａ／B 【01】"),
            normalize_title("a / b [01]")
        );
    }

    #[test]
    fn filter_rejects_inner_sub_and_batch() {
        assert!(!is_title_allowed("[喵萌奶茶屋] 某番 [01][1080P][内封]"));
        assert!(!is_title_allowed("[喵萌奶茶屋] 某番 [01-12][合集]"));
        assert!(is_title_allowed("[喵萌奶茶屋] 某番 [01][1080P]"));
    }

    #[test]
    fn filter_drops_cht_unless_group_is_bypassed() {
        assert!(!is_title_allowed("[喵萌奶茶屋] 某番 [05][1080P][CHT]"));
        assert!(is_title_allowed("[ANi] 某番 [05][1080P][CHT]"));
        assert!(is_title_allowed("[黒ネズミたち] 某番 [05][1080P][CHT]"));
    }

    #[test]
    fn filter_handles_huan_ying_and_episode_range() {
        assert!(!is_title_allowed(
            "【幻樱字幕组】【BIG5_MP4】【1280X720】某番【05】"
        ));
        assert!(!is_title_allowed(
            "【幻樱字幕组】【GB_MP4】【1280X720】某番【05】"
        ));
        assert!(is_title_allowed(
            "【幻樱字幕组】【GB_MP4】【1920X1080】某番【05】"
        ));
        assert!(!is_title_allowed("[喵萌奶茶屋] 某番 [1-12][1080P]"));
        assert!(is_title_allowed("[喵萌奶茶屋] 某番 [05][1080P]"));
    }

    #[test]
    fn source_is_recognized() {
        assert_eq!(extract_source("[ANi] 某番 [01][1080P][Baha]"), "Baha");
        assert_eq!(extract_source("[ANi] 某番 [01][1080P]"), "");
    }

    #[test]
    fn unknown_team_returns_none() {
        assert!(parse_info("[没听过的组] 某番 [01]", Some("没听过的组")).is_none());
    }
}
