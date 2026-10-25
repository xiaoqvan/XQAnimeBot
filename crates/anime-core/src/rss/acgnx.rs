//! 末日动漫资源库（share.acgnx.se）。
//!
//! 结构几乎和 dmhy 一样（也是 `category` + `author` 白名单），差别只有：
//! 站点、映射表（acgnx 的 `author` 有时直接就是字幕组名）、以及**没有** LoliHouse 例外。
//! 这份映射表同样是白名单。

use crate::models::{RssItem, RssKind};
use crate::rss::common::format_pub_date_local;
use crate::rss::{Http, xml};
use crate::title::{clean_title, is_title_allowed};
use crate::{Error, Result};

pub const URL: &str = "https://share.acgnx.se/rss-sort-1.xml";

/// `author` → 字幕组名，同时是白名单。
pub const AUTHOR_MAPPING: &[(&str, &str)] = &[
    ("smzase", "三明治摆烂组"),
    ("nekomoekissaten", "喵萌奶茶屋"),
    ("TrySail", "Kirara Fantasia"),
    ("ANiTorrent", "ANi"),
    ("dmguo", "動漫國字幕組"),
    ("MingYSub", "MingYSub"),
    ("SweetSub", "SweetSub"),
    ("Haruhanasub", "拨雪寻春"),
    ("星空字幕组", "星空字幕组"),
    ("悠哈璃羽字幕社", "悠哈璃羽字幕社"),
    ("orion", "猎户压制部"),
    ("aagaguai", "奇怪机翻组"),
    ("晚街与灯", "晚街与灯"),
    ("亿次研同好会", "亿次研同好会"),
    ("北宇治字幕组", "北宇治字幕组"),
    ("sakuratosub", "桜都字幕组"),
    ("云歌", "云歌字幕组"),
    ("樱桃花字幕组", "樱桃花字幕组"),
];

/// `author` 映射到字幕组名；不在表里返回 `None`（= 不在白名单）。
pub fn author_of(account: &str) -> Option<&'static str> {
    AUTHOR_MAPPING
        .iter()
        .find(|(from, _)| *from == account)
        .map(|(_, to)| *to)
}

pub fn fetch(http: &Http, max_items: usize) -> Result<Vec<RssItem>> {
    let xml = http.get(URL)?;
    from_xml(&xml, max_items)
}

/// 纯函数版本（同 dmhy：先截断再过滤）。
pub fn from_xml(xml: &str, max_items: usize) -> Result<Vec<RssItem>> {
    let mut items = Vec::new();
    for entry in xml::parse(xml)?.into_iter().take(max_items) {
        if entry.category.trim() != "動畫" {
            continue;
        }
        let account = entry.author.trim();
        if account.contains("鏡像") {
            continue;
        }
        let Some(author) = author_of(account) else {
            continue;
        };

        let title = clean_title(&entry.title);
        if !is_title_allowed(&title) || title.is_empty() || entry.enclosure.is_empty() {
            continue;
        }

        items.push(RssItem {
            kind: RssKind::Acgnx,
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

pub fn describe(err: Error) -> String {
    format!("末日动漫：{err}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0"><channel>
  <item>
    <title>[喵萌奶茶屋&amp;LoliHouse] 某番 - 05 [1080p][简繁内封]</title>
    <link>https://share.acgnx.se/show-1.html</link>
    <pubDate>Sun, 25 Oct 2026 13:00:00 +0000</pubDate>
    <author>nekomoekissaten</author>
    <category>動畫</category>
    <enclosure url="magnet:?xt=urn:btih:111"/>
  </item>
  <item>
    <title>[ANi] 某番 - 05 [1080P]</title>
    <link>https://share.acgnx.se/show-2.html</link>
    <pubDate>Sun, 25 Oct 2026 14:00:00 +0000</pubDate>
    <author>ANiTorrent</author>
    <category>動畫</category>
    <enclosure url="magnet:?xt=urn:btih:222"/>
  </item>
  <item>
    <title>[ANi] 某番 - 06 [1080P]</title>
    <link>https://share.acgnx.se/show-3.html</link>
    <pubDate>Sun, 25 Oct 2026 15:00:00 +0000</pubDate>
    <author>ANiTorrent</author>
    <category>動畫</category>
  </item>
</channel></rss>"#;

    #[test]
    fn drops_inner_subbed_and_missing_magnet() {
        // 第 1 条「内封」被 is_title_allowed 拦掉，第 3 条没有 enclosure。
        let items = from_xml(FEED, 50).unwrap();
        assert_eq!(items.len(), 1, "{items:#?}");
        assert_eq!(items[0].author.as_deref(), Some("ANi"));
        assert_eq!(items[0].kind, RssKind::Acgnx);
        assert_eq!(items[0].magnet.as_deref(), Some("magnet:?xt=urn:btih:222"));
    }

    #[test]
    fn identity_mapping_still_checks_whitelist() {
        assert_eq!(author_of("星空字幕组"), Some("星空字幕组"));
        assert_eq!(author_of("陌生人"), None);
    }
}
