//! Bangumi v0 条目的数据结构。
//!
//! 字段名与 api.bgm.tv 的 JSON 一一对应；所有字段都带默认值，Bangumi 以后加字段、
//! 或缺字段时不会把整个客户端弄挂。`infobox` 的 `value` 有两种真实形态：
//! 字符串、以及 `[{"v": "..."}]` 数组，见 [`InfoboxValue`]。

use std::collections::BTreeMap;

use serde::Deserialize;

/// 卡片上展示的 staff 字段，顺序即展示顺序（与 TS 番剧卡一致）。
pub const STAFF_KEYS: [&str; 6] = ["原作", "导演", "动画制作", "音乐", "系列构成", "脚本"];

/// 条目图片。五种尺寸都是 Bangumi 直接给的 URL。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Images {
    pub large: String,
    pub common: String,
    pub medium: String,
    pub small: String,
    pub grid: String,
}

impl Images {
    /// 按 TS 版回退顺序挑一张能用的图（海报字节下载不在这里做）。
    pub fn best(&self) -> &str {
        [
            &self.large,
            &self.medium,
            &self.common,
            &self.small,
            &self.grid,
        ]
        .into_iter()
        .map(String::as_str)
        .find(|url| !url.trim().is_empty())
        .unwrap_or("")
    }
}

/// 评分。`count` 的键是 1..=10 的分数，值是该分数的人数。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Rating {
    pub score: f32,
    pub total: u32,
    pub count: BTreeMap<u8, u32>,
}

impl Rating {
    pub fn count_for(&self, score: u8) -> u32 {
        self.count.get(&score).copied().unwrap_or(0)
    }
}

/// 收藏人数。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Collection {
    pub wish: u32,
    pub collect: u32,
    pub doing: u32,
    pub on_hold: u32,
    pub dropped: u32,
}

/// 标签。`count` 是本条目上的使用次数，`total_count` 是全站次数。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Tag {
    pub name: String,
    pub count: u32,
    pub total_count: u32,
}

/// `infobox[].value`。Bangumi 里既可能是字符串，也可能是 `{v: ...}` 数组。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum InfoboxValue {
    Text(String),
    Number(f64),
    List(Vec<InfoboxItem>),
    Other(serde_json::Value),
}

impl Default for InfoboxValue {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl InfoboxValue {
    /// 展开成多行文本（`别名` 这类数组会有多行）。
    pub fn texts(&self) -> Vec<String> {
        match self {
            Self::Text(text) => vec![text.trim().to_owned()],
            Self::Number(number) => vec![number.to_string()],
            Self::List(items) => items
                .iter()
                .filter_map(InfoboxItem::text)
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
                .collect(),
            Self::Other(value) => value
                .as_str()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
                .into_iter()
                .collect(),
        }
    }

    /// 单行展示：多个值用中文顿号连接（TS 版也是直接拼）。
    pub fn joined(&self) -> String {
        self.texts().join("、")
    }
}

/// `infobox[].value` 数组里的一项。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum InfoboxItem {
    Text(String),
    Object {
        v: String,
        #[serde(default)]
        k: Option<String>,
    },
    Other(serde_json::Value),
}

impl InfoboxItem {
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Object { v, .. } => Some(v),
            Self::Other(value) => value.as_str(),
        }
    }
}

/// 信息盒的一项，例如 `{"key":"导演","value":"斎藤圭一郎"}`。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Infobox {
    pub key: String,
    pub value: InfoboxValue,
}

impl Infobox {
    pub fn joined(&self) -> String {
        self.value.joined()
    }
}

/// 条目（`/v0/subjects/{id}`）。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Subject {
    pub id: u64,
    pub name: String,
    pub name_cn: String,
    pub summary: String,
    pub images: Images,
    pub rating: Rating,
    pub collection: Collection,
    /// 本篇话数（Bangumi 的 `eps`；`total_episodes` 是章节总数，含 SP）。
    pub eps: u32,
    pub date: String,
    pub platform: String,
    pub infobox: Vec<Infobox>,
    pub tags: Vec<Tag>,
}

impl Subject {
    /// 展示名：优先中文名，没有则原名。
    pub fn display_name(&self) -> &str {
        if self.name_cn.trim().is_empty() {
            &self.name
        } else {
            &self.name_cn
        }
    }

    pub fn infobox_value(&self, key: &str) -> Option<&InfoboxValue> {
        self.infobox
            .iter()
            .find(|item| item.key == key)
            .map(|item| &item.value)
    }

    /// 取信息盒里的文本，空值当作没有。
    pub fn infobox_text(&self, key: &str) -> Option<String> {
        self.infobox_value(key)
            .map(InfoboxValue::joined)
            .filter(|text| !text.trim().is_empty())
    }

    /// 卡片要展示的 staff（原作 / 导演 / 动画制作 / 音乐 / 系列构成 / 脚本）。
    pub fn staff(&self) -> Vec<(String, String)> {
        STAFF_KEYS
            .iter()
            .filter_map(|key| {
                self.infobox_text(key)
                    .map(|value| ((*key).to_owned(), value))
            })
            .collect()
    }

    /// 把 `rating.count` 展成下标 1..=10 的数组，下标 0 不用（卡片模板要这个形状）。
    pub fn rating_counts(&self) -> [u32; 11] {
        let mut counts = [0u32; 11];
        for (&score, &count) in &self.rating.count {
            if (1..=10).contains(&score) {
                counts[score as usize] = count;
            }
        }
        counts
    }
}

impl Subject {
    /// 转成库里的 `anime` 文档。
    ///
    /// 只映射**能从 Bangumi 拿到**的字段：创建/更新时间由 store 写，导航消息、
    /// r18、播放进度这些依赖本地状态，留空由后续流程补。
    pub fn to_anime(&self) -> crate::models::Anime {
        use crate::models::{Anime, Scalar};

        let mut names: Vec<String> = Vec::new();
        for name in [self.name.as_str(), self.name_cn.as_str()] {
            let name = name.trim();
            if !name.is_empty() && !names.iter().any(|existing| existing == name) {
                names.push(name.to_owned());
            }
        }
        let airing_start = if self.date.trim().is_empty() {
            self.infobox_text("放送开始")
        } else {
            Some(self.date.trim().to_owned())
        };

        Anime {
            id: self.id as i64,
            name: Some(self.name.trim().to_owned()).filter(|name| !name.is_empty()),
            name_cn: Some(self.name_cn.trim().to_owned()).filter(|name| !name.is_empty()),
            names,
            image: Some(self.images.best().to_owned()).filter(|url| !url.is_empty()),
            summary: Some(self.summary.trim().to_owned()).filter(|text| !text.is_empty()),
            tags: self
                .tags
                .iter()
                .map(|tag| tag.name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .collect(),
            episode: (self.eps > 0).then(|| self.eps.to_string()),
            score: (self.rating.score > 0.0).then_some(Scalar::Num(f64::from(self.rating.score))),
            airing_day: self.infobox_text("放送星期"),
            airing_start,
            ..Default::default()
        }
    }
}

impl Episode {
    /// 转成 `episodes_meta` 文档。空字符串统一折成 `None`：老库里的空值就是缺字段，
    /// 两种形态混着会让下游的 `Option` 判断变得难写。
    pub fn to_meta(&self, subject_id: u64) -> crate::models::EpisodeMeta {
        let text = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        };
        crate::models::EpisodeMeta {
            id: self.id as i64,
            ep: self.ep,
            sort: self.sort,
            r#type: Some(self.r#type),
            airdate: text(&self.airdate),
            name: text(&self.name),
            name_cn: text(&self.name_cn),
            duration: text(&self.duration),
            desc: text(&self.desc),
            comment: None,
            subject_id: subject_id as i64,
        }
    }
}

/// 章节（`/v0/episodes`）。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Episode {
    pub id: u64,
    /// 0 = 本篇，1 = SP，2 = OP，3 = ED，4 = 预告，5 = MAD，6 = 其他。
    #[serde(rename = "type")]
    pub r#type: i32,
    pub ep: f64,
    pub sort: f64,
    pub name: String,
    pub name_cn: String,
    pub airdate: String,
    pub duration: String,
    pub desc: String,
}

impl Episode {
    pub fn is_main(&self) -> bool {
        self.r#type == 0
    }

    pub fn display_name(&self) -> &str {
        if self.name_cn.trim().is_empty() {
            &self.name
        } else {
            &self.name_cn
        }
    }

    /// 卡片用的整数集号：优先 `sort`，没有就退回 `ep`。
    pub fn number(&self) -> u32 {
        let value = if self.sort > 0.0 { self.sort } else { self.ep };
        if value.is_finite() && value > 0.0 {
            value as u32
        } else {
            0
        }
    }

    /// 非本篇章节的展示标签（卡片把 SP/OP/ED 分开列）。
    pub fn type_label(&self) -> &'static str {
        match self.r#type {
            1 => "SP",
            2 => "OP",
            3 => "ED",
            4 => "预告",
            5 => "MAD",
            _ => "其他",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实 `/v0/subjects/400602` 的裁剪版（只删了部分 tags/infobox），
    /// 保留了 `infobox[].value` 的字符串 + `[{v: ...}]` 两种形态。
    const SUBJECT_JSON: &str = r#"{
        "date": "2023-09-29",
        "platform": "TV",
        "images": {
            "small": "https://lain.bgm.tv/r/200/pic/cover/l/13/c5/400602_ZI8Y9.jpg",
            "grid": "https://lain.bgm.tv/r/100/pic/cover/l/13/c5/400602_ZI8Y9.jpg",
            "large": "https://lain.bgm.tv/pic/cover/l/13/c5/400602_ZI8Y9.jpg",
            "medium": "https://lain.bgm.tv/r/800/pic/cover/l/13/c5/400602_ZI8Y9.jpg",
            "common": "https://lain.bgm.tv/r/400/pic/cover/l/13/c5/400602_ZI8Y9.jpg"
        },
        "summary": "魔法使芙莉莲和勇者辛美尔等人一起，历经十年的冒险之后击败了魔王。",
        "name": "葬送のフリーレン",
        "name_cn": "葬送的芙莉莲",
        "tags": [
            {"name": "治愈", "count": 8147, "total_count": 308775},
            {"name": "奇幻", "count": 6468, "total_count": 492419}
        ],
        "infobox": [
            {"key": "中文名", "value": "葬送的芙莉莲"},
            {"key": "别名", "value": [
                {"v": "Frieren: Beyond Journey's End"},
                {"v": "Sousou no Frieren"}
            ]},
            {"key": "话数", "value": "28"},
            {"key": "放送开始", "value": "2023年9月29日"},
            {"key": "导演", "value": "斎藤圭一郎"},
            {"key": "动画制作", "value": "MADHOUSE"}
        ],
        "rating": {
            "rank": 43,
            "total": 36718,
            "count": {
                "1": 130, "2": 37, "3": 54, "4": 111, "5": 357,
                "6": 1112, "7": 3744, "8": 10655, "9": 12995, "10": 7523
            },
            "score": 8.5
        },
        "total_episodes": 36,
        "collection": {
            "on_hold": 2060, "dropped": 776, "wish": 7129,
            "collect": 55383, "doing": 11876
        },
        "id": 400602,
        "eps": 28,
        "type": 2
    }"#;

#[test]
    fn subject_maps_to_anime_document() {
        let subject: Subject = serde_json::from_str(SUBJECT_JSON).unwrap();
        let anime = subject.to_anime();
        assert_eq!(anime.id, 400602);
        assert_eq!(anime.display_name(), "葬送的芙莉莲");
        // 中文名 + 原名都在 names 里，匹配时两种写法都能命中。
        assert!(anime.names.contains(&"葬送のフリーレン".to_owned()));
        assert!(anime.names.contains(&"葬送的芙莉莲".to_owned()));
        assert_eq!(anime.episode.as_deref(), Some("28"));
        assert_eq!(anime.airing_start.as_deref(), Some("2023-09-29"));
        assert!(anime.image.as_deref().unwrap_or_default().ends_with(".jpg"));
    }

    #[test]
    fn episode_maps_to_meta_and_blanks_become_none() {
        let episode: Episode = serde_json::from_str(
            r#"{"id": 1, "type": 1, "ep": 0, "sort": 1.5, "name": "SP1", "name_cn": "", "airdate": "", "duration": "24m"}"#,
        )
        .unwrap();
        let meta = episode.to_meta(400602);
        assert_eq!(meta.subject_id, 400602);
        assert_eq!(meta.r#type, Some(1));
        assert!((meta.sort - 1.5).abs() < 1e-9);
        // 空字符串统一折成 None，别让下游既判 None 又判空串。
        assert!(meta.name_cn.is_none());
        assert!(meta.airdate.is_none());
        assert_eq!(meta.name.as_deref(), Some("SP1"));
    }
    #[test]
    fn parses_real_subject_shape() {
        let subject: Subject = serde_json::from_str(SUBJECT_JSON).unwrap();
        assert_eq!(subject.id, 400602);
        assert_eq!(subject.name, "葬送のフリーレン");
        assert_eq!(subject.name_cn, "葬送的芙莉莲");
        assert_eq!(subject.display_name(), "葬送的芙莉莲");
        assert_eq!(subject.date, "2023-09-29");
        assert_eq!(subject.platform, "TV");
        assert_eq!(subject.eps, 28);
        assert_eq!(
            subject.images.large,
            "https://lain.bgm.tv/pic/cover/l/13/c5/400602_ZI8Y9.jpg"
        );
        assert_eq!(subject.images.best(), subject.images.large);
        assert_eq!(subject.rating.score, 8.5);
        assert_eq!(subject.rating.total, 36_718);
        assert_eq!(subject.rating.count_for(10), 7523);
        assert_eq!(subject.rating.count_for(0), 0);
        assert_eq!(subject.collection.collect, 55_383);
        assert_eq!(subject.collection.on_hold, 2060);
        assert_eq!(subject.collection.dropped, 776);
        assert_eq!(subject.tags.len(), 2);
        assert_eq!(subject.tags[0].name, "治愈");
        assert_eq!(subject.tags[0].total_count, 308_775);
    }

    #[test]
    fn infobox_handles_strings_and_arrays() {
        let subject: Subject = serde_json::from_str(SUBJECT_JSON).unwrap();
        assert_eq!(
            subject.infobox_text("中文名").as_deref(),
            Some("葬送的芙莉莲")
        );
        assert_eq!(subject.infobox_text("话数").as_deref(), Some("28"));
        assert_eq!(
            subject.infobox_text("别名").as_deref(),
            Some("Frieren: Beyond Journey's End、Sousou no Frieren")
        );
        assert_eq!(subject.infobox_text("不存在"), None);
        assert_eq!(
            subject.staff(),
            vec![
                ("导演".to_owned(), "斎藤圭一郎".to_owned()),
                ("动画制作".to_owned(), "MADHOUSE".to_owned()),
            ]
        );
        let counts = subject.rating_counts();
        assert_eq!(counts[0], 0);
        assert_eq!(counts[1], 130);
        assert_eq!(counts[10], 7523);
    }

    #[test]
    fn missing_subject_fields_fall_back_to_defaults() {
        let subject: Subject = serde_json::from_str(r#"{"id": 1, "name": "某番"}"#).unwrap();
        assert_eq!(subject.id, 1);
        assert!(subject.name_cn.is_empty());
        assert_eq!(subject.display_name(), "某番");
        assert_eq!(subject.images.best(), "");
        assert_eq!(subject.rating.count_for(8), 0);
        assert!(subject.infobox.is_empty());
        assert_eq!(subject.rating_counts(), [0; 11]);
    }

    #[test]
    fn parses_real_episode_shape() {
        let episode: Episode = serde_json::from_str(
            r#"{
                "airdate": "2023-09-29",
                "name": "冒険の終わり",
                "name_cn": "冒险结束",
                "duration": "00:26:00",
                "desc": "脚本：鈴木智尋",
                "ep": 1,
                "sort": 1,
                "id": 1227087,
                "subject_id": 400602,
                "comment": 297,
                "type": 0,
                "disc": 0,
                "duration_seconds": 1560
            }"#,
        )
        .unwrap();
        assert_eq!(episode.id, 1_227_087);
        assert_eq!(episode.r#type, 0);
        assert!(episode.is_main());
        assert_eq!(episode.ep, 1.0);
        assert_eq!(episode.sort, 1.0);
        assert_eq!(episode.number(), 1);
        assert_eq!(episode.name, "冒険の終わり");
        assert_eq!(episode.name_cn, "冒险结束");
        assert_eq!(episode.display_name(), "冒险结束");
        assert_eq!(episode.airdate, "2023-09-29");
        assert_eq!(episode.duration, "00:26:00");
        assert_eq!(episode.desc, "脚本：鈴木智尋");
    }

    #[test]
    fn special_episode_labels_and_number_fallback() {
        let special: Episode =
            serde_json::from_str(r#"{"id": 2, "type": 1, "ep": 2.5, "sort": 0, "name": "SP"}"#)
                .unwrap();
        assert!(!special.is_main());
        assert_eq!(special.type_label(), "SP");
        assert_eq!(special.number(), 2);
        assert_eq!(special.display_name(), "SP");
    }
}
