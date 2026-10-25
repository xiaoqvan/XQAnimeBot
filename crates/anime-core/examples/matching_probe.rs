//! 匹配链路的端到端探针：RSS 条目 → 番剧匹配 → 集数对齐。
//!
//! ```bash
//! cargo run -p anime-core --example matching_probe              # 内置 fixture 标题（不联网）
//! cargo run -p anime-core --example matching_probe -- --live     # 抓真实 RSS 再匹配
//! ```
//!
//! 「库」是内置的三部番，故意混进几种容易出错的形态：季名后缀、两字短名、
//! 全角英文名。看输出就知道每种情况落到哪个分支。

use std::time::Duration;

use anime_core::logger;
use anime_core::matching::{
    AnimeCandidate, EpisodeFit, EpisodeMatch, EpisodeSource, ItemMatch, SeasonDecision,
    decide_season, match_item,
};
use anime_core::models::{AnimeItem, EpisodeMeta};
use anime_core::rss::{self, Http};

/// 内置「库」：id 就是 Bangumi subject id。
fn library() -> Vec<AnimeCandidate> {
    vec![
        AnimeCandidate {
            id: 400602,
            name: Some("葬送のフリーレン".to_owned()),
            name_cn: Some("葬送的芙莉莲".to_owned()),
            names: vec!["Frieren".to_owned()],
            episode: Some("28".to_owned()),
            airing_start: Some("2023年9月29日".to_owned()),
            ..Default::default()
        },
        AnimeCandidate {
            id: 425998,
            name: Some("BLEACH 千年血戦篇-禍進譚-".to_owned()),
            name_cn: Some("死神 千年血战篇-祸进谭-".to_owned()),
            names: vec!["Bleach: Thousand-Year Blood War".to_owned()],
            episode: Some("12".to_owned()),
            airing_start: Some("2026年7月5日".to_owned()),
            ..Default::default()
        },
        AnimeCandidate {
            id: 396293,
            name: Some("SPY×FAMILY Season 2".to_owned()),
            name_cn: Some("间谍过家家 第二季".to_owned()),
            names: vec!["SPY×FAMILY".to_owned()],
            episode: Some("12".to_owned()),
            airing_start: Some("2026年10月7日".to_owned()),
            ..Default::default()
        },
    ]
}

/// 给 400602 造 28 集章节：2023-09-29 起每周一集。
fn episodes_for(id: i64) -> Vec<EpisodeMeta> {
    if id != 400602 {
        return Vec::new();
    }
    let start = chrono::NaiveDate::from_ymd_opt(2023, 9, 29).expect("固定日期必须合法");
    (1..=28)
        .map(|index| {
            let airdate = start + chrono::TimeDelta::days(i64::from(index - 1) * 7);
            EpisodeMeta {
                id: 9_000_000 + i64::from(index),
                sort: f64::from(index),
                ep: f64::from(index),
                r#type: Some(0),
                airdate: Some(airdate.format("%Y-%m-%d").to_string()),
                subject_id: id,
                ..Default::default()
            }
        })
        .collect()
}

/// 离线 fixture：直接造已经解析好的条目，绕过抓取。
fn fixture_items() -> Vec<AnimeItem> {
    let cases = [
        (
            "[喵萌奶茶屋] 葬送的芙莉莲 [05][1080P][简日双语]",
            "葬送的芙莉莲",
            "05",
        ),
        (
            "[黒ネズミたち] 葬送的芙莉莲 - 28 [1080P]",
            "葬送的芙莉莲",
            "28",
        ),
        ("[ANi] 葬送的芙莉莲 - 29 [1080P]", "葬送的芙莉莲", "29"),
        (
            "[ANi] 死神 千年血战篇 - 05 [1080P]",
            "死神 千年血战篇",
            "05",
        ),
        ("[ANi] 死神 - 05 [1080P]", "死神", "05"),
        (
            "[LoliHouse] SPY×FAMILY - 03 [WebRip 1080p]",
            "SPY×FAMILY",
            "03",
        ),
        ("[ANi] 完全没听过的番 - 01 [1080P]", "完全没听过的番", "01"),
        ("[ANi] 葬送的芙莉莲 - 05v2 [1080P]", "葬送的芙莉莲", "05v2"),
    ];
    cases
        .into_iter()
        .map(|(title_text, name, episode)| AnimeItem {
            title: title_text.to_owned(),
            pub_date: String::new(),
            magnet: "magnet:?xt=urn:btih:00112233445566778899aabbccddeeff00112233".to_owned(),
            link: String::new(),
            team: "fixture".to_owned(),
            fansub: vec!["fixture".to_owned()],
            names: vec![name.to_owned()],
            source: None,
            episode: Some(episode.to_owned()),
        })
        .collect()
}

/// 真网络：抓一轮 RSS，用标题解析出 names/episode 再匹配。
fn live_items(limit: usize) -> (Vec<AnimeItem>, Vec<(String, String)>) {
    let http = Http::new(Duration::from_secs(30));
    let report = rss::fetch_all(&http, 60, &[], &logger::stderr());
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    for entry in &report.items {
        let Some(built) = rss::build_item(entry, None) else {
            skipped.push((
                entry.title.clone(),
                "标题解析不出番名（规则表未收录）".to_owned(),
            ));
            continue;
        };
        if items.len() >= limit {
            break;
        }
        items.push(built);
    }
    (items, skipped)
}

fn main() {
    let live = std::env::args().any(|arg| arg == "--live");
    let (items, skipped) = if live {
        println!("== 抓真实 RSS（最多 12 条能解析的）==");
        live_items(12)
    } else {
        println!("== 内置 fixture（加 --live 换成真实 RSS）==");
        (fixture_items(), Vec::new())
    };

    let library = library();
    let today = chrono::Local::now().date_naive();

    println!("\n库内番剧：");
    for candidate in &library {
        println!(
            "  {:>6}  {} / {}  （{} 集，{} 起）",
            candidate.id,
            candidate.name_cn.as_deref().unwrap_or("-"),
            candidate.name.as_deref().unwrap_or("-"),
            candidate.episode.as_deref().unwrap_or("-"),
            candidate.airing_start.as_deref().unwrap_or("-")
        );
    }

    println!("\n== 匹配结果 ==");
    let mut ready = 0;
    for item in &items {
        // 两段式，和插件壳里的真实顺序一致：先用 season 结论定位番剧，
        // 再按 id 去章节表取集数（这里章节是内置的）。
        let season = decide_season(&item.names, &library);
        let episodes = season.anime_id().map(episodes_for).unwrap_or_default();
        let outcome = match_item(item, &library, &episodes, today, None);
        ready += usize::from(outcome.is_ready());
        print_outcome(item, &outcome);
    }

    if !skipped.is_empty() {
        println!("\n== 抓取后跳过（{} 条）==", skipped.len());
        for (title_text, reason) in skipped.iter().take(8) {
            println!("  - {title_text}\n      {reason}");
        }
    }

    println!(
        "\n可自动投放 {ready} / {} 条；其余需要复核或按新番处理。",
        items.len()
    );
}

fn print_outcome(item: &AnimeItem, outcome: &ItemMatch) {
    let status = if outcome.is_ready() {
        "可投"
    } else {
        "复核"
    };
    println!("\n[{status}] {}", item.title);
    println!("    番剧：{}", outcome.season.describe());
    match &outcome.episode {
        Some(EpisodeMatch::Matched {
            episode_id,
            sort,
            source,
            ..
        }) => println!(
            "    集数：章节 {episode_id}，sort={}（{}）",
            anime_core::matching::episode::format_number(*sort),
            match source {
                EpisodeSource::Ai => "AI 判定",
                EpisodeSource::Auto => "自动对齐",
            }
        ),
        Some(other) => println!("    集数：{}", other.describe()),
        None => println!("    集数：（没确定番剧，跳过集数对齐）"),
    }
    if let EpisodeFit::Beyond { episode, total } = outcome.episode_fit {
        println!(
            "    提示：第 {} 集超出总集数 {}，可能是新一季",
            anime_core::matching::episode::format_number(episode),
            anime_core::matching::episode::format_number(total)
        );
    }
    if let SeasonDecision::NeedsReview {
        also: Some(runner), ..
    } = &outcome.season
    {
        println!(
            "    竞争候选：『{}』{} 分",
            runner.matched_name, runner.score
        );
    }
}
