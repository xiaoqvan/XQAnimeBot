//! **ultralightGenImage** —— 一个插件搞定 HTML 出图。
//!
//! 对外只暴露一个服务 `image.render`，没有任何业务字段：给它 HTML（或「模板 + 数据」），
//! 它还你一张图。动漫卡、周报、订单、报表都走同一条路。
//!
//! ```text
//! 任意业务插件
//!     │  host.get_service("image.render") → ServiceVTable
//!     │  vtable.methods["render"]({"template":"x","template_dir":"...","data":{...},"out":"..."})
//!     ▼
//! 本插件（宿主进程内的 cdylib）
//!     │  JSON 行协议，stdin/stdout
//!     ▼
//! image-shot.exe（同包产物，独立进程，独占主线程）
//!     │  Ultralight 2.0：加载最终 HTML（数据已绑好）→ 截图
//!     ▼
//!   JPEG/PNG 路径
//! ```
//!
//! # 为什么一个插件里有两个产物
//!
//! Ultralight 要求 Renderer 在**主线程**创建并驱动，而宿主进程的主线程属于 FuyuBot 的
//! tokio 运行时——插件里没有「主线程」可用，硬塞会在某些平台上直接崩。所以：
//!
//! - `ultralight-gen-image.dll`（本 crate 的 lib）：插件本体，只发请求；
//! - `image-shot.exe`（本 crate 的 bin）：真正碰 Ultralight 的进程。
//!
//! 两个产物来自**同一个 crate**，请求类型也只有一份（[`RenderRequest`]），
//! 不会再出现「插件和渲染器字段对不上」的问题。渲染器崩了只崩一张图，机器人不受影响。
//!
//! 服务是**同步**的：`ServiceMethodFn` 只有同步形态。调用方应放在 `spawn_blocking`
//! 或专用线程里，不要塞进 TDLib 的更新回调线程。

pub mod client;
pub mod render;
pub mod request;
pub mod template;

use std::ffi::{c_char, c_void};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use fuyubot_abi::{HostApi, LogLevel, Plugin};
use fuyubot_service::{Handler, Service, error, ok, parse};
use serde_json::{Value, json};

pub use client::{Error as ClientError, Rendered, ShotClient};
pub use request::RenderRequest;

/// 插件标识（宿主日志与插件列表里显示的名字）。
pub const PLUGIN_NAME: &str = "ultralightGenImage";

/// 服务名：`image.render`。任何插件按这个名字取。
pub const SERVICE_NAME: &str = "image.render";

/// 插件启动配置（都可省略）：
///
/// ```json
/// { "binary": "D:/.../image-shot.exe", "templates_dir": "D:/.../templates" }
/// ```
#[derive(Debug, Default)]
struct Settings {
    /// 显式指定 `image-shot` 路径；不填就按 `IMAGE_SHOT_BIN` + 约定目录自动找。
    binary: Option<PathBuf>,
    /// 请求里没给 `template_dir` 时用的默认模板目录。
    templates_dir: Option<PathBuf>,
}

static SETTINGS: OnceLock<Mutex<Settings>> = OnceLock::new();

fn settings() -> &'static Mutex<Settings> {
    SETTINGS.get_or_init(|| Mutex::new(Settings::default()))
}

struct ImageGenPlugin {
    host: HostApi,
}

impl Plugin for ImageGenPlugin {
    fn init(host: HostApi) -> Result<Self, String> {
        Service::new(
            SERVICE_NAME,
            vec![
                ("render", svc_render as Handler),
                ("ping", svc_ping as Handler),
            ],
        )
        .register(&host)?;
        Ok(Self { host })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        apply_settings(config_json);

        // 预热：提前把渲染进程拉起来，第一张图不用等进程启动。
        // 失败不算致命——出图时会重试，并把具体错误回给调用方。
        match with_client(|client| client.ping()) {
            Ok(info) => self.host.log(
                LogLevel::Info,
                &format!(
                    "[{PLUGIN_NAME}] 渲染器已就绪：{}",
                    info.get("info")
                        .and_then(|value| value.get("runtime"))
                        .and_then(Value::as_str)
                        .unwrap_or("运行时未知")
                ),
            ),
            Err(err) => self.host.log(
                LogLevel::Warn,
                &format!("[{PLUGIN_NAME}] 预热失败：{err}（首次出图时会重试）"),
            ),
        }
        Ok(())
    }
}

// `define_plugin!` 只收字面量，所以这里重复一遍；下面有测试钉住两者一致。
fuyubot_abi::define_plugin!(ImageGenPlugin, "ultralightGenImage");

fn apply_settings(config_json: &str) {
    let parsed: Value = serde_json::from_str(config_json).unwrap_or(Value::Null);
    let mut guard = settings()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(binary) = parsed.get("binary").and_then(Value::as_str) {
        guard.binary = Some(PathBuf::from(binary));
    }
    if let Some(dir) = parsed.get("templates_dir").and_then(Value::as_str) {
        guard.templates_dir = Some(PathBuf::from(dir));
    }
}

// ─── 服务实现 ──────────────────────────────────────────────────────────────

/// 常驻客户端。所有请求串行——Ultralight 本身就是单线程渲染，并发只会互相等。
static CLIENT: OnceLock<Mutex<Option<ShotClient>>> = OnceLock::new();

unsafe extern "C" fn svc_render(_context: *mut c_void, input_json: *const c_char) -> *const c_char {
    let request = match parse(input_json).and_then(|value| {
        serde_json::from_value::<RenderRequest>(value)
            .map_err(|err| format!("出图参数不合法：{err}"))
    }) {
        Ok(request) => request,
        Err(message) => return error(message),
    };

    if request
        .out
        .as_ref()
        .is_none_or(|out| out.as_os_str().is_empty())
    {
        return error("缺少 out（渲染结果必须落盘，TDLib 只接受本地路径）");
    }
    if request.html.is_none() && request.template.is_none() {
        return error("必须给 html 或 template");
    }

    // 调用方没指定模板目录时，用插件配置里的默认目录（如果配了）。
    let mut request = request;
    if request.template_dir.is_none() {
        let guard = settings()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        request.template_dir = guard.templates_dir.clone();
    }

    match with_client(|client| client.render(&request)) {
        Ok(rendered) => ok(json!({
            "path": rendered.path.display().to_string(),
            "width": rendered.width,
            "height": rendered.height,
            "bytes": rendered.bytes,
            "ms": rendered.ms,
        })),
        Err(err) => error(err.to_string()),
    }
}

unsafe extern "C" fn svc_ping(_context: *mut c_void, _input_json: *const c_char) -> *const c_char {
    match with_client(|client| client.ping()) {
        Ok(info) => ok(json!({ "backend": "ultralight", "plugin": PLUGIN_NAME, "info": info })),
        Err(err) => error(err.to_string()),
    }
}

/// 取客户端，没有就拉起来；请求失败就把客户端丢掉，下次重新连。
fn with_client<T>(
    run: impl FnOnce(&mut ShotClient) -> Result<T, ClientError>,
) -> Result<T, ClientError> {
    let slot = CLIENT.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        let binary = {
            let config = settings()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            config.binary.clone()
        };
        let mut client = match binary {
            Some(binary) => ShotClient::spawn(&binary)?,
            None => ShotClient::auto()?,
        };
        // 单张卡片实测 0.2~1s；给宽一点，避免机器负载高时误杀。
        client.set_timeout(Duration::from_secs(20));
        *guard = Some(client);
    }
    let client = guard.as_mut().expect("上面刚填过");
    match run(client) {
        Ok(value) => Ok(value),
        Err(err) => {
            // 子进程崩了 / 超时：丢弃句柄，下次请求重新拉起。
            *guard = None;
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn service_declares_render_and_ping() {
        let service = Service::new(
            SERVICE_NAME,
            vec![
                ("render", svc_render as Handler),
                ("ping", svc_ping as Handler),
            ],
        );
        assert_eq!(service.name(), "image.render");
        assert_eq!(service.vtable().method_count, 2);
        assert_eq!(service.methods().len(), 2);
    }

    #[test]
    fn plugin_identity_is_the_requested_name() {
        assert_eq!(PLUGIN_NAME, "ultralightGenImage");
    }

    #[test]
    fn malformed_input_returns_error_json() {
        let bad = CString::new("{not json").unwrap();
        let raw = unsafe { svc_render(std::ptr::null_mut(), bad.as_ptr()) };
        let text = unsafe { std::ffi::CStr::from_ptr(raw) }.to_str().unwrap();
        let value: Value = serde_json::from_str(text).unwrap();
        assert_eq!(value.get("ok").and_then(Value::as_bool), Some(false));
    }

    #[test]
    fn missing_out_is_rejected_before_touching_child_process() {
        let input = CString::new(r#"{"html":"<html></html>"}"#).unwrap();
        let raw = unsafe { svc_render(std::ptr::null_mut(), input.as_ptr()) };
        let text = unsafe { std::ffi::CStr::from_ptr(raw) }.to_str().unwrap();
        let value: Value = serde_json::from_str(text).unwrap();
        assert_eq!(value.get("ok").and_then(Value::as_bool), Some(false));
        assert!(value["error"].as_str().unwrap().contains("out"));
    }
}
