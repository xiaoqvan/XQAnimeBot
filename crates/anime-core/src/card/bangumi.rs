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
    ///
    /// 模板是纯 HTML（没有 JS 运行时），凡是「算出来才好画」的东西都在这里预先算好：
    /// 标题回退、分页标签、评分条、补位格、两位数集号、评分/进度的展示文本。
    /// 模板只排版不计算，字段名写错会在出图前直接报错。
    pub fn to_html_payload(&self) -> serde_json::Value {
        let poster = self.poster.as_ref().and_then(|bytes| {
            cover_jpeg(bytes, 420, 580, 90)
                .ok()
                .map(|jpeg| data_uri("image/jpeg", &jpeg))
        });

        // 标题：中文名为空时回退原名（模板不再写 || 表达式）。
        let title = if self.name_cn.trim().is_empty() {
            self.name.clone()
        } else {
            self.name_cn.clone()
        };
        // 进度数字：0 就显示 “-”，与 TS 版的 || '-' 一致。
        let ep_num_fmt = |n: u32| if n > 0 { n.to_string() } else { "-".to_owned() };

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
        // 章节分页芯片：标签 “01-30” 这种区间，以及当前页高亮。
        let page_chips: Vec<serde_json::Value> = (0..pages)
            .map(|page| {
                let start = page * EPISODES_PER_PAGE + 1;
                let end = start
                    .saturating_add(EPISODES_PER_PAGE - 1)
                    .min(self.episodes.len());
                json!({
                    "label": format!("{start:02}-{end:02}"),
                    "active": page == active_page,
                })
            })
            .collect();

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
                    // 展示用：两位数集号（01）与「已放送」标记，模板不做计算。
                    "sort_fmt": format!("{:02}", ep.sort),
                    "aired": ep.sort <= self.current_ep,
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
                                "sort_fmt": format!("{:02}", sort),
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

        // 当前页的章节 + 补位格（把 30 格占满，行高才整齐）。
        let page_start = active_page * EPISODES_PER_PAGE;
        let page_end = (page_start + EPISODES_PER_PAGE).min(episodes.len());
        let empty_slots: Vec<serde_json::Value> = (page_end..page_start + EPISODES_PER_PAGE)
            .map(|_| json!({}))
            .collect();

        json!({
            "id": self.id,
            "name": self.name,
            "name_cn": self.name_cn,
            "title": title,
            "poster": poster,
            "date": self.air_date,
            "platform": self.platform,
            "total_episodes": self.episodes.len(),
            "score_fmt": format!("{:.1}", self.score),
            "finished": self.aired_count >= self.episodes.len(),
            "pages": page_chips,
            "pageEpisodes": episodes[page_start..page_end].to_vec(),
            "emptySlots": empty_slots,
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
                // 展示用：空值显示 “-”；右列文字右对齐（模板不做计算）。
                "value_fmt": if value.is_empty() { "-".to_owned() } else { value.clone() },
                "align_right": index % 2 == 1,
                "align": if index % 2 == 1 { "right" } else { "" },
            })).collect::<Vec<_>>(),
            "ratingLabel": rating_label(self.score),
            "ratingEmojiIdx": self.rating_emoji_idx.min(4),
            "generatedAt": self.generated_at,
            "rateEmoDataUri": self.rating_emoji.as_ref().map(|gif| data_uri("image/gif", gif)).unwrap_or_default(),
            "rateEmoW": 170,
            "rateEmoH": 34,
            // 雪碧图是 5 格横排：每格 34px，按评分取第几格的偏移也算好。
            "rateEmoCellW": 34,
            "rateEmoOffset": self.rating_emoji_idx.min(4) * 34,
            "barPcts": bar_pcts,
            // 评分直方图：从 10 分到 1 分，高度百分比 + 刻度文案 + 当前评分高亮。
            "bars": bar_pcts.iter().enumerate().map(|(idx, pct)| json!({
                "pct": pct,
                "label": (10 - idx).to_string(),
                "highlight": (10 - idx) as f32 == self.score.round(),
            })).collect::<Vec<_>>(),
            "episodes": episodes,
            "otherEpisodes": other,
            "airedCount": self.aired_count,
            "episodesPerPage": EPISODES_PER_PAGE,
            "activePageIdx": active_page,
            "currentEpNum": self.current_ep,
            "currentEpNum_fmt": ep_num_fmt(self.current_ep),
            "currentEpName": self.current_ep_name,
            "currentEpAirDate": self.current_ep_airdate,
            "nextEpNum": self.next_ep,
            "nextEpNum_fmt": ep_num_fmt(self.next_ep),
            "nextEpName": self.next_ep_name,
            "nextEpAirDate": self.next_ep_airdate,
            "fansubs": self.fansubs,
        })
    }
}

impl BangumiCard {
    /// 用 Bangumi API 的条目 + 章节填好卡片里**静态**的那部分。
    ///
    /// 播放进度（`current_ep` / `next_ep` / `aired_count`）、海报字节、字幕组和
    /// 评分表情图仍由调用方补：这些要么依赖本地库，要么要下载图片/读资源。
    pub fn from_subject(
        subject: &crate::bangumi::Subject,
        episodes: &[crate::bangumi::Episode],
    ) -> Self {
        let mut aired = Vec::new();
        let mut other_episodes: Vec<(String, Vec<u32>)> = Vec::new();
        for episode in episodes {
            if episode.is_main() {
                aired.push(Episode {
                    sort: episode.number(),
                    name: episode.display_name().to_owned(),
                    airdate: episode.airdate.clone(),
                });
                continue;
            }
            let label = episode.type_label().to_owned();
            let sort = episode.number();
            match other_episodes
                .iter_mut()
                .find(|(existing, _)| *existing == label)
            {
                Some((_, sorts)) => sorts.push(sort),
                None => other_episodes.push((label, vec![sort])),
            }
        }

        let air_date = if subject.date.trim().is_empty() {
            subject.infobox_text("放送开始").unwrap_or_default()
        } else {
            subject.date.clone()
        };

        Self {
            id: subject.id,
            name: subject.name.clone(),
            name_cn: subject.name_cn.clone(),
            poster: None,
            air_date,
            platform: subject.platform.clone(),
            score: subject.rating.score,
            rating_total: subject.rating.total,
            rating_counts: subject.rating_counts(),
            tags: subject.tags.iter().map(|tag| tag.name.clone()).collect(),
            staff: subject.staff(),
            collection: Collection {
                wish: subject.collection.wish,
                collect: subject.collection.collect,
                doing: subject.collection.doing,
                on_hold: subject.collection.on_hold,
            },
            episodes: aired,
            other_episodes,
            generated_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            ..Default::default()
        }
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
        // 没给海报时必须是 null，模板用 #if 判断。
        assert!(payload["poster"].is_null());
        // 展示用字段：模板只排版不计算，这些都得预先算好。
        assert_eq!(payload["title"], "葬送的芙莉莲");
        assert_eq!(payload["score_fmt"], "8.9");
        assert_eq!(payload["currentEpNum_fmt"], "4");
        assert_eq!(payload["nextEpNum_fmt"], "-");
        assert_eq!(payload["finished"], true);
        assert_eq!(payload["pages"][0]["label"], "01-04");
        assert_eq!(payload["pages"][0]["active"], true);
        assert_eq!(payload["pageEpisodes"][3]["sort_fmt"], "04");
        assert_eq!(payload["pageEpisodes"][3]["aired"], true);
        assert_eq!(payload["bars"][1]["label"], "9");
        assert_eq!(payload["bars"][1]["highlight"], true);
        assert_eq!(payload["displayStaff"][0]["value_fmt"], "斋藤圭一郎");
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

    #[test]
    fn from_subject_maps_api_payload_and_splits_episodes() {
        let subject: crate::bangumi::Subject = serde_json::from_str(
            r#"{
                "id": 400602,
                "name": "葬送のフリーレン",
                "name_cn": "葬送的芙莉莲",
                "date": "2023-09-29",
                "platform": "TV",
                "tags": [{"name": "治愈", "count": 8147}],
                "rating": {
                    "score": 8.5,
                    "total": 36718,
                    "count": {"1": 130, "8": 10655, "10": 7523}
                },
                "collection": {
                    "wish": 7129, "collect": 55383, "doing": 11876,
                    "on_hold": 2060, "dropped": 776
                },
                "infobox": [
                    {"key": "放送开始", "value": "2023年9月29日"},
                    {"key": "导演", "value": "斎藤圭一郎"},
                    {"key": "动画制作", "value": "MADHOUSE"}
                ]
            }"#,
        )
        .unwrap();
        let episodes: Vec<crate::bangumi::Episode> = serde_json::from_str(
            r#"[
                {"id": 1, "type": 0, "ep": 1, "sort": 1,
                 "name": "冒険の終わり", "name_cn": "冒险结束", "airdate": "2023-09-29"},
                {"id": 2, "type": 0, "ep": 2, "sort": 2,
                 "name": "別に魔法じゃなくたって…", "name_cn": "", "airdate": "2023-09-29"},
                {"id": 3, "type": 1, "ep": 1, "sort": 1, "name": "SP1"}
            ]"#,
        )
        .unwrap();

        let card = BangumiCard::from_subject(&subject, &episodes);
        assert_eq!(card.id, 400602);
        assert_eq!(card.name, "葬送のフリーレン");
        assert_eq!(card.name_cn, "葬送的芙莉莲");
        assert_eq!(card.air_date, "2023-09-29");
        assert_eq!(card.platform, "TV");
        assert_eq!(card.score, 8.5);
        assert_eq!(card.rating_total, 36_718);
        assert_eq!(card.rating_counts[10], 7523);
        assert_eq!(card.tags, vec!["治愈".to_owned()]);
        assert_eq!(card.collection.collect, 55_383);
        assert_eq!(card.episodes.len(), 2);
        // 有中文名用中文名，没有则回退原名。
        assert_eq!(card.episodes[0].name, "冒险结束");
        assert_eq!(card.episodes[1].name, "別に魔法じゃなくたって…");
        assert_eq!(card.other_episodes, vec![("SP".to_owned(), vec![1])]);
        assert_eq!(card.staff[0], ("导演".to_owned(), "斎藤圭一郎".to_owned()));
        assert_eq!(card.staff.len(), 2);
        assert!(card.poster.is_none());
    }

    #[test]
    fn from_subject_falls_back_to_infobox_air_date() {
        let subject: crate::bangumi::Subject = serde_json::from_str(
            r#"{
                "id": 1,
                "name": "某番",
                "infobox": [{"key": "放送开始", "value": "2024年1月5日"}]
            }"#,
        )
        .unwrap();
        let card = BangumiCard::from_subject(&subject, &[]);
        assert_eq!(card.air_date, "2024年1月5日");
        assert!(card.episodes.is_empty());
        assert!(card.other_episodes.is_empty());
    }
}
