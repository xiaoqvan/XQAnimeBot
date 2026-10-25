//! 端到端跑一遍 ultralightGenImage：拉起 `image-shot` → 渲染 → 落盘。
//!
//! ```bash
//! cargo run -p ultralight-gen-image --example render_card
//! cargo run -p ultralight-gen-image --example render_card -- path/to/template_dir
//! ```
//!
//! 不给参数就用 crate 自带的 `templates/demo.html`（通用报价卡，和业务无关）。

use std::path::PathBuf;

use ultralight_gen_image::{RenderRequest, ShotClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from("crates/ultralight-gen-image/out");
    std::fs::create_dir_all(&out_dir)?;
    let out = out_dir.join("demo.jpg");

    let binary = ShotClient::find_binary().ok_or(
        "找不到 image-shot.exe：先 `cargo build -p ultralight-gen-image`，或设 IMAGE_SHOT_BIN",
    )?;
    println!("渲染器：{}", binary.display());

    let mut client = ShotClient::spawn(&binary)?;
    println!("ping：{}", client.ping()?);

    // 自带 demo 模板；也可以给一个目录用别人的模板（业务插件就是这么用的）。
    let template_dir = match std::env::args().nth(1) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates"),
    };

    let request = RenderRequest::template(
        "demo",
        serde_json::json!({
            "badge": "EXAMPLE",
            "title": "HTML 出图，一个插件搞定",
            "subtitle": "ultralightGenImage 只认 HTML / 模板 + JSON，不认识动漫、订单或报表",
            "rows": [
                { "label": "渲染后端", "value": "Ultralight 2.0" },
                { "label": "布局", "value": "HTML + Vue（Flexbox 原生生效）" },
                { "label": "输出", "value": "JPEG / PNG，2× 超采样" }
            ],
            "metrics": [
                { "label": "视口", "value": "1000 × 550" },
                { "label": "像素比", "value": "2.0" },
                { "label": "格式", "value": "JPEG q90" }
            ],
            "footer": "由 ultralightGenImage 渲染"
        }),
        &out,
    )
    .with_template_dir(&template_dir);

    let rendered = client.render(&request)?;
    println!(
        "已生成：{}（{} KB，{}×{}，耗时 {} ms）",
        rendered.path.display(),
        rendered.bytes / 1024,
        rendered.width,
        rendered.height,
        rendered.ms
    );
    Ok(())
}
