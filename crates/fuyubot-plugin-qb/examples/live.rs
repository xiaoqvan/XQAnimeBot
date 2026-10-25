//! 真机联调：连本机 qBittorrent WebUI 跑一遍读接口。
//!
//! ```bash
//! cargo run -p fuyubot-plugin-qb --example live
//! # 自定义：
//! QB_URL=http://127.0.0.1:8080 QB_USER=admin QB_PASS=xxx cargo run -p fuyubot-plugin-qb --example live
//! # 连写接口一起测（会临时添加一个暂停的磁力，随后删除）：
//! QB_WRITE_TEST=1 cargo run -p fuyubot-plugin-qb --example live
//! ```
//!
//! 默认只读：版本、传输状态、种子列表、文件列表、属性。
//! 写接口（add/pause/resume/delete）只在 `QB_WRITE_TEST=1` 时才碰，且用的是一条公有种子，
//! 添加后立刻删掉，不碰你现有的任务。

use fuyubot_plugin_qb::api::{call, configure};
use serde_json::{Value, json};

const MAGNET: &str = "magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel&tr=udp%3A%2F%2Fexplodie.org%3A6969";

fn main() {
    let url = std::env::var("QB_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_owned());
    let user = std::env::var("QB_USER").unwrap_or_else(|_| "admin".to_owned());
    let pass = std::env::var("QB_PASS").unwrap_or_else(|_| "adminadmin".to_owned());
    let write_test = std::env::var("QB_WRITE_TEST")
        .map(|v| v == "1")
        .unwrap_or(false);

    println!("连接 {url}（用户 {user}）");
    configure(&url, &user, &pass, 10_000);

    step("login", call("login", json!({})));
    step("version", call("version", json!({})));
    let info = step("ping", call("ping", json!({})));
    check_supported(&info);

    // 5.0 的 filter 白名单：旧名字（paused/resumed）自动映射，未知取值必须报错。
    for (label, filter) in [
        ("filter=stopped", "stopped"),
        ("filter=paused（4.x 旧名 → stopped）", "paused"),
    ] {
        step(label, call("list", json!({ "filter": filter, "limit": 1 })));
    }
    match call("list", json!({ "filter": "finished" })) {
        Ok(_) => {
            eprintln!("✗ 未知 filter 居然被接受了：qB 会把未知值当成 all，这里必须拒绝");
            std::process::exit(1);
        }
        Err(err) => println!("✓ 未知 filter 被拒绝：{err}"),
    }

    step("transfer", call("transfer", json!({})));

    let listed = step(
        "list",
        call(
            "list",
            json!({ "limit": 5, "sort": "added_on", "reverse": true }),
        ),
    );
    let torrents = listed.as_array().cloned().unwrap_or_default();
    println!("  当前种子数（前 5 条已展示）：{}", torrents.len());

    if let Some(first) = torrents.first() {
        let hash = first["hash"].as_str().unwrap_or_default();
        let name = first["name"].as_str().unwrap_or_default();
        println!(
            "  抽查首条：{}（{}，进度 {:.1}%）",
            name,
            hash,
            first["progress"].as_f64().unwrap_or(0.0) * 100.0
        );
        step("files", call("files", json!({ "hash": hash })));
        step("properties", call("properties", json!({ "hash": hash })));
        // 只读：不暂停、不删除别人正在跑的任务。
        step(
            "list（按 hash 过滤）",
            call("list", json!({ "hashes": hash })),
        );
    } else {
        println!("  列表为空，跳过 files/properties 抽查");
    }

    if write_test {
        write_round_trip();
    } else {
        println!("\n（写接口未测试；设 QB_WRITE_TEST=1 可跑 add → list → delete）");
    }

    println!("\n读接口全部通过：download.qbittorrent 与本机 qBittorrent 联调成功。");
}

/// 写接口往返：添加一个暂停的磁力 → 确认出现在列表 → 暂停/继续 → 删除（不删文件）。
///
/// **无论中间哪步失败，最后都会执行删除**——测试不能往用户的任务列表里留东西。
fn write_round_trip() {
    println!("\n── 写接口测试（临时种子，结束会删除）──");
    let hash = "08ada5a7a6183aae1e09d831df6748d566095a10";
    let mut failures: Vec<String> = Vec::new();

    report(
        &mut failures,
        "add",
        call(
            "add",
            json!({
                "urls": MAGNET,
                "category": "xq-smoke",
                "paused": true,
                "tags": "xq-smoke"
            }),
        ),
    );

    // 等 qB 把种子落库（它是异步处理的）。
    let mut added = false;
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if let Ok(value) = call("list", json!({ "hashes": hash }))
            && let Some(first) = value.as_array().and_then(|items| items.first())
        {
            added = true;
            let state = first["state"].as_str().unwrap_or("?");
            println!(
                "✓ 种子已入库：state={} progress={}",
                state,
                first["progress"].as_f64().unwrap_or(0.0)
            );
            // 5.0 里「添加但暂停」的结果是 stoppedDL；不是 stopped 就说明 stopped 参数没生效，
            // 那它会悄悄开始下载——这是必须报出来的错。
            if !state.starts_with("stopped") {
                failures.push(format!("add 的 stopped 参数未生效（state={state}）"));
                println!("⚠ state={state}，预期 stoppedDL：stopped 参数可能被忽略");
            }
            break;
        }
    }
    if !added {
        failures.push("种子 10 秒内没进列表".to_owned());
        println!("⚠ 10 秒内没在列表里看到新种子");
    }

    if added {
        report(
            &mut failures,
            "files",
            call("files", json!({ "hash": hash })),
        );
        report(
            &mut failures,
            "properties",
            call("properties", json!({ "hash": hash })),
        );
        report_endpoint(
            &mut failures,
            "pause",
            "/api/v2/torrents/stop",
            call("pause", json!({ "hashes": hash })),
        );
        report_endpoint(
            &mut failures,
            "resume",
            "/api/v2/torrents/start",
            call("resume", json!({ "hashes": hash })),
        );
    }

    // 清理：必须先于结论执行。
    match call("delete", json!({ "hashes": hash, "delete_files": false })) {
        Ok(_) => match call("list", json!({ "hashes": hash })) {
            Ok(value) if value.as_array().map(|a| a.is_empty()).unwrap_or(false) => {
                println!("✓ 清理完成：临时种子已移除")
            }
            Ok(value) => failures.push(format!("删除后仍能查到：{}", brief(&value.to_string()))),
            Err(err) => failures.push(format!("删除后查询失败：{err}")),
        },
        Err(err) => failures.push(format!("删除失败（请手动清理 {hash}）：{err}")),
    }

    if failures.is_empty() {
        println!("\n写接口全部通过。");
    } else {
        eprintln!(
            "\n写接口有 {} 项失败：{}",
            failures.len(),
            failures.join("；")
        );
        std::process::exit(1);
    }
}

fn report(failures: &mut Vec<String>, label: &str, result: Result<Value, String>) {
    match result {
        Ok(value) => println!("✓ {label}: {}", brief(&value.to_string())),
        Err(err) => {
            eprintln!("✗ {label} 失败：{err}");
            failures.push(format!("{label}: {err}"));
        }
    }
}

/// 写操作要确认打的是 5.0 的端点，而不是 4.x 的 pause/resume。
fn report_endpoint(
    failures: &mut Vec<String>,
    label: &str,
    expected: &str,
    result: Result<Value, String>,
) {
    match result {
        Ok(value) => {
            let used = value["endpoint"].as_str().unwrap_or("");
            if used == expected {
                println!("✓ {label}: {used}");
            } else {
                eprintln!("✗ {label} 打到了 {used}，应为 {expected}");
                failures.push(format!("{label}: 端点 {used} != {expected}"));
            }
        }
        Err(err) => {
            eprintln!("✗ {label} 失败：{err}");
            failures.push(format!("{label}: {err}"));
        }
    }
}

/// 版本必须是 5.x —— 写接口的端点/参数都按 5.0 写，低版本会静默走错语义。
fn check_supported(info: &Value) {
    let version = info["version"].as_str().unwrap_or("未知");
    let api_version = info["api_version"].as_str().unwrap_or("未知");
    println!("  服务器：qBittorrent {version}（WebAPI {api_version}）");
    if info["supported"].as_bool().unwrap_or(false) {
        println!("✓ 版本满足 5.0+，写接口可用");
    } else {
        eprintln!(
            "✗ qBittorrent {version} 低于 5.0：本插件按 5.x 接口实现，读接口可用但写接口会被拒绝"
        );
        std::process::exit(1);
    }
}

fn step(label: &str, result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => {
            println!("✓ {label}: {}", brief(&value.to_string()));
            value
        }
        Err(err) => {
            eprintln!("✗ {label} 失败：{err}");
            std::process::exit(1);
        }
    }
}

fn brief(text: &str) -> String {
    const LIMIT: usize = 260;
    if text.chars().count() <= LIMIT {
        return text.to_owned();
    }
    let head: String = text.chars().take(LIMIT).collect();
    format!("{head}…（共 {} 字符）", text.chars().count())
}
