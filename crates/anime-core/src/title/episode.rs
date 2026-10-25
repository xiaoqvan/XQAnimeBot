//! 从 BT 标题里抠集数。
//!
//! 输入千奇百怪：`[01]`、`[02v2]`、`[03_卢恩城]`、`【14】`、` - 02v2`、`第13话`、
//! `[OVA2]`、`48.5`、`[01-12]`……TS 版用一串「先试这个、不行试那个」的正则级联，
//! 这里保持同样的顺序——**顺序本身就是规则**，换个先后就可能在别名里抠出个假集数。
//!
//! Rust 的 `regex` 不支持 lookahead，所以 TS 里 `\[(\d+)\](?![^[]*\[)`
//! （「最后一个不带后缀的集数括号」）改成了显式的「从右往左找第一个纯数字括号」，
//! 语义等价，还少一次回溯。

use std::sync::LazyLock;

use regex::Regex;

/// 提取集数；`None` 表示标题里没有可信的集数信息。
pub fn extract(title: &str) -> Option<String> {
    if let Some(range) = extract_range(title) {
        return Some(range);
    }
    for (pattern, group) in CANDIDATES.iter() {
        let re = cached(pattern);
        if let Some(caps) = re.captures(title)
            && let Some(m) = caps.get(*group)
        {
            let value = m.as_str().trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    last_number_bracket(title).or_else(|| extract_special(title))
}

/// 集数区间（`[01-12]` / `(01v2-12v2)`）→ 展开成 `01,02,…,12`。
///
/// TS 版这里其实被后面一行 `episode = epMatch?.[1]` 覆盖掉了（区间信息丢失，
/// 最终落到「未知」）。这里保留展开结果：区间标题进到下游时至少还有集数可用。
pub fn extract_range(title: &str) -> Option<String> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"\[(\d{1,3}(?:v\d+)?)-(\d{1,3}(?:v\d+)?)\]|\((\d{1,3}(?:v\d+)?)-(\d{1,3}(?:v\d+)?)\)",
        )
        .expect("内置正则必须合法")
    });
    let caps = RE.captures(title)?;
    let start = caps.get(1).or_else(|| caps.get(3))?.as_str();
    let end = caps.get(2).or_else(|| caps.get(4))?.as_str();
    let (start_text, start_ver) = split_version(start);
    let (end_text, end_ver) = split_version(end);
    let start_num: u32 = start_text.parse().ok()?;
    let end_num: u32 = end_text.parse().ok()?;
    if end_num < start_num || end_num - start_num > 200 {
        return None;
    }
    // 补零宽度只看数字部分：`01v2` 的宽度是 2，不是 4。
    let width = start_text.len().clamp(1, 3);
    let mut out: Vec<String> = Vec::with_capacity((end_num - start_num + 1) as usize);
    for n in start_num..=end_num {
        let num = format!("{n:0width$}");
        let ver = if n == start_num {
            start_ver
        } else if n == end_num {
            end_ver
        } else {
            ""
        };
        out.push(format!("{num}{ver}"));
    }
    Some(out.join(","))
}

fn split_version(token: &str) -> (&str, &str) {
    match token.find(['v', 'V']) {
        Some(idx) => (&token[..idx], &token[idx..]),
        None => (token, ""),
    }
}

/// 级联顺序与 TS 版一致：越靠前的越可信。
const CANDIDATES: &[(&str, usize)] = &[
    (r"\[(\d{1,3})_", 1),
    (r"\[(\d{1,3}(?:v\d+)?)\s+[^\]]+\]", 1),
    (r"\[(\d{1,3})\s*-\s*总第\d+", 1),
    (r"【(\d{1,3}(?:v\d+)?)】", 1),
    (r"\s-\s(\d{1,3}(?:v\d+)?)(?:\s|$|\(|\[|【)", 1),
    (r"\s-\s(\d{1,3})(?:\s|$|\()", 1),
    (r"(?i)(?:第|EP)(\d{1,3}(?:v\d+)?)(?:话|集|話)", 1),
];

fn cached(pattern: &'static str) -> &'static Regex {
    // 数量固定（7 条），用一个惰性表缓存，避免每处理一条 RSS 就编译一遍。
    static CACHE: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
        CANDIDATES
            .iter()
            .map(|(p, _)| (*p, Regex::new(p).expect("内置正则必须合法")))
            .collect()
    });
    &CACHE
        .iter()
        .find(|(p, _)| *p == pattern)
        .expect("模式一定在表里")
        .1
}

/// 从右往左找第一个「内容就是集数」的方括号。
///
/// 从右往左是刻意的：`[葬送的芙莉莲][01][1080P]` 里左边的括号可能是番名的一部分，
/// 右边的才是集数。
fn last_number_bracket(title: &str) -> Option<String> {
    static NUM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\d{1,3}(?:v\d+)?$").expect("内置正则必须合法"));
    let mut best: Option<String> = None;
    for (start, end) in super::bracket_spans(title) {
        let content = &title[start..end];
        if NUM.is_match(content) {
            best = Some(content.to_owned());
        }
    }
    best
}

/// 没有数字集数时的特殊集：剧场版 / SP / 电影 / OVA 带编号 / 小数集。
fn extract_special(title: &str) -> Option<String> {
    static MOVIE_COMP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(?:剧场总集篇|劇場總集篇)").expect("内置正则必须合法"));
    static MOVIE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:剧场版|劇場版|Gekijouban|Eiga|Movie)").expect("内置正则必须合法")
    });
    static FILM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:电影|電影)").expect("内置正则必须合法"));
    static SPECIAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:特别篇|特別篇|Special|SP)").expect("内置正则必须合法")
    });
    static OVA: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:OVA|OAD|SP|Extra|番外|特典)[\s\-:]?(\d{1,3})")
            .expect("内置正则必须合法")
    });
    static OVA_BRACKET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\[(OVA|OAD|SP|Extra|番外|特典)[\s\-:]?(\d{1,3})\]")
            .expect("内置正则必须合法")
    });
    static DECIMAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[\s\-\[]?(\d{1,3}\.\d)[\s\)\]]").expect("内置正则必须合法"));

    if MOVIE_COMP.is_match(title) {
        return Some("剧场总集篇".to_owned());
    }
    if MOVIE.is_match(title) {
        return Some("剧场版".to_owned());
    }
    if FILM.is_match(title) {
        return Some("电影".to_owned());
    }
    if SPECIAL.is_match(title) {
        return Some("SP".to_owned());
    }
    if let Some(caps) = OVA_BRACKET.captures(title) {
        return Some(format!("{}{}", &caps[1], &caps[2]));
    }
    if let Some(caps) = OVA.captures(title) {
        // `[OVA2]` 这种取整体（`OVA2`），和 TS 的 `specialEp[0]` 一致。
        return Some(caps.get(0)?.as_str().trim().to_owned());
    }
    DECIMAL.captures(title).map(|c| c[1].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_bracket_episode() {
        assert_eq!(extract("[ANi] 芙莉莲 - 01 [1080P]").as_deref(), Some("01"));
    }

    #[test]
    fn versioned_episode_keeps_version() {
        assert_eq!(
            extract("【喵萌奶茶屋】[芙莉莲][02v2][1080P]").as_deref(),
            Some("02v2")
        );
    }

    #[test]
    fn picks_the_right_bracket_not_the_name() {
        // `[葬送的芙莉莲]` 是番名，真正的集数在右边。
        assert_eq!(
            extract("[桜都字幕组][葬送的芙莉莲][03][1080P]").as_deref(),
            Some("03")
        );
    }

    #[test]
    fn underscore_and_chinese_forms() {
        assert_eq!(extract("[组][番名][03_卢恩城]").as_deref(), Some("03"));
        assert_eq!(extract("【组】【番名】【14】").as_deref(), Some("14"));
        assert_eq!(extract("某番 第13话 1080P").as_deref(), Some("13"));
    }

    #[test]
    fn dash_form() {
        assert_eq!(extract("[组] 番名 - 02v2 [1080P]").as_deref(), Some("02v2"));
        assert_eq!(extract("[组] 番名 - 7").as_deref(), Some("7"));
    }

    #[test]
    fn ranges_expand() {
        assert_eq!(
            extract("[组] 番名 [01-03][1080P]").as_deref(),
            Some("01,02,03")
        );
        assert_eq!(
            extract("[组] 番名 [01v2-02v2]").as_deref(),
            Some("01v2,02v2")
        );
    }

    #[test]
    fn specials_without_numbers() {
        assert_eq!(
            extract("[组] 番名 剧场版 [1080P]").as_deref(),
            Some("剧场版")
        );
        assert_eq!(extract("[组] 番名 [OVA2][1080P]").as_deref(), Some("OVA2"));
        assert_eq!(extract("[组] 番名 48.5 [1080P]").as_deref(), Some("48.5"));
        assert_eq!(extract("[组] 番名 [SP][1080P]").as_deref(), Some("SP"));
    }

    #[test]
    fn unknown_episode_is_none() {
        assert_eq!(extract("[组] 番名 无集数标记"), None);
    }
}
