//! 番剧卡出图：把 [BangumiCard] 交给 image.render，拿回图片路径。
//!
//! 模板随插件打包（`include_str!`），不依赖部署目录：出图插件的 `template_dir`
//! 是调用方传的，如果这里再依赖一个外部路径，装插件时就要多配一项，且配错只会在
//! 运行时炸。直接喂完整 HTML 最稳。
//!
//! 评分表情是 5 格雪碧图，跟着卡片一起发；没有它模板会少一块，但不会崩。

use std::path::PathBuf;

use anime_core::card::bangumi::BangumiCard;
use anime_core::{Error, Result};
use fuyubot_service::call_service;
use serde_json::json;

use crate::runtime::Runtime;

/// 番剧卡模板（1000×550）。纯 HTML 模板，字段占位符由出图插件渲染前绑定。
const TEMPLATE: &str = include_str!("../assets/templates/bangumi-card.html");
/// 5 格评分表情雪碧图。
pub const RATING_EMOJI: &[u8] = include_bytes!("../assets/emoji/rate_emo.gif");

/// 按评分挑表情格（0..=4），阈值与 TS 的 `ratingEmojiIdx` 一致。
pub fn emoji_index(score: f32) -> usize {
    if score >= 8.5 {
        4
    } else if score >= 6.5 {
        3
    } else if score >= 5.5 {
        2
    } else if score >= 3.5 {
        1
    } else {
        0
    }
}

/// 渲染一张卡片，返回图片落盘路径。
///
/// 输出目录用系统临时目录：卡片是「发完即弃」的中间产物，不该混进工作目录。
pub fn render(runtime: &Runtime, card: &BangumiCard) -> Result<PathBuf> {
    let out = std::env::temp_dir().join(format!("xq-card-{}.jpg", card.id));
    let payload = card.to_html_payload();
    // html + data：出图插件渲染前把 payload 绑进模板占位符，页面里没有 JS。
    call_service(
        &runtime.host,
        "image.render",
        "render",
        &json!({
            "html": TEMPLATE,
            "data": payload,
            "out": out.to_string_lossy(),
            "width": 1000,
            "height": 550,
            "scale": 2.0,
            "format": "jpeg",
        }),
    )
    .map_err(|message| Error::Service {
        service: "image.render".to_owned(),
        message,
    })?;
    Ok(out)
}

/// 补上出图用到的本地素材（表情图 + 表情下标）。
pub fn with_emoji(mut card: BangumiCard) -> BangumiCard {
    card.rating_emoji_idx = emoji_index(card.score);
    card.rating_emoji = Some(RATING_EMOJI.to_vec());
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_thresholds_match_ts() {
        assert_eq!(emoji_index(9.0), 4);
        assert_eq!(emoji_index(8.5), 4);
        assert_eq!(emoji_index(7.0), 3);
        assert_eq!(emoji_index(6.0), 2);
        assert_eq!(emoji_index(4.0), 1);
        assert_eq!(emoji_index(1.0), 0);
    }

    #[test]
    fn template_is_plain_html_with_data_bindings() {
        // 模板里必须有数据绑定点，否则渲染出来是一张静态图；
        // 也绝不能再出现 Vue 运行时或早期的注入占位符。
        assert!(TEMPLATE.contains("{{ title }}"));
        assert!(TEMPLATE.contains("{{#each pages}}"));
        assert!(!TEMPLATE.contains("__IMAGE_DATA__"));
        assert!(!TEMPLATE.contains("__VUE_RUNTIME__"));
        assert!(!TEMPLATE.contains("Vue."));
    }
}
