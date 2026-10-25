//! Bangumi 导航封面卡（1000×550）的**数据**部分。
//!
//! 只做一件事：把 Bangumi API + 本地库的数据拼成 `bangumi-card.html` 认识的 payload。
//! 排版在 HTML 模板里（`assets/templates/bangumi-card.html`），渲染在出图插件里，
//! 本模块连像素都不碰。

use serde_json::json;

use super::content_hash_of;
use super::image::{cover_jpeg, data_uri};

/// 本篇一页放 30 集（10 列 × 3 行），与模板里的 `episodesPerPage` 必须一致。
pub const EPISODES_PER_PAGE: usize = 30;

#[derive(Debug, Clone, Default)]
pub struct Episode {
    pub sort: u32,
    pub name: String,
    pub airdate: String,
}

#[derive(Debug, Clone, Default)]
pub struct Collection {
    pub wish: u32,
    pub collect: u32,
    pub doing: u32,
    pub on_hold: u32,
}

/// 一张卡片需要的全部输入。
#[derive(Debug, Clone, Default)]
pub struct BangumiCard {
    pub id: u64,
    pub name: String,
    pub name_cn: String,
    /// 海报原始字节（任意常见格式），出图前裁成 210×290 的等比缩略。
    pub poster: Option<Vec<u8>>,
    pub air_date: String,
    pub platform: String,
    pub score: f32,
    pub rating_total: u32,
    /// 下标 1..=10 对应 1..10 分的人数，下标 0 不用。
    pub rating_counts: [u32; 11],
    pub tags: Vec<String>,
    /// (键, 值)，一般是 原作 / 导演 / 动画制作 / 音乐 / 系列构成 / 脚本。
    pub staff: Vec<(String, String)>,
    pub collection: Collection,
    pub episodes: Vec<Episode>,
    /// 非本篇集数：(类型标签, 集号列表)，如 ("SP", [1, 2])。
    pub other_episodes: Vec<(String, Vec<u32>)>,
    pub aired_count: usize,
    pub current_ep: u32,
    pub current_ep_name: String,
    pub current_ep_airdate: String,
    pub next_ep: u32,
    pub next_ep_name: String,
    pub next_ep_airdate: String,
    pub fansubs: Vec<String>,
    pub generated_at: String,
    /// `rate_emo.gif` 的原始字节（5 个表情横排的雪碧图）。
    pub rating_emoji: Option<Vec<u8>>,
    /// 0..5，取第几格表情。
    pub rating_emoji_idx: usize,
}

impl BangumiCard {
    /// 内容哈希：评分、集数、标签、字幕组变了才需要重新出图。
    pub fn cache_key(&self) -> String {
        let mut parts: Vec<String> = vec![
            format!("bangumi-card:v1:{}", self.id),
            format!("{:.1}", self.score),
            self.poster.is_some().to_string(),
        ];
        for count in self.rating_counts.iter().skip(1) {
            parts.push(count.to_string());
        }
        for ep in &self.episodes {
            parts.push(format!("{}@{}", ep.sort, ep.airdate));
        }
        parts.extend(self.tags.iter().cloned());
        parts.extend(self.fansubs.iter().cloned());
        let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
        content_hash_of(&refs)
    }

    /// 生成给 HTML 模板用的数据（字段名与 `bangumi-card.html` 一一对应）。
    pub fn to_html_payload(&self) -> serde_json::Value {
        let poster = self.poster.as_ref().and_then(|bytes| {
            cover_jpeg(bytes, 420, 580, 90)
                .ok()
                .map(|jpeg| data_uri("image/jpeg", &jpeg))
        });

        let max = self
            .rating_counts
            .iter()
            .skip(1)
            .copied()
            .max()
            .unwrap_or(0)
            .max(1) as f32;
        let bar_pcts: Vec<u32> = (1..=10)
            .rev()
            .map(|score| {
                let pct = (self.rating_counts[score] as f32 / max * 100.0).round() as u32;
                pct.max(3)
            })
            .collect();

        let pages = self.episodes.len().div_ceil(EPISODES_PER_PAGE).max(1);
        let active_page = if self.current_ep > 0 {
            ((self.current_ep as usize - 1) / EPISODES_PER_PAGE).min(pages - 1)
        } else {
            0
        };

        let mut infobox: serde_json::Map<String, serde_json::Value> = self
            .staff
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect();
        // 模板头部读的是 infoboxMap['放送开始']，优先于 date 字段。
        infobox.insert("放送开始".to_owned(), json!(self.air_date));

        let episodes: Vec<serde_json::Value> = self
            .episodes
            .iter()
            .map(|ep| {
                json!({
                    "id": ep.sort,
                    "ep": ep.sort,
                    "sort": ep.sort,
                    "name": ep.name,
                    "name_cn": ep.name,
                    "airdate": ep.airdate,
                    "type": 0,
                })
            })
            .collect();

        let other: Vec<serde_json::Value> = self
            .other_episodes
            .iter()
            .enumerate()
            .map(|(index, (label, sorts))| {
                json!({
                    "type": index as u32 + 1,
                    "label": label,
                    "episodes": sorts
                        .iter()
                        .map(|sort| {
                            json!({
                                "id": sort,
                                "ep": sort,
                                "sort": sort,
                                "name": "",
                                "name_cn": "",
                                "airdate": "",
                                "type": index as u32 + 1,
                            })
                        })
                        .collect::<Vec<_>>(),
                })
            })
            .collect();

        json!({
            "id": self.id,
            "name": self.name,
            "name_cn": self.name_cn,
            "poster": poster,
            "date": self.air_date,
            "platform": self.platform,
            "total_episodes": self.episodes.len(),
            "rating": {
                "score": self.score,
                "rank": 0,
                "total": self.rating_total,
                "count": (1..=10)
                    .map(|score| (score.to_string(), json!(self.rating_counts[score])))
                    .collect::<serde_json::Map<_, _>>(),
            },
            "tags": self.tags.iter().map(|tag| json!({ "name": tag, "count": 0 })).collect::<Vec<_>>(),
            "infoboxMap": infobox,
            "collection": {
                "wish": self.collection.wish,
                "collect": self.collection.collect,
                "doing": self.collection.doing,
                "on_hold": self.collection.on_hold,
                "wish_fmt": format_count(self.collection.wish),
                "collect_fmt": format_count(self.collection.collect),
                "doing_fmt": format_count(self.collection.doing),
                "on_hold_fmt": format_count(self.collection.on_hold),
            },
            "displayStaff": self.staff.iter().take(6).enumerate().map(|(index, (key, value))| json!({
                "key": key,
                "value": value,
                "align": if index % 2 == 1 { "right" } else { "" },
            })).collect::<Vec<_>>(),
            "ratingLabel": rating_label(self.score),
            "ratingEmojiIdx": self.rating_emoji_idx.min(4),
            "generatedAt": self.generated_at,
            "rateEmoDataUri": self.rating_emoji.as_ref().map(|gif| data_uri("image/gif", gif)).unwrap_or_default(),
            "rateEmoW": 170,
            "rateEmoH": 34,
            "barPcts": bar_pcts,
            "episodes": episodes,
            "otherEpisodes": other,
            "airedCount": self.aired_count,
            "episodesPerPage": EPISODES_PER_PAGE,
            "activePageIdx": active_page,
            "currentEpNum": self.current_ep,
            "currentEpName": self.current_ep_name,
            "currentEpAirDate": self.current_ep_airdate,
            "nextEpNum": self.next_ep,
            "nextEpName": self.next_ep_name,
            "nextEpAirDate": self.next_ep_airdate,
            "fansubs": self.fansubs,
        })
    }
}

/// 收藏人数缩写：`1.2K`，与 TS 版 `fmtNum` 一致。
pub fn format_count(count: u32) -> String {
    if count >= 1000 {
        format!("{:.1}K", count as f64 / 1000.0)
    } else {
        count.to_string()
    }
}

/// 评分文案，阈值与 TS 版 `getRatingLabel` 完全一致。
pub fn rating_label(score: f32) -> &'static str {
    if score >= 8.5 {
        "神作"
    } else if score >= 7.5 {
        "力荐"
    } else if score >= 6.5 {
        "推荐"
    } else if score >= 5.5 {
        "还行"
    } else if score >= 4.5 {
        "不过不失"
    } else if score >= 3.5 {
        "较差"
    } else if score >= 2.5 {
        "差"
    } else if score >= 1.5 {
        "很差"
    } else {
        "不忍直视"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BangumiCard {
        BangumiCard {
            id: 400602,
            name: "葬送のフリーレン".to_owned(),
            name_cn: "葬送的芙莉莲".to_owned(),
            air_date: "2023-09-29".to_owned(),
            platform: "TV".to_owned(),
            score: 8.9,
            rating_total: 20_000,
            rating_counts: [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100],
            tags: vec!["奇幻".to_owned(), "冒险".to_owned()],
            staff: vec![("导演".to_owned(), "斋藤圭一郎".to_owned())],
            collection: Collection {
                wish: 1200,
                collect: 3400,
                doing: 800,
                on_hold: 100,
            },
            episodes: (1..=4)
                .map(|sort| Episode {
                    sort,
                    name: format!("第{sort}话"),
                    airdate: "2023-10-01".to_owned(),
                })
                .collect(),
            other_episodes: vec![("SP".to_owned(), vec![1, 2])],
            aired_count: 4,
            current_ep: 4,
            current_ep_name: "第4话".to_owned(),
            generated_at: "2026-10-25 12:00".to_owned(),
            rating_emoji_idx: 1,
            ..Default::default()
        }
    }

    #[test]
    fn payload_matches_template_field_names() {
        let payload = sample().to_html_payload();
        assert_eq!(payload["id"], 400602);
        assert!((payload["rating"]["score"].as_f64().unwrap() - 8.9).abs() < 1e-3);
        assert_eq!(payload["ratingLabel"], "神作");
        assert_eq!(payload["collection"]["wish_fmt"], "1.2K");
        assert_eq!(payload["infoboxMap"]["放送开始"], "2023-09-29");
        assert_eq!(payload["episodesPerPage"], EPISODES_PER_PAGE);
        assert_eq!(payload["episodes"].as_array().unwrap().len(), 4);
        assert_eq!(payload["otherEpisodes"][0]["label"], "SP");
        // 没给海报时必须是 null，模板用 v-if 判断。
        assert!(payload["poster"].is_null());
    }

    #[test]
    fn cache_key_tracks_content() {
        let card = sample();
        let before = card.cache_key();
        let mut changed = sample();
        changed.score = 9.1;
        assert_ne!(before, changed.cache_key());
        assert_eq!(before, sample().cache_key());
    }

    #[test]
    fn count_formatting_matches_ts() {
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1.0K");
        assert_eq!(format_count(12_345), "12.3K");
    }

    #[test]
    fn rating_labels_cover_all_bands() {
        assert_eq!(rating_label(9.0), "神作");
        assert_eq!(rating_label(7.0), "推荐");
        assert_eq!(rating_label(0.5), "不忍直视");
    }
}
