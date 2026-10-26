//! `/start <payload>` 的深链（TS `cmd/start.ts`）。
//!
//! 三种载荷：
//!
//! ```text
//! bgm_cb-<code>_<state>   OAuth 回调：code 换 token（`/bindbangumi` 发的授权链接）
//! collection-<animeId>    把条目标成「在看」，再给四个改状态的按钮
//! eplook-<episodeId>      把章节标成「看过」
//! ```
//!
//! 分隔规则照 TS：`-` 之前是动作，之后按 `_` 切成参数表。
//! **认不出的载荷一律不回复**——`/start` 是 Telegram 里最常见的命令，
//! 宿主还有自己的 start 插件，抢着回一句「无法识别」会很吵。

/// Bangumi 授权码是 40 位十六进制（SHA1 长度）。TS 的 `base62ToHex` 会补齐到这个长度。
const CODE_HEX_LEN: usize = 40;

/// 一条 `/start` 深链。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepLink {
    pub action: Action,
    /// `-` 之后按 `_` 切开的参数。
    pub values: Vec<String>,
}

/// 认得出的三种动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// OAuth 回调（`bgm_cb`）。
    BangumiCallback,
    /// 收藏条目（`collection`）。
    Collection,
    /// 标记章节看过（`eplook`）。
    EpisodeWatched,
}

/// 解析 `/start` 后面那一段。不是我们认识的三种就 `None`。
pub fn parse(payload: &str) -> Option<DeepLink> {
    let payload = payload.trim();
    if payload.is_empty() {
        return None;
    }
    // TS：`payload.indexOf("-")`，没有 `-` 时整个 payload 就是 action。
    let (action, rest) = match payload.split_once('-') {
        Some((action, rest)) => (action, Some(rest)),
        None => (payload, None),
    };
    let action = match action {
        "bgm_cb" => Action::BangumiCallback,
        "collection" => Action::Collection,
        "eplook" => Action::EpisodeWatched,
        _ => return None,
    };
    let values = match rest {
        Some(rest) if !rest.is_empty() => rest.split('_').map(str::to_owned).collect(),
        _ => Vec::new(),
    };
    Some(DeepLink { action, values })
}

/// base62 → 十六进制（TS `utils/base62ToHex.ts`）。
///
/// Bangumi 授权页回传的 `code` 是 base62 的，而换 token 的接口要十六进制。
/// 值可能超过 160 位（`u128` 放不下），所以按 **base-256 大端数组**逐位乘 62 加，
/// 而不是先转成整数。
///
/// 非法字符返回 `None`（TS 是抛异常，这里传进来的都是用户点链接带的东西，
/// 不该因为一串乱码把命令处理线程打挂）。
pub fn base62_to_hex(input: &str) -> Option<String> {
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    // 大端 base-256 累加器。`vec![0]` 是「值为 0」，省掉空数组的特判。
    let mut digits: Vec<u8> = vec![0];
    for byte in input.bytes() {
        let index = CHARS.iter().position(|&known| known == byte)? as u32;
        let mut carry = index;
        for digit in digits.iter_mut().rev() {
            let value = (*digit as u32) * 62 + carry;
            *digit = (value & 0xff) as u8;
            carry = value >> 8;
        }
        while carry > 0 {
            digits.insert(0, (carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    let start = digits
        .iter()
        .position(|&digit| digit != 0)
        .unwrap_or(digits.len());
    let mut hex: String = digits[start..].iter().map(|digit| format!("{digit:02x}")).collect();
    if hex.is_empty() {
        hex.push('0');
    }
    // 与 TS 同序：先补齐到 40 位，再补成偶数长度。
    if hex.len() < CODE_HEX_LEN {
        hex = "0".repeat(CODE_HEX_LEN - hex.len()) + &hex;
    }
    if hex.len() % 2 == 1 {
        hex.insert(0, '0');
    }
    Some(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_actions() {
        assert_eq!(
            // 动作与参数表用 `-` 分，参数之间用 `_` 分（TS 同规则）。
            parse("bgm_cb-abc_42"),
            Some(DeepLink {
                action: Action::BangumiCallback,
                values: vec!["abc".to_owned(), "42".to_owned()],
            })
        );
        assert_eq!(
            parse("collection-123"),
            Some(DeepLink {
                action: Action::Collection,
                values: vec!["123".to_owned()],
            })
        );
        assert_eq!(
            parse("eplook-9"),
            Some(DeepLink {
                action: Action::EpisodeWatched,
                values: vec!["9".to_owned()],
            })
        );
    }

    #[test]
    fn unknown_or_empty_payloads_are_ignored() {
        // 宿主还有自己的 start 插件：认不出就别抢着回话。
        assert_eq!(parse(""), None);
        assert_eq!(parse("something-else"), None);
        // 没有 `-` 时整个 payload 就是 action：`bgm_cb` 认得出（只是没参数），
        // 所以它**不是**「不认识的载荷」——下面那条断言专门盯这个区别。
        assert_eq!(
            parse("collection"),
            Some(DeepLink {
                action: Action::Collection,
                values: Vec::new(),
            })
        );
        assert_eq!(
            parse("bgm_cb"),
            Some(DeepLink {
                action: Action::BangumiCallback,
                values: Vec::new(),
            })
        );
    }

    #[test]
    fn base62_round_trips_small_values() {
        assert_eq!(base62_to_hex("0").as_deref(), Some("0".repeat(40).as_str()));
        // 62 = 0x3e
        assert!(base62_to_hex("10").unwrap().ends_with("3e"));
        // 61 = 0x3d（base62 里 'z' 是 35，'A' 是 36…这里用 'Z'=61）
        assert!(base62_to_hex("Z").unwrap().ends_with("3d"));
    }

    #[test]
    fn base62_pads_to_forty_hex_chars_and_keeps_them_even() {
        let hex = base62_to_hex("1").unwrap();
        assert_eq!(hex.len(), 40);
        assert!(hex.ends_with('1'));
    }

    #[test]
    fn base62_handles_values_wider_than_u128() {
        // 27 个 'Z'（每位 61）远超 128 位：这条同时验证没有走整数溢出。
        let hex = base62_to_hex(&"Z".repeat(27)).unwrap();
        assert!(hex.len() > 32, "{hex}");
        assert_eq!(hex.len() % 2, 0);
        assert!(hex.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn base62_rejects_foreign_characters() {
        assert_eq!(base62_to_hex("ab!"), None);
        assert_eq!(base62_to_hex("中文"), None);
    }
}
