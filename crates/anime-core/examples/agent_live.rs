//! LLM Agent 真机验证：拿一条真实 BT 标题走一遍 Bangumi 匹配。
//!
//! **不把凭据写进代码**——全部走环境变量：
//!
//! ```sh
//! export XQ_AI_BASE_URL="https://openrouter.ai/api/v1"
//! export XQ_AI_KEY="sk-or-v1-..."
//! export XQ_AI_MODEL="nvidia/nemotron-3.5-super-vl:free"
//! cargo run -p anime-core --example agent_live -- "[喵萌奶茶屋] 葬送的芙莉莲 - 05"
//! ```
//!
//! 为什么单独做一个 example 而不是走插件：`/aimatch` 要起宿主、装 TDLib、连数据库；
//! 而 Agent 这条链路（Bangumi 搜索 + LLM 迭代）本身不需要它们。这里能独立验证
//! 「工具调用循环到底转了几轮、置信度多少、理由是什么」。
//!
//! 输出里会打印每次工具调用，那是判断「模型有没有瞎转」的唯一依据。

use anime_core::ai::{BangumiAgent, MatchInput};
use anime_core::bangumi::{BgmClient, BgmTools};
use anime_core::config::AiConfig;
use anime_core::title;

fn main() {
    let mut args = std::env::args().skip(1);
    let title = args
        .next()
        .unwrap_or_else(|| "[喵萌奶茶屋] 葬送的芙莉莲 - 05 [1080p]".to_owned());

    let config = AiConfig {
        enabled: true,
        api_key: std::env::var("XQ_AI_KEY").unwrap_or_default(),
        base_url: std::env::var("XQ_AI_BASE_URL")
            .unwrap_or_else(|_| "https://api.deepseek.com".to_owned()),
        model: std::env::var("XQ_AI_MODEL").unwrap_or_else(|_| "deepseek-flash".to_owned()),
    };
    if !config.usable() {
        eprintln!("缺少凭据：请先 export XQ_AI_KEY=...（可选 XQ_AI_BASE_URL / XQ_AI_MODEL）");
        std::process::exit(2);
    }
    println!("模型：{} @ {}", config.model, config.base_url);

    // 番名候选：按字幕组规则解析；没有组标签就退回整条标题。
    let team = anime_core::rss::fansub_of(&title).into_iter().next();
    let (names, episode) = match title::parse_info(&title, team.as_deref()) {
        Some(info) if info.is_usable() => {
            let episode = (info.episode != "未知").then_some(info.episode);
            (info.names, episode)
        }
        _ => (vec![title.clone()], None),
    };
    println!("标题：{title}");
    println!("番名：{}", names.join("、"));
    println!("集数：{}", episode.as_deref().unwrap_or("(未解析)"));

    let tools = BgmTools::new(BgmClient::new(Default::default()));
    let agent = BangumiAgent::new(config, tools);
    let started = std::time::Instant::now();
    let result = agent.match_subject(
        &MatchInput {
            title: title.clone(),
            names,
            episode,
        },
        1,
    );

    println!("\n── 结果（耗时 {:.1}s）──", started.elapsed().as_secs_f64());
    println!(
        "条目 id ：{}",
        result
            .subject_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "(无)".to_owned())
    );
    println!(
        "章节 id ：{}",
        result
            .episode_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "(无)".to_owned())
    );
    println!(
        "集数     ：{}",
        result
            .episode_sort
            .map(|sort| sort.to_string())
            .unwrap_or_else(|| "-".to_owned())
    );
    println!("置信度   ：{:.2}", result.confidence);
    println!("够自信先发后审：{}", result.is_confident());
    println!("理由     ：{}", result.reason);
}
