//! 动漫花园（dmhy.org）。
//!
//! 与其他两源最大的差别：dmhy 的 `author` 是**发布者账号**，不是字幕组名，
//! 所以这里有一张账号 → 字幕组的映射表，表本身就是白名单——
//! 不在表里的发布者直接丢掉（TS 版 `authorWhitelist = Object.keys(authorMapping)`）。
//!
//! LoliHouse 是唯一例外：它的标题带 `[简繁内封字幕]`，会被 `is_title_allowed` 的「内封」
//! 规则误杀，所以先把这段改写成 `[简体内嵌]` 再放行（TS 版原样照抄）。

use crate::models::{RssItem, RssKind};
use crate::rss::common::format_pub_date_local;
use crate::rss::{Http, xml};
use crate::title::{clean_title, is_title_allowed};
use crate::{Error, Result};

pub const URL: &str = "https://dmhy.org/topics/rss/sort_id/2/rss.xml";
const REFERER: &str = "https://dmhy.org/";

/// 发布者账号 → 字幕组名。**这张表同时也是白名单**，加人就是加信任。
pub const AUTHOR_MAPPING: &[(&str, &str)] = &[
    ("smzase", "三明治摆烂组"),
    ("nekomoekissaten", "喵萌奶茶屋"),
    ("ANiTorrent", "ANi"),
    ("春音爱良aira", "動漫國字幕組"),
    ("MingY明", "MingYSub"),
    ("wudihongjing", "MingYSub"),
    ("SweetSub", "SweetSub"),
    ("芙宁娜", "拨雪寻春"),
    ("Needfire", "拨雪寻春"),
    ("XKSub", "星空字幕组"),
    ("千恋万花", "星空字幕组"),
    ("捷德本尊", "星空字幕组"),
    ("悠哈C9字幕社", "悠哈璃羽字幕社"),
    ("望月月", "悠哈璃羽字幕社"),
    ("orion321", "猎户压制部"),
    ("aagaguai", "奇怪机翻组"),
    ("晚街与灯", "晚街与灯"),
    ("microseventh", "亿次研同好会"),
    ("pianolibrary", "亿次研同好会"),
    ("小圆香径独徘徊", "亿次研同好会"),
    ("Kitauji", "北宇治字幕组"),
    ("mistakey", "北宇治字幕组"),
    ("sakurato", "桜都字幕组"),
    ("shkong", "桜都字幕组"),
    ("anmli", "桜都字幕组"),
    ("清风夏沐", "云歌字幕组"),
    ("萌樱字幕组", "云歌字幕组"),
    ("Alicest", "S1百综字幕组"),
    ("InsWan", "云歌字幕组"),
    ("樱桃花字幕组desu", "樱桃花字幕组"),
    ("D4869", "云光字幕组"),
    ("KissSub", "爱恋字幕社"),
    ("nagi123", "爱恋字幕社"),
    ("kevin14827", "爱恋字幕社"),
    ("HYSUB", "幻樱字幕组"),
    ("summer1278", "幻樱字幕组"),
    ("MingHyuk", "雪飄工作室"),
    ("LoliHouse", "LoliHouse"),
];

/// 账号映射到字幕组名；不在表里返回 `None`（= 不在白名单）。
pub fn author_of(account: &str) -> Option<&'static str> {
    AUTHOR_MAPPING
        .iter()
        .find(|(from, _)| *from == account)
        .map(|(_, to)| *to)
}

/// 抓取并过滤。网络错误向上抛，交由 [`crate::rss::fetch_all`] 记成「该源失败」。
pub fn fetch(http: &Http, max_items: usize) -> Result<Vec<RssItem>> {
    let xml = http.get_with(URL, Some(REFERER))?;
    from_xml(&xml, max_items)
}

/// 纯函数版本：喂一段 XML 出条目，单测与回放都靠它。
///
/// `max_items` 的语义与 TS 一致：**只处理前 N 条原始条目**，过滤发生在截断之后，
/// 所以结果条数会小于 N。
pub fn from_xml(xml: &str, max_items: usize) -> Result<Vec<RssItem>> {
    let mut items = Vec::new();
    for entry in xml::parse(xml)?.into_iter().take(max_items) {
        // dmhy 的 RSS 混着漫画/日剧，只留动画分类。
        if entry.category.trim() != "動畫" {
            continue;
        }
        let account = entry.author.trim();
        // 「鏡像」是别人搬运的副本，标题会出现两份，留原始发布者即可。
        if account.contains("鏡像") {
            continue;
        }
        let Some(author) = author_of(account) else {
            continue;
        };

        let mut title = clean_title(&entry.title);
        if account == "LoliHouse" {
            title = title.replace("[简繁内封字幕]", "[简体内嵌]");
        } else if !is_title_allowed(&title) {
            continue;
        }
        if title.is_empty() || entry.enclosure.is_empty() {
            continue;
        }

        items.push(RssItem {
            kind: RssKind::Dmhy,
            id: None,
            title,
            link: entry.link.trim().to_owned(),
            pub_date: format_pub_date_local(&entry.pub_date),
            magnet: Some(entry.enclosure),
            torrent: None,
            author: Some(author.to_owned()),
        });
    }
    Ok(items)
}

/// 把 HTTP 层的错误补上「哪个源」，方便 fetch_all 汇总成一句人话。
pub fn describe(err: Error) -> String {
    format!("动漫花园：{err}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0"><channel>
  <item>
    <title><![CDATA[[ANi] 葬送的芙莉莲 - 05 [1080P][Baha][CHT]]></title>
    <link>https://share.dmhy.org/topics/view/1.html</link>
    <pubDate>Sun, 25 Oct 2026 13:00:00 +0000</pubDate>
    <author><![CDATA[ANiTorrent]]></author>
    <category><![CDATA[動畫]]></category>
    <enclosure url="magnet:?xt=urn:btih:aaa"/>
  </item>
  <item>
    <title><![CDATA[[LoliHouse] 某番 - 05 [简繁内封字幕][1080P]]></title>
    <link>https://share.dmhy.org/topics/view/2.html</link>
    <pubDate>Sun, 25 Oct 2026 14:00:00 +0000</pubDate>
    <author><![CDATA[LoliHouse]]></author>
    <category><![CDATA[動畫]]></category>
    <enclosure url="magnet:?xt=urn:btih:bbb"/>
  </item>
  <item>
    <title><![CDATA[[ANi] 某番 - 05 [1080P]]]></title>
    <link>https://share.dmhy.org/topics/view/3.html</link>
    <pubDate>Sun, 25 Oct 2026 15:00:00 +0000</pubDate>
    <author><![CDATA[unknown-uploader]]></author>
    <category><![CDATA[動畫]]></category>
    <enclosure url="magnet:?xt=urn:btih:ccc"/>
  </item>
  <item>
    <title><![CDATA[[ANi] 某番 - 05 [1080P]]]></title>
    <link>https://share.dmhy.org/topics/view/4.html</link>
    <pubDate>Sun, 25 Oct 2026 16:00:00 +0000</pubDate>
    <author><![CDATA[ANiTorrent]]></author>
    <category><![CDATA[日劇]]></category>
    <enclosure url="magnet:?xt=urn:btih:ddd"/>
  </item>
  <item>
    <title><![CDATA[[ANi] 某番 - 05 [1080P][鏡像]]]></title>
    <link>https://share.dmhy.org/topics/view/5.html</link>
    <pubDate>Sun, 25 Oct 2026 17:00:00 +0000</pubDate>
    <author><![CDATA[ANiTorrent 鏡像]]></author>
    <category><![CDATA[動畫]]></category>
    <enclosure url="magnet:?xt=urn:btih:eee"/>
  </item>
  <item>
    <title><![CDATA[[ANi] 某番 - 05 [720P]]]></title>
    <link>https://share.dmhy.org/topics/view/6.html</link>
    <pubDate>Sun, 25 Oct 2026 18:00:00 +0000</pubDate>
    <author><![CDATA[ANiTorrent]]></author>
    <category><![CDATA[動畫]]></category>
    <enclosure url="magnet:?xt=urn:btih:fff"/>
  </item>
</channel></rss>"#;

    #[test]
    fn keeps_only_whitelisted_animation_items() {
        let items = from_xml(FEED, 50).unwrap();
        // 留下来的：ANi 的正常条目 + LoliHouse（内封被改写后放行）。
        assert_eq!(items.len(), 2, "{items:#?}");
        assert_eq!(items[0].author.as_deref(), Some("ANi"));
        assert_eq!(items[1].author.as_deref(), Some("LoliHouse"));
        assert!(
            items[1].title.contains("[简体内嵌]"),
            "LoliHouse 的内封要改写成内嵌：{}",
            items[1].title
        );
        assert!(items.iter().all(|item| item.kind == RssKind::Dmhy));
        assert_eq!(items[0].pub_date, "2026年10月25日 09:00PM");
    }

    #[test]
    fn max_items_truncates_before_filtering() {
        // 只取前 3 条 → 里面只有 2 条能过（第 3 条是未知发布者）。
        let items = from_xml(FEED, 3).unwrap();
        assert_eq!(items.len(), 2);
        // 只取前 1 条 → 只剩 ANi 那条。
        assert_eq!(from_xml(FEED, 1).unwrap().len(), 1);
    }

    #[test]
    fn author_table_lookup_is_exact() {
        assert_eq!(author_of("ANiTorrent"), Some("ANi"));
        assert_eq!(author_of("aniTorrent"), None);
        assert_eq!(author_of("路人甲"), None);
    }
}
