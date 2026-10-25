//! bangumi.moe（萌番组）站内 RSS。
//!
//! 这个源**没有作者字段**，也就没有白名单——过滤只靠标题规则。
//! 它的 `enclosure` 是 `.torrent` 地址，真正的磁力要去 API 取（见 [`super::bangumi_moe`]），
//! 所以这里只填 `torrent`，`magnet` 留空。
//!
//! 时间处理和其他两源不同：`pubDate` 按 UTC 解析后固定 +8，不跟随本机时区（TS 版如此）。

use crate::models::{RssItem, RssKind};
use crate::rss::common::format_pub_date_utc8;
use crate::rss::{Http, xml};
use crate::title::{clean_title, is_title_allowed};
use crate::{Error, Result};

pub const URL: &str = "https://bangumi.moe/rss/latest";

pub fn fetch(http: &Http, max_items: usize) -> Result<Vec<RssItem>> {
    let xml = http.get(URL)?;
    from_xml(&xml, max_items)
}

/// 纯函数版本。`max_items == 0` 表示不截断（bangumi 站内 RSS 本来条数就少）。
pub fn from_xml(xml: &str, max_items: usize) -> Result<Vec<RssItem>> {
    let parsed = xml::parse(xml)?;
    let limit = if max_items == 0 {
        parsed.len()
    } else {
        max_items
    };
    let mut items = Vec::new();
    for entry in parsed.into_iter().take(limit) {
        let title = clean_title(&entry.title);
        if title.is_empty() || !is_title_allowed(&title) {
            continue;
        }
        if entry.enclosure.is_empty() {
            continue;
        }
        items.push(RssItem {
            kind: RssKind::Bangumi,
            id: Some(torrent_id(&entry.link)),
            title,
            link: entry.link.trim().to_owned(),
            pub_date: format_pub_date_utc8(&entry.pub_date),
            magnet: None,
            torrent: Some(entry.enclosure),
            author: None,
        });
    }
    Ok(items)
}

/// 从 `https://bangumi.moe/torrent/5f00...` 取最后一段当 torrent id。
pub fn torrent_id(link: &str) -> String {
    link.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

pub fn describe(err: Error) -> String {
    format!("bangumi.moe：{err}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
  <item>
    <title>[喵萌奶茶屋&amp;LoliHouse] 某番 - 05 [1080p][简日内嵌]</title>
    <link>https://bangumi.moe/torrent/65f0a1b2c3d4e5f6</link>
    <pubDate>Sun, 25 Oct 2026 13:00:00 +0000</pubDate>
    <enclosure url="https://bangumi.moe/download/torrent/65f0a1b2c3d4e5f6.torrent" length="0" type="application/x-bittorrent"/>
  </item>
  <item>
    <title>[ANi] 某番 - 05 [1080P][CHT]</title>
    <link>https://bangumi.moe/torrent/aaaabbbbccccdddd</link>
    <pubDate>Sun, 25 Oct 2026 14:00:00 +0000</pubDate>
    <enclosure url="https://bangumi.moe/download/torrent/aaaabbbbccccdddd.torrent"/>
  </item>
</channel></rss>"#;

    #[test]
    fn keeps_both_and_fills_torrent_fields() {
        let items = from_xml(FEED, 0).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id.as_deref(), Some("65f0a1b2c3d4e5f6"));
        assert_eq!(items[0].kind, RssKind::Bangumi);
        assert_eq!(items[0].magnet, None);
        assert!(
            items[0]
                .torrent
                .as_deref()
                .unwrap()
                .ends_with("65f0a1b2c3d4e5f6.torrent")
        );
        // 标题里的 `&amp;` 要还原成 `&`。
        assert!(items[0].title.contains("喵萌奶茶屋&LoliHouse"));
        assert_eq!(items[0].pub_date, "2026年10月25日 09:00PM");
    }

    #[test]
    fn torrent_id_survives_trailing_slash() {
        assert_eq!(torrent_id("https://bangumi.moe/torrent/abc/"), "abc");
        assert_eq!(torrent_id("abc"), "abc");
    }
}
