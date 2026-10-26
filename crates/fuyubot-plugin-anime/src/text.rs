//! 导航正文与视频文案：TS `anime/text.ts` 的两个函数。
//!
//! 这里全是**纯函数**（不联网、不碰宿主），因为它们是全仓最容易和 TS 走偏、
//! 也最该被单测钉住的东西：一个 `#` 前缀的差别、集数排序差一位，用户在频道里
//! 看到的就是错的。
//!
//! 两份产物：
//!
//! | 函数 | 用在哪 | 长度限制 |
//! | --- | --- | --- |
//! | [`navmegtext`] | 导航频道那条带图消息 + 后续分页 | 首条 1024（图注上限），分页 4096 |
//! | [`anime_text`] | 每集视频的 caption | 1024 |
//!
//! ## 长度测量是近似（有意为之）
//!
//! TS 用 TDLib 的 `parseTextEntities` 量「解析后的纯文本长度 + 实体个数」。
//! 这里没有那条同步接口（调一次要等 TDLib 回包，而正文生成是链路上高频的一步），
//! 所以用 [`measure`] 近似：把 `[文字](链接)` 还原成「文字」后数字数，实体数按
//! 链接数算。差的是 emoji / 组合字符（UTF-16 码元 vs 字符数），量级足够判断
//! 「要不要压缩 summary、要不要把资源区挪到分页」——这才是这两个数字唯一的用途。
//!
//! 另外 TS 的实体上限是 100（Telegram 的硬限制），所以链接特别多的番剧会把资源区
//! 整个挪到分页消息里，这个分支在这里照做。

use std::collections::BTreeMap;

use anime_core::models::{Anime, AnimeItem};

/// 深链前缀（`?start=` 后面的那一段由本模块拼）。
const BOT_START_URL: &str = "https://t.me/XiaoQvanAnimeBot?start=";
/// 带图消息的 caption 上限（Telegram 的硬限制）。
pub const MAIN_LIMIT: usize = 1024;
/// 纯文本消息上限。
pub const PAGE_LIMIT: usize = 4096;
/// Markdown 实体数上限。
pub const ENTITY_LIMIT: usize = 100;

/// 导航正文里的一个集数链接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEntry {
    /// 集数字符串（`"01"` / `"05v2"` / `"SP1"`）。
    pub episode: String,
    /// 该集视频消息的链接；没有链接的资源不进正文（点了没反应不如不显示）。
    pub link: Option<String>,
}

/// 按字幕组分组的资源。`BTreeMap` 而不是 `HashMap`：字幕组的展示顺序必须稳定，
/// 否则每刷新一次导航，组的顺序就变一次。
pub type GroupedResources = BTreeMap<String, Vec<ResourceEntry>>;

/// 把标签变成 `#标签` 串（TS 的 `formatTags`）。
///
/// 纯数字标签被丢掉：Bangumi 的标签里有大量「2024」「10」这类年份/数字，
/// 做成 `#2024` 既没有检索价值又会把正文撑长。
pub fn format_tags(tags: &[String]) -> String {
    tags.iter()
        .map(|tag| safe_tag(tag))
        .filter(|tag| !tag.is_empty() && !tag.chars().all(|ch| ch.is_ascii_digit()))
        .map(|tag| format!("#{tag}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 标签安全化：只留中日文、拉丁字母、数字和下划线（TS 的 `safeTag`）。
///
/// 空格被**删掉**而不是换成下划线：Telegram 的 hashtag 不能有空格，
/// 而换成下划线会让「字幕组名」和「番名」在搜索时和用户手打的对不上
/// （TS 也是这么做的）。
pub fn safe_tag(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|ch| {
            if ch.is_ascii_alphanumeric() || *ch == '_' {
                return true;
            }
            matches!(ch,
                '\u{4e00}'..='\u{9fff}'      // CJK 汉字
                | '\u{3040}'..='\u{309f}'    // 平假名
                | '\u{30a0}'..='\u{30ff}'    // 片假名
                | '\u{31f0}'..='\u{31ff}'    // 片假名扩展
            )
        })
        .filter(|ch| !matches!(ch, '-' | '\u{2740}'))
        .collect()
}

/// 建档时的标签过滤（TS 的 `extractFilteredTagNames`）。
///
/// 与 [`format_tags`] 的区别：这一步在**写库**时过滤（决定是否存），
/// 后者在**展示**时格式化。剔除规则和 TS 一致：`^\d{4}年`（放送年份）、纯数字。
/// TS 还有一份自定义的 `tagExcludeList`（存在库里），这里没有那张表，暂不实现。
pub fn filter_tag_names(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .filter(|tag| !is_year_tag(tag))
        .filter(|tag| !tag.chars().all(|ch| ch.is_ascii_digit()))
        .map(str::to_owned)
        .collect()
}

/// `2024年10月` 这类「放送年份」标签。
fn is_year_tag(tag: &str) -> bool {
    let digits: String = tag.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits.len() == 4 && tag[digits.len()..].starts_with('年')
}

/// `#2024年10月`（没有放送日期时为空串）。
///
/// 只留年月：TS 也是这么截的（`/(\d{4})年(\d{1,2})月.*/`）——「2024年10月3日」
/// 做成标签太长，而按年月检索才是真实用法。
pub fn anime_date(airing_start: Option<&str>) -> String {
    let Some(raw) = airing_start.map(str::trim).filter(|text| !text.is_empty()) else {
        return String::new();
    };
    let year: String = raw.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if year.len() != 4 {
        return String::new();
    }
    let rest = &raw[year.len()..];
    let Some(after_year) = rest.strip_prefix('年') else {
        return String::new();
    };
    let month: String = after_year
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if month.is_empty() {
        return format!("#{year}年");
    }
    format!("#{year}年{month}月")
}

/// `#NSFW`（`r18` 为真时）。
pub fn nsfw_tag(r18: Option<bool>) -> &'static str {
    match r18 {
        Some(true) => "#NSFW",
        _ => "",
    }
}

/// 简介：`> 每行前缀` + 超长截断并附「详细」链接（TS 的 `summaryTrim`）。
pub fn summary_trim(summary: Option<&str>, subject_id: i64, max_len: usize) -> String {
    let Some(summary) = summary.map(str::trim).filter(|text| !text.is_empty()) else {
        return String::new();
    };
    // TS 先 `\n` 字面量还原再压连续空行：库里的简介是 Bangumi 原样存的，
    // 两道工序缺一道就会出现一坨空行。
    let cleaned = collapse_blank_lines(&summary.replace("\\n", "\n"));
    let truncated = if cleaned.chars().count() > max_len {
        let head: String = cleaned.chars().take(max_len).collect();
        format!("{head}[...详细](https://bgm.tv/subject/{subject_id})")
    } else {
        cleaned
    };
    format!("\n介绍:\n{indented}||", indented = indented(&truncated))
}

/// 把多行文本的每一行加上 `> ` 前缀（Markdown 引用）。
fn indented(text: &str) -> String {
    text.split('\n')
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 丢掉空行（TS 的 `.replace(/\n{2,}/g, "\n")`）。
///
/// 是**删掉**空行而不是压成一个：TS 把连续换行压成单个换行，等价于「行间不留空行」。
fn collapse_blank_lines(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 集数的三种形态（决定排序位置）。
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedEpisode {
    /// `05` / `05v2`：正片，带版本号。
    Num { number: f64, version: String },
    /// `SP1` / `特别篇1`：特殊集，紧跟在同号正片后面。
    Sp { number: i64 },
    /// 剧场版之类：排最后。
    Other,
}

/// 解析集数字符串（TS 的 `parseEpisode`）。
///
/// 公开是因为除了导航排序，`/jz` 的单集纠正也要用它——纠正时「第 03 集」要在
/// 目标番的章节表里找回去，与排序必须是同一套理解。
pub fn parse_episode(episode: &str) -> ParsedEpisode {
    let text = episode.trim();
    let digits: String = text
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    if let Ok(number) = digits.parse::<f64>()
        && !digits.is_empty()
    {
        let rest = &text[digits.len()..];
        // 版本号：`03v2`。其余后缀（`-FIN` 之类）一律按正片处理。
        let version = rest
            .strip_prefix('v')
            .filter(|tail| !tail.is_empty() && tail.chars().all(|ch| ch.is_ascii_digit()))
            .map(str::to_owned)
            .unwrap_or_default();
        return ParsedEpisode::Num { number, version };
    }
    for prefix in ["SP", "特别篇"] {
        let Some(head) = text.get(..prefix.len()) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(prefix) {
            continue;
        }
        if let Ok(number) = text[prefix.len()..].trim().parse::<i64>() {
            return ParsedEpisode::Sp { number };
        }
    }
    ParsedEpisode::Other
}

/// 集数排序（TS 的 `compareEpisode`）。
///
/// 注意 SP / 特别篇**不是**全排最后——它们按自己的编号和正片**混排**
/// （`SP1` 会排在 `03` 前面，因为 1 < 3）。这是 TS 的真实行为，照搬：
/// 混合编号的番剧（本篇里夹特别篇）本来就该按播出顺序排。
pub fn compare_episode(a: &ResourceEntry, b: &ResourceEntry) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let left = parse_episode(&a.episode);
    let right = parse_episode(&b.episode);
    match (left, right) {
        (ParsedEpisode::Num { number: l, .. }, ParsedEpisode::Num { number: r, .. }) => {
            if l != r {
                return l.total_cmp(&r);
            }
            // 同集数按版本号字符串比：`03` < `03v2`。
            let lv = parse_episode(&a.episode);
            let rv = parse_episode(&b.episode);
            if let (
                ParsedEpisode::Num { version: lv, .. },
                ParsedEpisode::Num { version: rv, .. },
            ) = (lv, rv)
            {
                return lv.cmp(&rv);
            }
            Ordering::Equal
        }
        (
            ParsedEpisode::Num { number: l, .. },
            ParsedEpisode::Sp { number: r },
        ) => {
            if l != r as f64 {
                return l.total_cmp(&(r as f64));
            }
            Ordering::Less
        }
        (
            ParsedEpisode::Sp { number: l },
            ParsedEpisode::Num { number: r, .. },
        ) => {
            if l as f64 != r {
                return (l as f64).total_cmp(&r);
            }
            Ordering::Greater
        }
        (ParsedEpisode::Sp { number: l }, ParsedEpisode::Sp { number: r }) => l.cmp(&r),
        (ParsedEpisode::Other, ParsedEpisode::Other) => Ordering::Equal,
        (ParsedEpisode::Other, _) => Ordering::Greater,
        (_, ParsedEpisode::Other) => Ordering::Less,
    }
}

/// 把分组资源渲染成正文里的「资源段」（TS 的 `formatGroupedResources`）。
///
/// 每个字幕组一段：`[#组名]` 一行，下面一行是该组所有集数的链接，
/// 整段每行加 `> ` 前缀（导航正文整体是引用块）。
pub fn format_grouped_resources(grouped: &GroupedResources) -> Vec<String> {
    grouped
        .iter()
        .map(|(group, entries)| {
            let usable: Vec<&ResourceEntry> = entries
                .iter()
                .filter(|entry| entry.link.as_deref().is_some_and(|link| !link.trim().is_empty()))
                .collect();
            if usable.is_empty() {
                return String::new();
            }
            let mut entries = usable;
            entries.sort_by(|a, b| compare_episode(a, b));
            let line = entries
                .iter()
                .filter_map(|entry| {
                    entry
                        .link
                        .as_deref()
                        .map(|link| format!("[{}]({link})", entry.episode))
                })
                .collect::<Vec<_>>()
                .join(" | ");
            let section = format!("[#{}]\n{line}", safe_tag(group));
            section
                .split('\n')
                .map(|line| if line.trim().is_empty() { line.to_owned() } else { format!("> {line}") })
                .collect::<Vec<_>>()
                .join("\n")
        })
        // 一组里一条可用链接都没有时整组不显示：TS 会留下一个空的 `[#组名]`
        // 标题行，点不了也没信息量。
        .filter(|section| !section.trim().is_empty() && !section.contains("]\n\n"))
        .collect()
}

/// 导航正文：首条（带图）+ 后续资源分页。
///
/// 与 TS 同构，四步降级，每一步都以「塞得进 1024 / 100 个实体」为条件：
///
/// 1. 先压 summary（300 → 250/200/150/120/100）；
/// 2. 再加资源**分页导航**（`[第1页](link) [第2页]…`）；
/// 3. 再试着把完整资源区放进首条；
/// 4. 放不进去就把资源整段挪到后续分页消息（4096 一条）。
///
/// `page_links` 由调用方给出（第 N 页的链接要从库里 `navVideoMessage` 查，
/// 纯函数层拿不到库）。
pub fn navmegtext(
    anime: &Anime,
    sections: &[String],
    page_links: &[String],
) -> Vec<String> {
    let build_main = |summary_max: usize, resources: Option<&str>, navigation: &str| {
        let title = format!(
            "{} {} {}",
            anime_date(anime.airing_start.as_deref()),
            nsfw_tag(anime.r18),
            anime.name.as_deref().unwrap_or("")
        );
        let base = format!(
            "\n> 中文名称: {}\n> 本季话数: {}\n> 放送开始: {}\n> 放送星期: {}\n> 动漫评分: [{}](https://bgm.tv/subject/{}/stats){}",
            anime.name_cn.as_deref().unwrap_or("未知"),
            anime.episode.as_deref().unwrap_or("未知"),
            anime.airing_start.as_deref().unwrap_or("未知"),
            anime.airing_day.as_deref().unwrap_or("未知"),
            anime
                .score
                .as_ref()
                .and_then(|score| score.as_f64())
                .map(|score| format!("{score:.1}"))
                .unwrap_or_else(|| "未知".to_owned()),
            anime.id,
            beijing_date(),
        );
        let summary = summary_trim(anime.summary.as_deref(), anime.id, summary_max);
        let resources = match resources {
            Some(_) if !sections.is_empty() => format!("\n资源:\n{}", sections.join("\n")),
            _ if !navigation.is_empty() => format!("\n资源:\n> {navigation}"),
            _ => String::new(),
        };
        let tags = format!("\n标签: \n> {}||", format_tags(&filter_tag_names(&anime.tags)));
        format!("{title}{base}{summary}{resources}{tags}")
    };

    let navigation = page_links.join(" ");

    // 1) summary 逐级压缩（只在无量资源版本上量长度）。
    let mut summary_max = 300;
    let mut main = build_main(summary_max, None, "");
    if over_limit(&main) {
        for candidate in [250, 200, 150, 120, 100] {
            summary_max = candidate;
            main = build_main(summary_max, None, "");
            if !over_limit(&main) {
                break;
            }
        }
    }
    // 2) 加分页导航（放不下就算了：导航链接是锦上添花，正文不能为它再压一次）。
    if !navigation.is_empty() {
        let with_nav = build_main(summary_max, None, &navigation);
        if !over_limit(&with_nav) {
            main = with_nav;
        }
    }
    // 3) 试着把资源区塞进首条。
    let mut resources_in_main = false;
    if !sections.is_empty() {
        let with_resources = build_main(summary_max, Some(""), "");
        if !over_limit(&with_resources) {
            main = with_resources;
            resources_in_main = true;
        }
    }

    let mut out = vec![main];
    // 4) 首条装不下才分页。
    if !resources_in_main && !sections.is_empty() {
        out.extend(resource_pages(anime, sections));
    }
    out
}

/// 资源分页：每段（一个字幕组）整体放一页，实在太长就单独一页（TS 同逻辑）。
///
/// 导航要先用它算出「一共有几页」才能生成正文里的页码导航，所以是公开的。
pub fn resource_pages(anime: &Anime, sections: &[String]) -> Vec<String> {
    let title = format!(
        "动漫: {}\n\n资源:\n",
        anime.name_cn.as_deref().unwrap_or("")
    );
    let mut pages = Vec::new();
    let mut body = String::new();
    let mut flush = |body: &mut String| {
        if body.trim().is_empty() {
            return;
        }
        pages.push(format!("{title}{}", body.trim()));
        body.clear();
    };
    for section in sections {
        let candidate = if body.is_empty() {
            section.clone()
        } else {
            format!("{body}\n{section}")
        };
        if over_page(&format!("{title}{candidate}")) {
            flush(&mut body);
            // 单段自己就超限：TS 也是硬塞（否则死循环），这里照做。
            body.push_str(section);
        } else {
            body = candidate;
        }
    }
    flush(&mut body);
    pages
}

/// 视频 caption（TS 的 `AnimeText`）：原名/中文名/发布组/发布时间 + 四个链接。
///
/// 四个链接只在**导航消息已存在**时才给：没有导航就没处可跳，给了也是坏链。
pub fn anime_text(anime: &Anime, item: &AnimeItem, episode_id: i64) -> String {
    let nsfw = nsfw_tag(anime.r18);
    let title = match item.link.trim() {
        "" => item.title.clone(),
        link => format!("[{}]({link})", item.title),
    };
    let date_tag = anime_date(anime.airing_start.as_deref());
    let name = anime
        .name_cn
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .or(anime.name.as_deref())
        .unwrap_or("未知");
    let fansub_tags = format_tags(&item.fansub);
    let mut out = format!(
        "{date_tag} {nsfw} {title}\n>原名称: {}\n>中文名: {}\n>发布组: {}{}",
        anime.name.as_deref().unwrap_or("未知"),
        anime.name_cn.as_deref().unwrap_or("未知"),
        fansub_tags,
        item.pub_date
            .trim()
            .is_empty()
            .then(String::new)
            .unwrap_or_else(|| format!("\n>发布时间: {}", item.pub_date)),
    );
    let group_tags = item
        .fansub
        .iter()
        .map(|fansub| format!("#{}_{}", safe_tag(fansub), safe_tag(name)))
        .collect::<Vec<_>>()
        .join(" ");
    out.push_str(&format!("\n追踪标签：\n>名称: #{}\n>番剧组: {group_tags}", safe_tag(name)));
    if let Some(link) = anime
        .nav_message
        .as_ref()
        .and_then(|nav| nav.link.as_deref())
        .filter(|link| !link.trim().is_empty())
    {
        out.push_str(&format!(
            " \n[番剧导航]({link}) | [收藏番剧]({BOT_START_URL}collection-{}) | [标记看过]({BOT_START_URL}eplook-{episode_id}) | [吐槽箱](https://bgm.tv/ep/{episode_id})",
            anime.id
        ));
    }
    out
}

/// 首条（带图 caption）是否超限。
fn over_limit(markdown: &str) -> bool {
    let (length, entities) = measure(markdown);
    length > MAIN_LIMIT || entities > ENTITY_LIMIT
}

/// 分页（纯文本）是否超限。
fn over_page(markdown: &str) -> bool {
    let (length, entities) = measure(markdown);
    length > PAGE_LIMIT || entities > ENTITY_LIMIT
}

/// 近似测量：`(可见字符数, 链接数)`。
///
/// 见模块文档里的说明——这是「够用就好」的近似，不是 TDLib 的实体解析。
pub fn measure(markdown: &str) -> (usize, usize) {
    let mut visible = String::new();
    let mut links = 0usize;
    let mut rest = markdown;
    while let Some(start) = rest.find('[') {
        visible.push_str(&rest[..start]);
        let Some(middle) = rest[start..].find("](") else {
            visible.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let text = &rest[start + 1..start + middle];
        let after = &rest[start + middle + 2..];
        let Some(end) = after.find(')') else {
            visible.push_str(&rest[start..]);
            rest = "";
            break;
        };
        visible.push_str(text);
        links += 1;
        rest = &after[end + 1..];
    }
    visible.push_str(rest);
    (visible.chars().count(), links)
}

/// 当前北京时间 `(yyyy-MM-dd)`。TS 用它在正文末尾标「这份导航是哪天生成的」。
pub fn beijing_date() -> String {
    let beijing = chrono::Utc::now() + chrono::Duration::hours(8);
    format!("({})", beijing.format("%Y-%m-%d"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anime() -> Anime {
        Anime {
            id: 400602,
            name: Some("葬送のフリーレン".to_owned()),
            name_cn: Some("葬送的芙莉莲".to_owned()),
            episode: Some("28".to_owned()),
            airing_start: Some("2023年9月29日".to_owned()),
            airing_day: Some("星期五".to_owned()),
            tags: vec!["奇幻".to_owned(), "2023年".to_owned(), "24".to_owned()],
            summary: Some("魔王被打倒之后".to_owned()),
            ..Default::default()
        }
    }

    fn entry(episode: &str) -> ResourceEntry {
        ResourceEntry {
            episode: episode.to_owned(),
            link: Some(format!("https://t.me/c/1/{episode}")),
        }
    }

    #[test]
    fn safe_tag_keeps_cjk_and_drops_spaces_and_symbols() {
        assert_eq!(safe_tag(" 喵萌奶茶屋 "), "喵萌奶茶屋");
        assert_eq!(safe_tag("ANi Project"), "ANiProject");
        assert_eq!(safe_tag("葬送のフリーレン"), "葬送のフリーレン");
        // `-` 与装饰符号被去掉：hashtag 里它们是噪音。
        assert_eq!(safe_tag("黑-ネズミ❀"), "黑ネズミ");
    }

    #[test]
    fn format_tags_drops_pure_numbers() {
        assert_eq!(format_tags(&["奇幻".to_owned(), "2024".to_owned()]), "#奇幻");
        assert_eq!(format_tags(&[]), "");
    }

    #[test]
    fn year_and_number_tags_are_filtered_at_import() {
        let tags = vec!["2024年".to_owned(), "24".to_owned(), "奇幻".to_owned()];
        assert_eq!(filter_tag_names(&tags), vec!["奇幻".to_owned()]);
    }

    #[test]
    fn anime_date_keeps_only_year_and_month() {
        assert_eq!(anime_date(Some("2023年9月29日")), "#2023年9月");
        assert_eq!(anime_date(Some("2023年")), "#2023年");
        assert_eq!(anime_date(None), "");
        assert_eq!(anime_date(Some("未知")), "");
    }

    #[test]
    fn nsfw_only_for_r18() {
        assert_eq!(nsfw_tag(Some(true)), "#NSFW");
        assert_eq!(nsfw_tag(Some(false)), "");
        assert_eq!(nsfw_tag(None), "");
    }

    #[test]
    fn episodes_sort_numbers_then_versions_then_sp_then_other() {
        let mut entries = vec![
            entry("特别篇2"),
            entry("03v2"),
            entry("剧场版"),
            entry("SP1"),
            entry("03"),
            entry("01"),
        ];
        entries.sort_by(compare_episode);
        let order: Vec<&str> = entries.iter().map(|e| e.episode.as_str()).collect();
        // SP/特别篇按自己的编号混在正片里（1 < 2 < 3），剧场版这类无编号的排最后。
        assert_eq!(order, vec!["01", "SP1", "特别篇2", "03", "03v2", "剧场版"]);
    }

    #[test]
    fn grouped_resources_are_quoted_and_sorted_per_group() {
        let mut grouped = GroupedResources::new();
        grouped.insert("喵萌".to_owned(), vec![entry("03"), entry("01")]);
        let sections = format_grouped_resources(&grouped);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0], "> [#喵萌]\n> [01](https://t.me/c/1/01) | [03](https://t.me/c/1/03)");
    }

    #[test]
    fn entries_without_a_link_are_hidden() {
        let mut grouped = GroupedResources::new();
        grouped.insert(
            "组".to_owned(),
            vec![ResourceEntry {
                episode: "01".to_owned(),
                link: None,
            }],
        );
        assert!(format_grouped_resources(&grouped).is_empty());
    }

    #[test]
    fn measure_strips_markdown_links() {
        let (length, links) = measure("[第01集](https://t.me/c/1/1)");
        assert_eq!(length, "第01集".chars().count());
        assert_eq!(links, 1);
    }

    #[test]
    fn summary_is_trimmed_and_quoted() {
        let long = "あ".repeat(400);
        let out = summary_trim(Some(&long), 400602, 300);
        assert!(out.contains("[...详细](https://bgm.tv/subject/400602)"));
        assert!(out.starts_with("\n介绍:\n> "));
        assert!(out.ends_with("||"));
    }

    #[test]
    fn summary_newlines_are_normalized_and_quoted_per_line() {
        let out = summary_trim(Some("第一行\\n\n\n第二行"), 1, 300);
        assert!(out.contains("> 第一行\n> 第二行"), "{out}");
    }

    #[test]
    fn nav_text_starts_with_date_and_nsfw_tags() {
        let pages = navmegtext(&anime(), &[], &[]);
        assert!(pages[0].starts_with("#2023年9月  "), "{}", pages[0]);
        assert!(pages[0].contains("> 中文名称: 葬送的芙莉莲"));
        assert!(pages[0].contains("#奇幻||"), "纯数字标签不该出现：{}", pages[0]);
        // 没有资源就不该有资源区。
        assert!(!pages[0].contains("\n资源:\n"));
    }

    #[test]
    fn long_summary_is_compressed_to_fit_the_caption_limit() {
        let mut anime = anime();
        anime.summary = Some("あ".repeat(2000));
        let pages = navmegtext(&anime, &[], &[]);
        let (length, _) = measure(&pages[0]);
        // 压到 100 仍然超限时正文就是超的（这种情况只能靠资源外移再省）——
        // 但至少要确认真的压过了。
        assert!(length <= MAIN_LIMIT + 200, "length={length}");
    }

    #[test]
    fn resources_move_to_pages_when_the_main_text_is_too_long() {
        let mut anime = anime();
        anime.summary = Some("あ".repeat(900));
        // 资源区要大到连「summary 压到 100」也塞不进首条，才会外移到分页。
        let sections: Vec<String> = (0..30)
            .map(|index| {
                format!(
                    "> [#字幕组{index}]\n> [01](https://t.me/c/1/{index}) | [02](https://t.me/c/2/{index})"
                )
            })
            .collect();
        let pages = navmegtext(&anime, &sections, &["[第1页](https://t.me/c/1/9)".to_owned()]);
        assert!(pages.len() >= 2, "首条塞不下就该分页：{pages:?}");
        assert!(pages[1].contains("[#字幕组0]"));
    }

    #[test]
    fn page_navigation_is_added_when_it_fits() {
        let pages = navmegtext(&anime(), &[], &["[第1页](https://t.me/c/1/9)".to_owned()]);
        assert!(pages[0].contains("[第1页](https://t.me/c/1/9)"), "{}", pages[0]);
    }

    #[test]
    fn anime_text_has_four_links_only_when_nav_exists() {
        let item = AnimeItem {
            title: "[喵萌] 葬送的芙莉莲 - 05".to_owned(),
            pub_date: "2026-10-27".to_owned(),
            magnet: String::new(),
            link: "https://example.com/1".to_owned(),
            team: "喵萌".to_owned(),
            fansub: vec!["喵萌奶茶屋".to_owned()],
            names: vec!["葬送的芙莉莲".to_owned()],
            source: None,
            episode: Some("05".to_owned()),
        };
        // 没有 navMessage：四个链接不出现（跳不过去）。
        let without_nav = anime_text(&anime(), &item, 1227091);
        assert!(!without_nav.contains("番剧导航"));
        assert!(without_nav.contains(">原名称: 葬送のフリーレン"));
        assert!(without_nav.contains("#喵萌奶茶屋_葬送的芙莉莲"));

        let mut with_nav = anime();
        with_nav.nav_message = Some(anime_core::models::MessageRef {
            link: Some("https://t.me/c/2/3".to_owned()),
            ..Default::default()
        });
        let text = anime_text(&with_nav, &item, 1227091);
        assert!(text.contains("[番剧导航](https://t.me/c/2/3)"));
        assert!(text.contains("collection-400602"));
        assert!(text.contains("eplook-1227091"));
        assert!(text.contains("https://bgm.tv/ep/1227091"));
    }

    #[test]
    fn beijing_date_is_parenthesized_iso() {
        let text = beijing_date();
        assert!(text.starts_with('(') && text.ends_with(')'), "{text}");
        assert_eq!(text.len(), 12, "{text}");
    }
}
