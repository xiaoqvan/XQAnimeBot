//! `image-shot`：Ultralight 2.0 离屏截图器（ultralightGenImage 插件的一部分）。
//!
//! 为什么是独立进程而不是 dll 里直接调：Ultralight 要求 Renderer 在**主线程**创建并
//! 驱动，宿主的主线程归 tokio，插件抢不到。这里是它自己的主线程。
//!
//! ```text
//! image-shot --check                 # 检查运行时是否就位
//! image-shot --once req.json         # 渲染一次（不给文件则读 stdin）
//! image-shot --serve                 # 常驻：stdin 一行一个请求，stdout 一行一个响应
//! ```
//!
//! 退出码：0 正常，2 运行时缺失，3 渲染失败。

use std::io::{BufRead, Read, Write};

use ultralight_gen_image::RenderRequest;
use ultralight_gen_image::render::ffi::Ultra;
use ultralight_gen_image::render::{self, Response, ffi};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => usage(),
        Some("--check") => check(),
        Some("--once") => once(args.get(1).cloned()),
        Some("--serve") | None => serve(),
        Some(other) => {
            eprintln!("未知参数：{other}");
            usage();
            std::process::exit(2);
        }
    }
}

fn usage() {
    eprintln!(
        "image-shot 2.0（ultralightGenImage）\n\
         \n\
         --check            检查 Ultralight 运行时\n\
         --once [req.json]  渲染一次（缺省读 stdin）\n\
         --serve            常驻 JSON 行协议\n\
         \n\
         环境变量：ULTRALIGHT_DIR（运行时目录）/ IMAGE_TEMPLATES_DIR（默认模板目录）"
    );
}

fn check() {
    let Some(dir) = ffi::find_runtime_dir() else {
        fail(
            "找不到 Ultralight 运行时（设置 ULTRALIGHT_DIR，或把 DLL 放到 exe 同级的 ultralight/）",
        );
    };
    match Ultra::load(&dir) {
        Ok(ultra) => {
            let payload = serde_json::json!({
                "ok": true,
                "runtime": ultra.dir.display().to_string(),
                "templates": render::templates_dir().map(|p| p.display().to_string()),
            });
            println!("{payload}");
        }
        Err(err) => fail(&err.to_string()),
    }
}

fn once(path: Option<String>) {
    let raw = match path {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) => fail(&format!("读请求文件失败（{path}）：{err}")),
        },
        None => {
            let mut buf = String::new();
            if let Err(err) = std::io::stdin().read_to_string(&mut buf) {
                fail(&format!("读 stdin 失败：{err}"));
            }
            buf
        }
    };
    let response = match serde_json::from_str::<RenderRequest>(&raw) {
        Ok(request) => run(request),
        Err(err) => Response::error(format!("请求 JSON 解析失败：{err}")),
    };
    println!("{}", serde_json::to_string(&response).unwrap_or_default());
    if !response.ok {
        std::process::exit(3);
    }
}

fn serve() {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // 运行时只加载一次；加载失败也继续 serve，把错误逐条回给调用方，
    // 这样插件能看到「为什么没图」而不是「子进程无声退出」。
    let (ultra, renderer) = match ffi::find_runtime_dir() {
        Some(dir) => match Ultra::load(&dir).and_then(|ultra| {
            let renderer = ultra.create_renderer()?;
            Ok((ultra, renderer))
        }) {
            Ok((ultra, renderer)) => (Some(ultra), Some(renderer)),
            Err(err) => {
                eprintln!("[image-shot] {err}");
                (None, None)
            }
        },
        None => {
            eprintln!("[image-shot] 找不到 Ultralight 运行时");
            (None, None)
        }
    };

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<RenderRequest>(line) {
            Ok(request) => {
                let op = request.op.clone();
                let response = render::handle(ultra.as_ref(), renderer, request);
                let _ = writeln!(
                    out,
                    "{}",
                    serde_json::to_string(&response).unwrap_or_default()
                );
                let _ = out.flush();
                if op == "shutdown" {
                    break;
                }
                continue;
            }
            Err(err) => Response::error(format!("请求 JSON 解析失败：{err}")),
        };
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string(&response).unwrap_or_default()
        );
        let _ = out.flush();
    }

    if let (Some(ultra), Some(renderer)) = (&ultra, renderer) {
        unsafe { ultra.destroy_renderer(renderer) };
    }
}

fn run(request: RenderRequest) -> Response {
    let Some(dir) = ffi::find_runtime_dir() else {
        return Response::error(
            "找不到 Ultralight 运行时（ULTRALIGHT_DIR 未设置且默认路径不存在）",
        );
    };
    match Ultra::load(&dir) {
        Ok(ultra) => match ultra.create_renderer() {
            Ok(renderer) => {
                let response = render::handle(Some(&ultra), Some(renderer), request);
                unsafe { ultra.destroy_renderer(renderer) };
                response
            }
            Err(err) => Response::error(err.to_string()),
        },
        Err(err) => Response::error(err.to_string()),
    }
}

fn fail(message: &str) -> ! {
    println!(
        "{}",
        serde_json::to_string(&Response::error(message)).unwrap_or_default()
    );
    std::process::exit(2)
}
