//! 渲染端：Ultralight 加载 HTML → 截图 → 编码落盘。
//!
//! 只有 `image-shot` 这个 bin 会用到本模块（插件侧只跑 [`crate::client`]）。
//! 拆成「插件」和「渲染器」两个产物仍然是必须的：Ultralight 的 Renderer 必须在
//! 主线程创建并驱动，而宿主的主线程归 tokio。
//!
//! 协议与插件侧共用 [`RenderRequest`]：一行一个 JSON 请求，一行一个 JSON 响应。

pub mod ffi;

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;

use crate::request::RenderRequest;
use ffi::{Error, Handle, Result, Ultra};

/// 渲染器回给调用方的一条响应。
#[derive(Debug, Serialize, Default)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<serde_json::Value>,
}

impl Response {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            ..Default::default()
        }
    }
}

/// 处理一条请求。`renderer` 由调用方持有（进程级单例）。
pub fn handle(ultra: Option<&Ultra>, renderer: Option<Handle>, request: RenderRequest) -> Response {
    match request.op.as_str() {
        "ping" => Response {
            ok: true,
            info: Some(serde_json::json!({
                "version": "2.0",
                "runtime": ultra.map(|u| u.dir.display().to_string()),
                "templates": templates_dir().map(|p| p.display().to_string()),
            })),
            ..Default::default()
        },
        "render" => render(ultra, renderer, request),
        "shutdown" => Response {
            ok: true,
            ..Default::default()
        },
        other => Response::error(format!("未知 op：{other}（支持 render / ping / shutdown）")),
    }
}

fn render(ultra: Option<&Ultra>, renderer: Option<Handle>, request: RenderRequest) -> Response {
    let (Some(ultra), Some(renderer)) = (ultra, renderer) else {
        return Response::error("Ultralight 运行时未就绪");
    };
    let started = Instant::now();

    let html = match compose_html(&request) {
        Ok(html) => html,
        Err(err) => return Response::error(err.to_string()),
    };

    match ultra.render_html(
        renderer,
        &html,
        request.width,
        request.height,
        request.scale,
        request.timeout_ms,
        request.ready_expr.as_deref(),
    ) {
        Ok((rgb, width, height)) => {
            let image = match image::RgbImage::from_raw(width, height, rgb) {
                Some(image) => image,
                None => return Response::error("像素缓冲尺寸不匹配"),
            };
            let encoded = match encode(&image, &request) {
                Ok(bytes) => bytes,
                Err(err) => return Response::error(err.to_string()),
            };
            let path = match request.out.as_deref() {
                Some(out) => out.to_path_buf(),
                None => default_out(&request.format),
            };
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
                && let Err(err) = std::fs::create_dir_all(parent)
            {
                return Response::error(format!("创建目录失败（{}）：{err}", parent.display()));
            }
            if let Err(err) = std::fs::write(&path, &encoded) {
                return Response::error(format!("写文件失败（{}）：{err}", path.display()));
            }
            Response {
                ok: true,
                path: Some(path.display().to_string()),
                width: Some(width),
                height: Some(height),
                bytes: Some(encoded.len()),
                ms: Some(started.elapsed().as_millis() as u64),
                ..Default::default()
            }
        }
        Err(err) => Response::error(err.to_string()),
    }
}

fn encode(image: &image::RgbImage, request: &RenderRequest) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    match request.format.to_ascii_lowercase().as_str() {
        "png" => {
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(
                    image.as_raw(),
                    image.width(),
                    image.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(Error::Image)?;
        }
        _ => {
            image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut out,
                request.quality.clamp(1, 100),
            )
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(Error::Image)?;
        }
    }
    Ok(out)
}

fn default_out(format: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let ext = if format.eq_ignore_ascii_case("png") {
        "png"
    } else {
        "jpg"
    };
    std::env::temp_dir().join(format!("image-shot-{stamp}.{ext}"))
}

/// 把请求里的「HTML 或 模板 + 数据」合成最终要渲染的 HTML。
///
/// - `html` + `data`：把 `html` 当模板绑一遍（插件内嵌模板文本就是走这条）；
/// - 只有 `html`：调用方已绑定完，原样渲染；
/// - `template` + `data`：读 `template_dir/<name>.html` 再绑定。
fn compose_html(request: &RenderRequest) -> Result<String> {
    match (&request.html, &request.template) {
        (Some(html), _) => {
            if request.data.is_null() {
                Ok(html.clone())
            } else {
                crate::template::render(html, &request.data).map_err(Error::Template)
            }
        }
        (None, Some(name)) => {
            compose_template(name, &request.data, request.template_dir.as_deref())
        }
        (None, None) => Err(Error::Render("必须给 template 或 html".to_owned())),
    }
}

/// 读模板文件、把数据绑进去，得到最终 HTML。
///
/// `dir` 为空时用本 crate 的 `templates/`（只有 demo，业务模板必须自己传路径）。
/// 绑定走 [`crate::template`]（mustache 子集）：进渲染器的就是最终页面，
/// 没有 Vue / JS 运行时，也没有 `__READY__` 之类的就绪协议。
pub fn compose_template(
    name: &str,
    data: &serde_json::Value,
    dir: Option<&Path>,
) -> Result<String> {
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(Error::Render(format!("模板名非法：{name}")));
    }
    let dir = match dir {
        Some(dir) => dir.to_path_buf(),
        None => templates_dir()
            .ok_or_else(|| Error::RuntimeMissing("找不到 templates/ 目录".to_string()))?,
    };
    let path = dir.join(format!("{name}.html"));
    let html = std::fs::read_to_string(&path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;

    crate::template::render(&html, data).map_err(Error::Template)
}

/// 本 crate 自带的模板目录（只有 demo；业务模板由调用方通过 `template_dir` 指定）。
pub fn templates_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("IMAGE_TEMPLATES_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("templates"));
        candidates.push(dir.join("../templates"));
    }
    if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(Path::new(manifest).join("templates"));
    }
    candidates.into_iter().find(|dir| dir.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_name_is_validated_against_path_tricks() {
        let err = compose_template("../secret", &serde_json::json!({}), Some(Path::new(".")))
            .unwrap_err();
        assert!(err.to_string().contains("模板名非法"), "{err}");
    }

    #[test]
    fn data_is_html_escaped_so_it_cannot_break_the_page() {
        let dir = std::env::temp_dir().join("ugi-test-templates");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t.html"), "<html><p>{{ x }}</p></html>").unwrap();
        let html =
            compose_template("t", &serde_json::json!({"x": "</p><b>"}), Some(&dir)).unwrap();
        assert!(
            !html.contains("</p><b>"),
            "数据里的标签必须被转义：{html}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 修复回归：html + data 也必须做绑定（插件内嵌模板文本走的就是这条路）。
    #[test]
    fn html_with_data_is_bound_like_a_template() {
        let request = RenderRequest {
            html: Some("<html>{{ title }}</html>".to_owned()),
            data: serde_json::json!({ "title": "芙莉莲" }),
            ..Default::default()
        };
        let html = compose_html(&request).unwrap();
        assert_eq!(html, "<html>芙莉莲</html>");
    }

    #[test]
    fn html_without_data_is_final_and_passes_through() {
        let request = RenderRequest::html("<html>已是最终页面</html>", "x.jpg");
        let html = compose_html(&request).unwrap();
        assert_eq!(html, "<html>已是最终页面</html>");
    }

    #[test]
    fn template_missing_field_fails_before_rendering() {
        let dir = std::env::temp_dir().join("ugi-test-templates2");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t.html"), "<html>{{ title }}</html>").unwrap();
        let err = compose_template("t", &serde_json::json!({}), Some(&dir)).unwrap_err();
        assert!(err.to_string().contains("title"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
