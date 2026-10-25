//! 导出一份 `bangumi-card.html` 的渲染数据，方便单独调模板/出图，不用起机器人。
//!
//! ```bash
//! cargo run -p anime-core --example bangumi_payload
//! # 输出：crates/anime-core/out/bangumi_payload.json
//! ```
//!
//! 这份 JSON 直接就是出图服务 `render` 方法的 `data` 字段内容。

use anime_core::card::bangumi::{BangumiCard, Collection, Episode};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let card = demo_card();
    let payload = card.to_html_payload();
    let out_dir = std::path::PathBuf::from("crates/anime-core/out");
    std::fs::create_dir_all(&out_dir)?;
    let out = out_dir.join("bangumi_payload.json");
    std::fs::write(&out, serde_json::to_string_pretty(&payload)?)?;
    println!(
        "已导出 {}（cache_key={}）",
        out.display(),
        &card.cache_key()[..12]
    );
    Ok(())
}

/// 一份内容齐全的示例卡（字段名与模板一一对应）。
fn demo_card() -> BangumiCard {
    BangumiCard {
        id: 400602,
        name: "葬送のフリーレン".to_owned(),
        name_cn: "葬送的芙莉莲".to_owned(),
        air_date: "2023-09-29".to_owned(),
        platform: "TV".to_owned(),
        score: 8.9,
        rating_total: 20_000,
        rating_counts: [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100],
        tags: vec!["奇幻".to_owned(), "冒险".to_owned(), "治愈".to_owned()],
        staff: vec![
            ("导演".to_owned(), "斋藤圭一郎".to_owned()),
            ("动画制作".to_owned(), "MADHOUSE".to_owned()),
        ],
        collection: Collection {
            wish: 1200,
            collect: 3400,
            doing: 800,
            on_hold: 100,
        },
        episodes: (1..=4)
            .map(|sort| Episode {
                sort,
                name: format!("第{sort}话"),
                airdate: "2023-10-01".to_owned(),
            })
            .collect(),
        other_episodes: vec![("SP".to_owned(), vec![1, 2])],
        aired_count: 4,
        current_ep: 4,
        current_ep_name: "第4话".to_owned(),
        current_ep_airdate: "2023-10-22".to_owned(),
        next_ep: 5,
        next_ep_name: "第5话".to_owned(),
        next_ep_airdate: "2023-10-29".to_owned(),
        fansubs: vec!["LoliHouse".to_owned(), "喵萌奶茶屋".to_owned()],
        generated_at: "2026-10-25 20:30".to_owned(),
        rating_emoji_idx: 1,
        ..Default::default()
    }
}
