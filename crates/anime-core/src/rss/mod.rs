//! RSS 三源：抓取 → 过滤 → 合并去重 → 构建可入库条目。
//!
//! TS 版是五个文件（`rss/index.ts` + 三个源 + `common.ts` + `rssItemHandler.ts`），
//! 这里合成一个模块，因为真正的差别只有「作者白名单」和「时间怎么格式化」：
//!
//! ```text
//! fetch_all()          顺序无所谓：三源并行抓，任何一个挂了只影响它自己
//!   ├─ bangumi::fetch  bangumi.moe 站内 RSS（无作者，只按标题过滤）
//!   ├─ dmhy::fetch     动漫花园（账号映射表 = 白名单，LoliHouse 特例）
//!   └─ acgnx::fetch    末日动漫（作者映射表 = 白名单）
//! merge()              按 bangumi > dmhy > acgnx 去重，先过黑名单/合集/字幕形态
//! build_item()         抽字幕组 + 标题解析 → AnimeItem
//! enrich_bangumi()     bangumi.moe 条目补磁力/发布组/多语言番名（要打 API）
//! ```
//!
//! 「去重」和「抓取」是分开的纯函数，所以合并优先级、拉黑、合集过滤都能用固定
//! fixture 单测，不需要网络。

pub mod acgnx;
pub mod bangumi;
pub mod bangumi_moe;
pub mod common;
pub mod dmhy;
pub mod http;
pub mod xml;

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::LazyLock;

use regex::Regex;

use crate::logger::Logger;
use crate::models::{AnimeItem, RssItem, RssKind};
use crate::title::{self, clean_title, normalize_title};
use crate::{Result, log};

pub use common::{format_pub_date_local, format_pub_date_utc8, parse_pub_date_ms, skip_reason};
pub use http::Http;

/// 单个源的抓取结果。
#[derive(Debug, Clone)]
pub struct SourceReport {
    pub kind: RssKind,
    /// 过滤后留下的条数。
    pub count: usize,
    /// 抓取失败的原因（成功时为 `None`）。
    pub error: Option<String>,
}

/// 一轮抓取的汇总。
#[derive(Debug, Clone, Default)]
pub struct FetchReport {
    /// 合并去重后、按发布时间倒序的条目。
    pub items: Vec<RssItem>,
    pub sources: Vec<SourceReport>,
}

impl FetchReport {
    /// 有没有源失败了（插件壳据此决定要不要发告警）。
    pub fn failures(&self) -> impl Iterator<Item = &SourceReport> {
        self.sources.iter().filter(|report| report.error.is_some())
    }
}

/// 抓三个源并合并。
///
/// 并行用 `std::thread::scope`：插件壳调用时本来就在自己的线程里（同步服务方法），
/// 三个源各 15s 超时，串行最坏要 45s，并行只要 15s。任何一个源失败都不影响其他源——
/// TS 版用的是 `Promise.allSettled`，语义一样。
pub fn fetch_all(
    http: &Http,
    max_items: usize,
    blacklist: &[String],
    logger: &Logger,
) -> FetchReport {
    let (bangumi_result, dmhy_result, acgnx_result) = std::thread::scope(|scope| {
        let bangumi = scope.spawn(|| guarded("bangumi.moe", || bangumi::fetch(http, max_items)));
        let dmhy = scope.spawn(|| guarded("动漫花园", || dmhy::fetch(http, max_items)));
        let acgnx = scope.spawn(|| guarded("末日动漫", || acgnx::fetch(http, max_items)));
        (
            bangumi
                .join()
                .unwrap_or_else(|_| Err("bangumi.moe：抓取线程 panic".to_owned())),
            dmhy.join()
                .unwrap_or_else(|_| Err("动漫花园：抓取线程 panic".to_owned())),
            acgnx
                .join()
                .unwrap_or_else(|_| Err("末日动漫：抓取线程 panic".to_owned())),
        )
    });

    let mut sources = Vec::with_capacity(3);
    let mut collected: Vec<(RssKind, Vec<RssItem>)> = Vec::with_capacity(3);
    for (kind, result) in [
        (RssKind::Bangumi, bangumi_result),
        (RssKind::Dmhy, dmhy_result),
        (RssKind::Acgnx, acgnx_result),
    ] {
        match result {
            Ok(items) => {
                log!(logger, Debug, "{}：{} 条", kind.as_str(), items.len());
                sources.push(SourceReport {
                    kind,
                    count: items.len(),
                    error: None,
                });
                collected.push((kind, items));
            }
            Err(message) => {
                log!(logger, Warn, "RSS 源失败：{message}");
                sources.push(SourceReport {
                    kind,
                    count: 0,
                    error: Some(message),
                });
                collected.push((kind, Vec::new()));
            }
        }
    }

    FetchReport {
        items: merge(&collected, blacklist, logger),
        sources,
    }
}

/// 包一层，把 panic 和错误统一成「一句话原因」。
fn guarded(
    label: &str,
    fetch: impl FnOnce() -> Result<Vec<RssItem>>,
) -> std::result::Result<Vec<RssItem>, String> {
    match std::panic::catch_unwind(AssertUnwindSafe(fetch)) {
        Ok(Ok(items)) => Ok(items),
        Ok(Err(err)) => Err(format!("{label}：{err}")),
        Err(_) => Err(format!("{label}：抓取时 panic")),
    }
}

/// 合并去重：同一标题只留优先级最高的源（bangumi > dmhy > acgnx）。
///
/// 三个细节按 TS 版原样保留：
/// 1. 去重键用 **归一化后的原标题**（清理空白之前），展示用清理后的标题；
/// 2. 黑名单是「标题包含关键词」而不是精确等于；
/// 3. 结果按发布时间倒序，解析不了的时间算最旧。
pub fn merge(
    sources: &[(RssKind, Vec<RssItem>)],
    blacklist: &[String],
    logger: &Logger,
) -> Vec<RssItem> {
    let mut ordered: Vec<&(RssKind, Vec<RssItem>)> = sources.iter().collect();
    // 优先级高的后处理 → 覆盖低的。同优先级保持输入顺序。
    ordered.sort_by_key(|(kind, _)| kind.priority());

    let mut merged: HashMap<String, RssItem> = HashMap::new();
    for (_, items) in ordered {
        for item in items {
            if let Some(reason) = skip_reason(&item.title, blacklist) {
                log!(logger, Debug, "跳过（{reason}）：{}", item.title);
                continue;
            }
            let key = normalize_title(&item.title);
            let mut kept = item.clone();
            kept.title = clean_title(&item.title);
            merged.insert(key, kept);
        }
    }

    let mut items: Vec<RssItem> = merged.into_values().collect();
    items.sort_by_key(|item| std::cmp::Reverse(parse_pub_date_ms(&item.pub_date)));
    items
}

/// 从标题开头抽字幕组：`[组]` / `【组】`，多个组用 `&` `|` `/` `、` 分隔。
///
/// 没有开头的组标签就返回空——这条没法按组套规则，直接丢（TS 版同样 `return`）。
pub fn fansub_of(title: &str) -> Vec<String> {
    static LEADING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?:\[([^\]]+)\]|【([^】]+)】)").expect("内置正则必须合法"));
    static SPLIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*[&/|｜、]\s*").expect("内置正则必须合法"));

    let Some(caps) = LEADING.captures(title) else {
        return Vec::new();
    };
    let raw = caps
        .get(1)
        .or_else(|| caps.get(2))
        .map(|group| group.as_str().trim())
        .unwrap_or_default();
    SPLIT
        .split(raw)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 构建可入库条目：抽字幕组 → 套该组的标题规则 → 得到番名/集数/片源。
///
/// `team` 是用来查规则的发布组名（bangumi.moe 的组名要从 API 取，dmhy/acgnx 用作者名）。
/// 返回 `None` 表示「这条没法用」：没有组标签、组不在规则表里、或解析不出番名。
pub fn build_item(item: &RssItem, team: Option<&str>) -> Option<AnimeItem> {
    let fansub = fansub_of(&item.title);
    if fansub.is_empty() {
        return None;
    }
    let team = team
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| item.author.clone())
        .or_else(|| fansub.first().cloned())?;

    let info = title::parse_info(&item.title, Some(&team))?;
    if !info.is_usable() {
        return None;
    }
    let magnet = item.download_source()?.trim().to_owned();
    if magnet.is_empty() {
        return None;
    }

    Some(AnimeItem {
        title: item.title.clone(),
        pub_date: item.pub_date.clone(),
        magnet,
        link: item.link.clone(),
        team,
        fansub,
        names: info.names,
        source: Some(info.source).filter(|source| !source.is_empty()),
        episode: Some(info.episode),
    })
}

/// bangumi.moe 条目专用：补磁力、发布组名、多语言番名，再走 [`build_item`]。
///
/// API 挂了不算致命：RSS 里的 `.torrent` 地址本身就是可用的下载地址，
/// 只是少一次「番名对齐」。这种情况打 warn 然后降级。
pub fn enrich_bangumi(http: &Http, item: &RssItem, logger: &Logger) -> Option<AnimeItem> {
    let torrent = match item.id.as_deref() {
        Some(id) => match bangumi_moe::fetch_torrent(http, id) {
            Ok(info) => Some(info),
            Err(err) => {
                log!(
                    logger,
                    Warn,
                    "bangumi.moe 拉种子详情失败，回退用 RSS 的 .torrent 地址：{err}"
                );
                None
            }
        },
        None => None,
    };

    let mut team = None;
    let mut locale_names = Vec::new();
    if let Some(info) = &torrent {
        if let Some(team_id) = &info.team_id {
            match bangumi_moe::fetch_teams(http, std::slice::from_ref(team_id)) {
                Ok(teams) => {
                    team = teams
                        .into_iter()
                        .map(|team| team.name.trim().to_owned())
                        .find(|name| !name.is_empty());
                }
                Err(err) => log!(logger, Warn, "bangumi.moe 拉发布组失败：{err}"),
            }
        }
        if !info.tag_ids.is_empty() {
            match bangumi_moe::fetch_tags(http, &info.tag_ids) {
                Ok(tags) => locale_names = bangumi_moe::bangumi_names(&tags),
                Err(err) => log!(logger, Warn, "bangumi.moe 拉标签失败：{err}"),
            }
        }
    }

    let mut built = build_item(item, team.as_deref())?;
    if let Some(magnet) = torrent
        .as_ref()
        .map(|info| info.magnet.trim())
        .filter(|m| !m.is_empty())
    {
        built.magnet = magnet.to_owned();
    }
    for name in locale_names {
        if !built.names.contains(&name) {
            built.names.push(name);
        }
    }
    Some(built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger;

    fn item(kind: RssKind, title: &str, date: &str) -> RssItem {
        RssItem {
            kind,
            id: None,
            title: title.to_owned(),
            link: "https://example.com/1".to_owned(),
            pub_date: date.to_owned(),
            magnet: Some("magnet:?xt=urn:btih:x".to_owned()),
            torrent: None,
            author: Some("ANi".to_owned()),
        }
    }

    #[test]
    fn higher_priority_source_wins_on_same_title() {
        let sources = vec![
            (
                RssKind::Bangumi,
                vec![item(
                    RssKind::Bangumi,
                    "[ANi] 某番 - 05 [1080P]",
                    "2026年10月25日 09:00PM",
                )],
            ),
            (
                RssKind::Dmhy,
                vec![item(
                    RssKind::Dmhy,
                    "[ANi] 某番 - 05 [1080P]",
                    "2026年10月25日 08:00PM",
                )],
            ),
            (
                RssKind::Acgnx,
                vec![item(
                    RssKind::Acgnx,
                    "[ANi] 某番 - 05 [1080P]",
                    "2026年10月25日 07:00PM",
                )],
            ),
        ];
        let merged = merge(&sources, &[], &logger::noop());
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].kind, RssKind::Bangumi);
    }

    #[test]
    fn low_priority_still_kept_when_title_differs_only_by_width() {
        // 归一化之后是全角/半角差异 → 视为同一条，只留 dmhy。
        let sources = vec![
            (
                RssKind::Dmhy,
                vec![item(
                    RssKind::Dmhy,
                    "[ANi] 某番 - 05 [1080P]",
                    "2026年10月25日 08:00PM",
                )],
            ),
            (
                RssKind::Acgnx,
                vec![item(
                    RssKind::Acgnx,
                    "[ＡＮｉ] 某番 - 05 [1080P]",
                    "2026年10月25日 07:00PM",
                )],
            ),
        ];
        let merged = merge(&sources, &[], &logger::noop());
        assert_eq!(merged.len(), 1, "{merged:#?}");
        assert_eq!(merged[0].kind, RssKind::Dmhy);
    }

    #[test]
    fn merged_items_sort_newest_first() {
        let sources = vec![(
            RssKind::Dmhy,
            vec![
                item(
                    RssKind::Dmhy,
                    "[ANi] 某番 - 04 [1080P]",
                    "2026年10月24日 08:00PM",
                ),
                item(
                    RssKind::Dmhy,
                    "[ANi] 某番 - 05 [1080P]",
                    "2026年10月25日 08:00PM",
                ),
            ],
        )];
        let merged = merge(&sources, &[], &logger::noop());
        assert!(merged[0].title.contains("05"));
        assert!(merged[1].title.contains("04"));
    }

    #[test]
    fn blacklist_and_collections_are_dropped_before_dedup() {
        let sources = vec![(
            RssKind::Dmhy,
            vec![
                item(
                    RssKind::Dmhy,
                    "[ANi] 不看的番 - 05 [1080P]",
                    "2026年10月25日 08:00PM",
                ),
                item(
                    RssKind::Dmhy,
                    "[ANi] 某番 [01-12][1080P]",
                    "2026年10月25日 08:00PM",
                ),
                item(
                    RssKind::Dmhy,
                    "[ANi] 某番 - 05 [1080P]",
                    "2026年10月25日 08:00PM",
                ),
            ],
        )];
        let merged = merge(&sources, &["不看的番".to_owned()], &logger::noop());
        assert_eq!(merged.len(), 1);
        assert!(merged[0].title.contains("05"));
    }

    #[test]
    fn fansub_splits_multiple_groups() {
        assert_eq!(
            fansub_of("[喵萌奶茶屋&LoliHouse] 某番 - 05"),
            vec!["喵萌奶茶屋", "LoliHouse"]
        );
        assert_eq!(fansub_of("【ANi】某番 - 05"), vec!["ANi"]);
        assert_eq!(
            fansub_of("[悠哈C9字幕社/星空字幕组] 某番"),
            vec!["悠哈C9字幕社", "星空字幕组"]
        );
        assert!(fansub_of("没有组标签的标题").is_empty());
    }

    #[test]
    fn build_item_uses_team_rules_and_falls_back_to_author() {
        let dmhy_item = item(
            RssKind::Dmhy,
            "[ANi] 葬送的芙莉莲 - 05 [1080P][Baha][WEB-DL]",
            "2026年10月25日 08:00PM",
        );
        let built = build_item(&dmhy_item, None).expect("ANi 的规则应该能解析");
        assert_eq!(built.team, "ANi");
        assert_eq!(built.fansub, vec!["ANi"]);
        assert!(!built.names.is_empty());
        assert_eq!(built.episode.as_deref(), Some("05"));
        assert_eq!(built.source.as_deref(), Some("Baha"));
    }

    #[test]
    fn build_item_rejects_unknown_group_and_missing_magnet() {
        let mut unknown = item(
            RssKind::Dmhy,
            "[没听过的组] 某番 - 05",
            "2026年10月25日 08:00PM",
        );
        unknown.author = Some("没听过的组".to_owned());
        assert!(build_item(&unknown, None).is_none());

        let mut no_magnet = item(
            RssKind::Bangumi,
            "[ANi] 某番 - 05 [1080P]",
            "2026年10月25日 08:00PM",
        );
        no_magnet.magnet = None;
        no_magnet.torrent = None;
        assert!(build_item(&no_magnet, None).is_none());
    }

    #[test]
    fn bangumi_items_download_from_torrent_url() {
        let mut bangumi_item = item(
            RssKind::Bangumi,
            "[ANi] 葬送的芙莉莲 - 05 [1080P]",
            "2026年10月25日 08:00PM",
        );
        bangumi_item.magnet = None;
        bangumi_item.torrent = Some("https://bangumi.moe/download/torrent/abc.torrent".to_owned());
        let built = build_item(&bangumi_item, Some("ANi")).expect("应该能构建");
        assert_eq!(
            built.magnet,
            "https://bangumi.moe/download/torrent/abc.torrent"
        );
    }
}
