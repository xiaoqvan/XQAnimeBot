//! RSS 三源联调探针：真网络跑一遍「抓取 → 过滤 → 合并 → 构建条目」。
//!
//! ```bash
//! cargo run -p anime-core --example rss_probe                 # 抓三源 + 合并 + 构建
//! cargo run -p anime-core --example rss_probe -- --top 20     # 多看几条
//! cargo run -p anime-core --example rss_probe -- --build 0    # 只看抓取/合并
//! cargo run -p anime-core --example rss_probe -- --bangumi 2  # 额外打 bangumi.moe API 补磁力
//! cargo run -p anime-core --example rss_probe -- --json       # 输出合并结果的 JSON
//! ```
//!
//! 这个 example 不进正式流程：它存在的意义是「改了 RSS 层，能不能一次命令看到真数据」。

use std::time::Duration;

use anime_core::logger;
use anime_core::models::RssItem;
use anime_core::rss::{self, Http};

fn main() {
    let mut top = 10usize;
    let mut build_limit = 10usize;
    let mut bangumi_api = 0usize;
    let mut json = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--top" => top = args.next().and_then(|v| v.parse().ok()).unwrap_or(top),
            "--build" => build_limit = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--bangumi" => bangumi_api = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--json" => json = true,
            other => {
                eprintln!("未知参数：{other}");
                std::process::exit(2);
            }
        }
    }

    let http = Http::new(Duration::from_secs(30));
    let logger = logger::stderr();

    println!("== 抓取（每源上限 50 条，dmhy/acgnx 截断发生在过滤前）==");
    let report = rss::fetch_all(&http, 50, &[], &logger);
    for source in &report.sources {
        match &source.error {
            Some(error) => println!("{:<12} 失败：{error}", source.kind.as_str()),
            None => println!("{:<12} {} 条", source.kind.as_str(), source.count),
        }
    }
    println!("合并去重后 {} 条\n", report.items.len());

    if json {
        println!("{}", serde_json::to_string_pretty(&report.items).unwrap());
        return;
    }

    if build_limit > 0 {
        println!("== 前 {top} 条（按发布时间倒序）+ 标题解析 ==");
        for (index, item) in report.items.iter().take(top).enumerate() {
            let built = rss::build_item(item, None);
            let detail = match &built {
                Some(built) => format!(
                    "组={} 集={} 源={} 番名={}",
                    built.team,
                    built.episode.as_deref().unwrap_or("-"),
                    built.source.as_deref().unwrap_or("-"),
                    built.names.join(" / ")
                ),
                None => "（解析不出番名，下游会跳过）".to_owned(),
            };
            println!(
                "{:>2}. [{}] {} | {}\n    {}",
                index + 1,
                item.kind.as_str(),
                item.pub_date,
                item.title,
                detail
            );
        }

        let usable = report
            .items
            .iter()
            .filter(|item| rss::build_item(item, None).is_some())
            .count();
        println!(
            "\n可构建条目：{usable} / {}（其余是规则表里没收录的组，或标题结构不认识）",
            report.items.len()
        );
    }

    if bangumi_api == 0 {
        return;
    }
    println!("\n== bangumi.moe API 补全（前 {bangumi_api} 条 bangumi 源条目）==");
    let mut done = 0;
    for item in report.items.iter().filter(|item| is_bangumi(item)) {
        if done >= bangumi_api {
            break;
        }
        done += 1;
        match rss::enrich_bangumi(&http, item, &logger) {
            Some(built) => println!(
                "  ✅ 组={} 集={} 磁力={}… 番名={}",
                built.team,
                built.episode.as_deref().unwrap_or("-"),
                built.magnet.chars().take(48).collect::<String>(),
                built.names.join(" / ")
            ),
            None => println!("  ⚠️ 这条补全后仍然不能入库：{}", item.title),
        }
    }
    if done == 0 {
        println!("  这轮没有 bangumi 源条目（站内 RSS 条数少，属正常）");
    }
}

fn is_bangumi(item: &RssItem) -> bool {
    item.kind == anime_core::models::RssKind::Bangumi
}
