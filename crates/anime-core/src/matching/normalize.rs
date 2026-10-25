//! 匹配用的字符串归一化。**只用于比较，不用于展示**——归一化后的字符串会把
//! 全角字符、空白、大小写全部抹平，拿去做标题或日志就不可读了。
//!
//! [`normalize_for_match`] 逐条对照 TS 的 `normalizeForMatch`（`database/query.ts`）：
//! 去首尾空白 → 全角转半角 → 全角空格转半角 → 合并连续空白 → 转小写。
//! 顺序不能换：先去空白再全角转换，`\u{3000}` 会先被当空白吃掉，得到的结果一样，
//! 但 `　`（全角空格）夹在两个词中间时行为会不同，所以照抄 TS 的顺序。

/// 归一化：全角转半角、空白合并、小写。
///
/// ```
/// # use anime_core::matching::normalize_for_match;
/// assert_eq!(normalize_for_match("  ＡＢＣ　葬送的 芙莉莲 "), "abc 葬送的 芙莉莲");
/// ```
pub fn normalize_for_match(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // 空白不是立刻落盘，而是记一个「待补空格」标记：只有后面真的再来一个字符
    // 才补，这样末尾空白和连续空白都自然消失。
    let mut pending_space = false;
    for ch in text.trim().chars() {
        let ch = match ch as u32 {
            // 全角 ASCII（！~）折成半角：`Ａ` 与 `a` 必须归一。
            code @ 0xFF01..=0xFF5E => char::from_u32(code - 0xFEE0).unwrap_or(ch),
            // 表意空格 U+3000，`char::is_whitespace` 也认，但显式写出来更清楚。
            0x3000 => ' ',
            _ => ch,
        };
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        // 小写可能一扩多（`İ` → `i` + 组合点），所以按 char 展开而不是 `to_lowercase` 整串。
        out.extend(ch.to_lowercase());
    }
    out
}

/// 归一化后再删掉**所有**空白。
///
/// 中文番名里插空格是常态（`葬送的 芙莉莲`、`葬送的芙莉莲 第二季`），归一化只把
/// 连续空白压成一个，两侧仍然不等；多这一层才能判定「同一个名字」。
pub fn compact_for_match(text: &str) -> String {
    normalize_for_match(text)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullwidth_and_case_are_folded() {
        assert_eq!(normalize_for_match("ＳＰＹ×ＦＡＭＩＬＹ"), "spy×family");
        assert_eq!(normalize_for_match("Spy X Family"), "spy x family");
    }

    #[test]
    fn ideographic_space_becomes_a_single_space() {
        assert_eq!(normalize_for_match("葬送的\u{3000}芙莉莲"), "葬送的 芙莉莲");
    }

    #[test]
    fn whitespace_collapses_and_trims() {
        assert_eq!(
            normalize_for_match("  \u{3000}葬送 \t\n 的  芙莉莲  "),
            "葬送 的 芙莉莲"
        );
        assert_eq!(normalize_for_match("   "), "");
        assert_eq!(normalize_for_match(""), "");
    }

    #[test]
    fn compact_ignores_every_space() {
        assert_eq!(compact_for_match(" 葬送的 芙莉莲 "), "葬送的芙莉莲");
        assert_eq!(
            compact_for_match("葬送的芙莉莲"),
            compact_for_match("葬送的\u{3000}芙莉莲")
        );
    }
}
