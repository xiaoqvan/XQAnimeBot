//! 按 URL 取种子信息（TS `anime/get.ts`）。
//!
//! `/addanime` / `/addnewanime` 收的是「章节 id + 一个页面链接」，得先从那个页面
//! 里拿到**标题 / 磁力 / 发布组 / 发布时间**，才能走后面的解析 → 下载 → 发送。
//!
//! 两家源：
//!
//! ```text
//! https://bangumi.moe/torrent/<id>   →  POST /api/torrent/fetch {_id}
//!                                       再 POST /api/team/fetch、/api/tag/fetch 补发布组与译名
//! https://share.dmhy.org/topics/view/… →  抓 HTML，按标签抠字段
//! ```
//!
//! ## 为什么 dmhy 只做「够用」的抓取
//!
//! TS 用 cheerio 按 CSS 选择器取字段，那些选择器极其脆弱（改版一次就全空）。
//! 这里改成按**稳定标记**抠：`id="a_magnet"` 的 `href` 是磁力（页面上唯一），
//! 标题取 `<title>` 标签；取不到就报错，而不是返回「未知标题」继续跑——
//! 标题错了后面整条链路（番名解析、集数匹配）都会错，宁可早失败。

use std::thread::sleep;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::rss::Http;
use crate::{Error, Result};

/// bangumi.moe（注意不是 api.bgm.tv，也不是 bgm.tv）。
const BANGUMI_MOE: &str = "https://bangumi.moe";

/// 浏览器 UA：这两个站会挡没有 UA 的请求（TS 也是塞的浏览器 UA）。
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36 Edg/138.0.0.0";

/// 重试次数与间隔（TS `retryRequest`：3 次、间隔 10 秒）。
pub const RETRY_ATTEMPTS: u32 = 3;
pub const RETRY_INTERVAL: Duration = Duration::from_secs(10);

/// 用户给的那个链接属于哪家。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorrentSource {
    /// `…/torrent/<id>`：id 已抠出来。
    Bangumi { id: String },
    /// `dmhy.org/topics/view/…`：整条 URL 原样用。
    Dmhy { url: String },
}

/// 认链接。认不出返回 `None`（调用方据此回「目前仅支持 bangumi 和 dmhy」）。
///
/// 判断方式与 TS 一致——`url.includes("bangumi")` / `includes("dmhy")`，
/// 而不是解析域名：这两家的域名有国际版/镜像，`includes` 反而更稳。
pub fn parse_source(url: &str) -> Option<TorrentSource> {
    if url.contains("bangumi") {
        // `/torrent/<id>`：id 后面还可能有路径或查询串，再切一次。
        let rest = url.split("/torrent/").nth(1)?;
        let id = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if id.is_empty() {
            return None;
        }
        Some(TorrentSource::Bangumi { id: id.to_owned() })
    } else if url.contains("dmhy") {
        if !url.contains("dmhy.org/topics/view/") {
            return None;
        }
        Some(TorrentSource::Dmhy {
            url: url.to_owned(),
        })
    } else {
        None
    }
}

/// 从标题开头的 `[组名]` / `【组名】` 里拆出字幕组（可能多个）。
///
/// 分隔符照 TS：`&` `/` `|` `｜` `、`，两边空白也吃掉。
pub fn fansub_from_title(title: &str) -> Vec<String> {
    let raw = match bracket_prefix(title) {
        Some(raw) => raw,
        None => return Vec::new(),
    };
    raw.split(['&', '/', '|', '｜', '、'])
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 标题开头那对方括号/六角括号里的内容。
fn bracket_prefix(title: &str) -> Option<&str> {
    let body = title.strip_prefix('[').or_else(|| title.strip_prefix('【'))?;
    let end = if title.starts_with('[') {
        body.find(']')
    } else {
        body.find('】')
    }?;
    Some(&body[..end])
}

/// bangumi.moe 的种子。字段可能缺，全用 `Option` / `default` 兜住。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BangumiTorrent {
    pub title: String,
    pub magnet: String,
    pub pub_date: Option<String>,
    pub team_id: Option<String>,
    pub tag_ids: Vec<String>,
}

/// bangumi.moe 的发布组（`_ids` 返回数组，取第一个）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BangumiTeam {
    pub name: String,
}

/// bangumi.moe 的标签：只要 `type == "bangumi"` 那条的多语言译名。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BangumiTag {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub locale: Option<BangumiLocale>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BangumiLocale {
    pub zh_cn: Option<String>,
    pub ja: Option<String>,
    pub en: Option<String>,
}

impl BangumiLocale {
    /// 三个译名里非空的那些（去重、去空、去首尾空白）。
    pub fn names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for name in [&self.zh_cn, &self.ja, &self.en]
            .into_iter()
            .flatten()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
        {
            if !names.iter().any(|existing: &String| existing == name) {
                names.push(name.to_owned());
            }
        }
        names
    }
}

/// dmhy 页面上抠出来的字段。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DmhyTorrent {
    pub title: String,
    pub magnet: String,
    pub team: String,
    pub pub_date: String,
}

/// 重试包装（TS `retryRequest`）：失败就睡一觉再来，最后一次的错误原样返回。
///
/// 只用于「偶发失败值得再试」的外部请求（这两个站经常 502）。
/// **不要**拿它包写库操作——那时重试可能把同一条写两遍。
pub fn retry<T>(attempt: impl FnMut() -> Result<T>) -> Result<T> {
    retry_with(RETRY_ATTEMPTS, RETRY_INTERVAL, attempt)
}

/// 显式给次数与间隔的版本：生产用 [retry]，测试用这个（间隔给 0，别真睡 20 秒）。
pub fn retry_with<T>(
    attempts: u32,
    interval: Duration,
    mut attempt: impl FnMut() -> Result<T>,
) -> Result<T> {
    let mut last = None;
    for round in 0..attempts.max(1) {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error) => {
                last = Some(error);
                // 最后一次不用睡：睡完还是要把错误交出去，白等一轮。
                if round + 1 < attempts {
                    sleep(interval);
                }
            }
        }
    }
    Err(last.expect("attempts 至少为 1"))
}

/// 建一个带浏览器 UA 的 `Http`。
fn http() -> Http {
    Http::with_user_agent(BROWSER_UA, Duration::from_secs(20))
}

/// 取 bangumi.moe 的种子信息。
pub fn fetch_bangumi_torrent(id: &str) -> Result<BangumiTorrent> {
    let text = retry(|| {
        http()
            .post_json(&format!("{BANGUMI_MOE}/api/torrent/fetch"), &json!({ "_id": id }))
            .map(|value| value.to_string())
    })?;
    serde_json::from_str(&text).map_err(|err| Error::Parse {
        what: "bangumi.moe torrent",
        message: err.to_string(),
    })
}

/// 取 bangumi.moe 的发布组（返回数组，可能为空）。
pub fn fetch_bangumi_team(team_id: &str) -> Result<Vec<BangumiTeam>> {
    let text = retry(|| {
        http()
            .post_json(
                &format!("{BANGUMI_MOE}/api/team/fetch"),
                &json!({ "_ids": [team_id] }),
            )
            .map(|value| value.to_string())
    })?;
    serde_json::from_str(&text).map_err(|err| Error::Parse {
        what: "bangumi.moe team",
        message: err.to_string(),
    })
}

/// 取 bangumi.moe 的标签。
pub fn fetch_bangumi_tags(tag_ids: &[String]) -> Result<Vec<BangumiTag>> {
    let text = retry(|| {
        http()
            .post_json(
                &format!("{BANGUMI_MOE}/api/tag/fetch"),
                &json!({ "_ids": tag_ids }),
            )
            .map(|value| value.to_string())
    })?;
    serde_json::from_str(&text).map_err(|err| Error::Parse {
        what: "bangumi.moe tag",
        message: err.to_string(),
    })
}

/// 抓 dmhy 页面，抠出标题 / 磁力 / 发布组 / 发布时间。
pub fn fetch_dmhy_torrent(url: &str) -> Result<DmhyTorrent> {
    let html = retry(|| http().get_with(url, None))?;
    let title = tag_text(&html, "title").ok_or_else(|| Error::Parse {
        what: "dmhy 标题",
        message: format!("页面里没有 <title>：{url}"),
    })?;
    // 磁力在 `id="a_magnet"` 的 href 上：这是页面上唯一带这个 id 的元素。
    let magnet = attr_of(&html, "a_magnet", "href").ok_or_else(|| Error::Parse {
        what: "dmhy 磁力",
        message: format!("页面里找不到磁力链接：{url}"),
    })?;
    // 发布组从标题开头的方括号里取（TS 是从侧边栏抠的，那些选择器太脆）。
    // 取不到就是空串——后面 `parse_info` 会因此找不到字幕组规则，宁可早失败
    // 也别拿一个错组名去套规则。
    let team = fansub_from_title(&title).first().cloned().unwrap_or_default();
    Ok(DmhyTorrent {
        title,
        magnet,
        team,
        pub_date: String::new(),
    })
}

/// 取某个标签里的纯文本（去标签、去首尾空白）。
///
/// 只用于 `<title>` 这种**页面上只有一个**的标签——正文里同名的标签很多，
/// 取第一个就够，用不着完整的 HTML 解析。
fn tag_text(html: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = html.find(&open)?;
    let after = html[start..].find('>')? + start + 1;
    let close = format!("</{tag}>");
    let end = html[after..].find(&close)? + after;
    let text = html[after..end]
        .replace("<br>", " ")
        .replace("<br/>", " ");
    let text = strip_tags(&text).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// 在含 `id="<needle>"` 的那个标签里取属性值。
fn attr_of(html: &str, needle: &str, attr: &str) -> Option<String> {
    let anchor = format!("id=\"{needle}\"");
    let cursor = html.find(&anchor)?;
    // 属性值可能在 id 前面：往回退到这个标签的开头。
    let tag_start = html[..cursor].rfind('<')?;
    let tag_end = html[cursor..].find('>')? + cursor;
    let tag = &html[tag_start..=tag_end];
    let key = format!("{attr}=");
    let value_start = tag.find(&key)? + key.len();
    let rest = &tag[value_start..];
    let quote = rest.chars().next()?;
    let body = if quote == '"' || quote == '\'' {
        &rest[1..]
    } else {
        rest
    };
    let end = if quote == '"' || quote == '\'' {
        body.find(quote)?
    } else {
        body.find([' ', '>']).unwrap_or(body.len())
    };
    Some(body[..end].to_owned())
}

/// 去掉 `<...>`。
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for ch in text.chars() {
        match ch {
            '<' => inside = true,
            '>' => inside = false,
            other if !inside => out.push(other),
            _ => {}
        }
    }
    out
}

/// 把标签里 `type == "bangumi"` 那条的译名取出来。
pub fn locale_names(tags: &[BangumiTag]) -> Vec<String> {
    tags.iter()
        .find(|tag| tag.kind.as_deref() == Some("bangumi"))
        .and_then(|tag| tag.locale.as_ref())
        .map(BangumiLocale::names)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bangumi_torrent_urls() {
        assert_eq!(
            parse_source("https://bangumi.moe/torrent/68a1c2"),
            Some(TorrentSource::Bangumi {
                id: "68a1c2".to_owned()
            })
        );
        // id 后面可能还有路径或查询串。
        assert_eq!(
            parse_source("https://bangumi.moe/torrent/68a1c2?x=1"),
            Some(TorrentSource::Bangumi {
                id: "68a1c2".to_owned()
            })
        );
        // 没有 /torrent/ 就认不出：没法猜 id。
        assert_eq!(parse_source("https://bangumi.moe/"), None);
    }

    #[test]
    fn dmhy_only_accepts_topic_pages() {
        assert_eq!(
            parse_source("https://share.dmhy.org/topics/view/123_abc.html"),
            Some(TorrentSource::Dmhy {
                url: "https://share.dmhy.org/topics/view/123_abc.html".to_owned()
            })
        );
        // 首页/列表页不是种子页（TS 同判定：必须含 dmhy.org/topics/view/）。
        assert_eq!(parse_source("https://share.dmhy.org/"), None);
    }

    #[test]
    fn unrelated_urls_are_rejected() {
        assert_eq!(parse_source("https://example.com/a"), None);
    }

    #[test]
    fn fansub_splits_on_every_separator_ts_uses() {
        assert_eq!(
            fansub_from_title("[喵萌奶茶屋&LoliHouse] 某番 [01]"),
            vec!["喵萌奶茶屋".to_owned(), "LoliHouse".to_owned()]
        );
        assert_eq!(
            fansub_from_title("【悠哈C9字幕社｜喵萌】 某番"),
            vec!["悠哈C9字幕社".to_owned(), "喵萌".to_owned()]
        );
    }

    #[test]
    fn fansub_ignores_titles_without_a_leading_bracket() {
        // 括号不在开头 = 不是字幕组标记，TS 的正则也是 `^`。
        assert!(fansub_from_title("某番 [01]").is_empty());
        assert!(fansub_from_title("").is_empty());
    }

    #[test]
    fn locale_names_only_read_the_bangumi_tag() {
        let tags = vec![
            BangumiTag {
                kind: Some("other".to_owned()),
                locale: Some(BangumiLocale {
                    zh_cn: Some("错的名字".to_owned()),
                    ..Default::default()
                }),
            },
            BangumiTag {
                kind: Some("bangumi".to_owned()),
                locale: Some(BangumiLocale {
                    zh_cn: Some("葬送的芙莉莲".to_owned()),
                    ja: Some("葬送のフリーレン".to_owned()),
                    en: Some("  ".to_owned()),
                }),
            },
        ];
        assert_eq!(
            locale_names(&tags),
            vec!["葬送的芙莉莲".to_owned(), "葬送のフリーレン".to_owned()]
        );
        assert!(locale_names(&[BangumiTag::default()]).is_empty());
    }

    #[test]
    fn extractors_read_html_without_a_parser() {
        let html = r#"<html><head><title>  [组] 某番 [01][BIG5]  </title></head>
        <body><a id="a_magnet" href="magnet:?xt=urn:btih:ABC">磁力</a></body></html>"#;
        assert_eq!(tag_text(html, "title").as_deref(), Some("[组] 某番 [01][BIG5]"));
        assert_eq!(
            attr_of(html, "a_magnet", "href").as_deref(),
            Some("magnet:?xt=urn:btih:ABC")
        );
        // 属性在 id 之前也要能取到（HTML 属性顺序不保证）。
        let flipped = r#"<a href="magnet:?xt=1" id="a_magnet">x</a>"#;
        assert_eq!(attr_of(flipped, "a_magnet", "href").as_deref(), Some("magnet:?xt=1"));
    }

    #[test]
    fn dmhy_parse_fails_loudly_when_the_magnet_is_missing() {
        // 拿不到磁力就报错，而不是返回「未知磁力链接」继续跑——
        // 后面整条链路（解析、匹配、下载）都靠它。
        let html = "<html><head><title>某番</title></head><body>没有磁力</body></html>";
        assert!(attr_of(html, "a_magnet", "href").is_none());
    }

    #[test]
    fn retry_returns_the_last_error_after_three_attempts() {
        let mut rounds = 0;
        // 间隔给 0：这条用例不该真睡 20 秒。
        let result = retry_with(RETRY_ATTEMPTS, Duration::ZERO, || {
            rounds += 1;
            Err::<(), _>(Error::Parse {
                what: "x",
                message: format!("第 {rounds} 次"),
            })
        });
        assert_eq!(rounds, RETRY_ATTEMPTS);
        assert!(result.is_err());
    }

    #[test]
    fn retry_stops_at_the_first_success() {
        let mut rounds = 0;
        let value = retry_with(RETRY_ATTEMPTS, Duration::ZERO, || {
            rounds += 1;
            Ok::<u32, Error>(rounds)
        });
        assert_eq!(value.unwrap(), 1);
        assert_eq!(rounds, 1);
    }
}
