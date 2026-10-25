//! RSS 2.0 → [`Entry`]：只认三源实际会用到的那五个标签。
//!
//! 三个源的 XML 长得都一样（`channel > item`，标题在 `title`，磁力在 `enclosure@url`），
//! 差别全在「填哪些字段、怎么过滤」，所以解析只做一次，源模块只做映射。
//!
//! 三个容易踩的点（都踩过）：
//! - 文本可能是 `<![CDATA[...]]>`（dmhy 全用 CDATA），也可能是转义实体（acgnx 混用）；
//! - `&amp;` 这类实体在 quick-xml 0.37+ 是**独立事件**（`Event::GeneralRef`），
//!   只收 `Text` 会把 `[A&amp;B]` 悄悄变成 `[A`；
//! - `enclosure` 在 dmhy / acgnx 里是**自闭合**标签，只有属性没有文本。
//!
//! 因为实体把一段文本切成了好几个事件，这里按「元素」攒缓冲区，遇到结束标签才落盘；
//! 也正因为如此**不能开 `trim_text`**（会把实体两侧的空格吃掉）。

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};

use crate::{Error, Result};

/// 一条原始 RSS 条目。字段一律 `String`（空串表示该源没给），
/// 由各源模块决定「空串算不算缺失」。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    pub link: String,
    pub pub_date: String,
    pub author: String,
    pub category: String,
    /// `enclosure@url`：dmhy / acgnx 是磁力，bangumi.moe 是 `.torrent` 地址。
    pub enclosure: String,
}

/// 解析整段 RSS，按文档顺序返回全部 `channel > item`。
///
/// 解析失败一律报错（不静默返回空列表）：源挂了应该在日志里看得见，
/// 而不是表现为「今天没有新番」。
pub fn parse(xml: &str) -> Result<Vec<Entry>> {
    let mut reader = Reader::from_str(xml);

    let mut entries = Vec::new();
    let mut current: Option<Entry> = None;
    let mut field: Option<Field> = None;
    let mut buffer = String::new();

    loop {
        let event = reader.read_event().map_err(|err| Error::Parse {
            what: "RSS XML",
            message: err.to_string(),
        })?;
        match event {
            Event::Eof => break,
            Event::Start(start) => match start.name().as_ref() {
                "item" => {
                    current = Some(Entry::default());
                    field = None;
                    buffer.clear();
                }
                "enclosure" => {
                    if let Some(entry) = current.as_mut() {
                        entry.enclosure = attribute(&start, "url");
                    }
                }
                name => {
                    if current.is_some() {
                        field = Field::of(name);
                        buffer.clear();
                    }
                }
            },
            // `<enclosure url="..." type="..."/>`
            Event::Empty(start) => {
                if start.name().as_ref() == "enclosure"
                    && let Some(entry) = current.as_mut()
                {
                    entry.enclosure = attribute(&start, "url");
                }
            }
            Event::Text(text) => {
                if field.is_some() {
                    let raw = text.into_inner();
                    // 实体解码失败（源里写了裸 `&`）不能让整条 feed 挂掉：退回原文。
                    match unescape(&raw) {
                        Ok(decoded) => buffer.push_str(&decoded),
                        Err(_) => buffer.push_str(&raw),
                    }
                }
            }
            // `&amp;` / `&#38;`：独立事件，不处理会静默丢字符。
            Event::GeneralRef(reference) => {
                if field.is_some() {
                    let text = match reference.resolve_char_ref() {
                        Ok(Some(ch)) => ch.to_string(),
                        _ => match reference.into_inner().as_ref() {
                            "amp" => "&".to_owned(),
                            "lt" => "<".to_owned(),
                            "gt" => ">".to_owned(),
                            "quot" => "\"".to_owned(),
                            "apos" => "'".to_owned(),
                            name => format!("&{name};"),
                        },
                    };
                    buffer.push_str(&text);
                }
            }
            Event::CData(data) => {
                if field.is_some() {
                    buffer.push_str(&data.into_inner());
                }
            }
            Event::End(end) => match end.name().as_ref() {
                "item" => {
                    if let Some(entry) = current.take() {
                        entries.push(entry);
                    }
                    field = None;
                    buffer.clear();
                }
                _ => {
                    if let (Some(target), Some(entry)) = (field.take(), current.as_mut()) {
                        target.set(entry, buffer.trim());
                    }
                    buffer.clear();
                }
            },
            _ => {}
        }
    }

    Ok(entries)
}

/// item 里我们关心的那几个子标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    Link,
    PubDate,
    Author,
    Category,
}

impl Field {
    fn of(name: &str) -> Option<Self> {
        match name {
            "title" => Some(Self::Title),
            "link" => Some(Self::Link),
            "pubDate" => Some(Self::PubDate),
            "author" => Some(Self::Author),
            "category" => Some(Self::Category),
            _ => None,
        }
    }

    /// 这些标签在 RSS 里都是单值；真出现重复（多分类）时以第一个为准，
    /// 不做拼接——拼出来的 `動畫 季度全集` 会让下游的字符串比较失效。
    fn set(self, entry: &mut Entry, text: &str) {
        let slot = match self {
            Self::Title => &mut entry.title,
            Self::Link => &mut entry.link,
            Self::PubDate => &mut entry.pub_date,
            Self::Author => &mut entry.author,
            Self::Category => &mut entry.category,
        };
        if slot.is_empty() && !text.is_empty() {
            slot.push_str(text);
        }
    }
}

fn attribute(start: &BytesStart<'_>, key: &str) -> String {
    start
        .attributes()
        .flatten()
        .find(|attr| attr.key.as_ref() == key)
        .and_then(|attr| attr.normalized_value(XmlVersion::Explicit1_0).ok())
        .map(|value| value.trim().to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dmhy 真实结构的裁剪版：CDATA 标题 + 自闭合 enclosure + 分类。
    const DMHY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title><![CDATA[動畫-動漫花園資源網]]></title>
    <link>http://share.dmhy.org</link>
    <item>
      <title><![CDATA[[ANi] 葬送的芙莉莲 - 05 [1080P][Baha][WEB-DL][AAC AVC][CHT]]]></title>
      <link>https://share.dmhy.org/topics/view/1.html</link>
      <pubDate>Sun, 25 Oct 2026 21:00:00 +0800</pubDate>
      <author><![CDATA[ANiTorrent]]></author>
      <category domain="http://share.dmhy.org/topics/list/sort_id/2"><![CDATA[動畫]]></category>
      <category><![CDATA[季度全集]]></category>
      <enclosure url="magnet:?xt=urn:btih:abc&amp;dn=&amp;tr=udp" type="application/x-bittorrent" length="0"/>
    </item>
    <item>
      <title>第二条 &amp; 带实体</title>
      <link>https://share.dmhy.org/topics/view/2.html</link>
      <pubDate>Sun, 25 Oct 2026 22:00:00 +0800</pubDate>
      <author>LoliHouse</author>
      <category>動畫</category>
      <enclosure url="magnet:?xt=urn:btih:def"/>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_cdata_and_self_closing_enclosure() {
        let entries = parse(DMHY).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].title,
            "[ANi] 葬送的芙莉莲 - 05 [1080P][Baha][WEB-DL][AAC AVC][CHT]"
        );
        assert_eq!(entries[0].author, "ANiTorrent");
        // 多分类只取第一个，且 CDATA 内容要保住。
        assert_eq!(entries[0].category, "動畫");
        // 属性里的实体要还原（磁力链接全靠这步）。
        assert_eq!(entries[0].enclosure, "magnet:?xt=urn:btih:abc&dn=&tr=udp");
        assert_eq!(entries[0].pub_date, "Sun, 25 Oct 2026 21:00:00 +0800");
    }

    #[test]
    fn decodes_entities_in_plain_text_without_losing_spaces() {
        let entries = parse(DMHY).unwrap();
        assert_eq!(entries[1].title, "第二条 & 带实体");
    }

    #[test]
    fn broken_xml_is_an_error_not_an_empty_list() {
        let err = parse("<rss><channel><item></channel></rss>").unwrap_err();
        assert!(matches!(err, Error::Parse { .. }), "{err}");
    }

    #[test]
    fn empty_document_yields_nothing() {
        assert!(
            parse("<rss version=\"2.0\"><channel></channel></rss>")
                .unwrap()
                .is_empty()
        );
    }
}
