//! 出图请求：**插件与渲染器共用这一个类型**。
//!
//! 以前插件侧一份 `RenderRequest`、渲染器侧一份 `Request`，字段靠手抄对齐，
//! 改一个字段要改两处、错一个字段只在运行时炸。合并成一个 crate 之后没有理由再分，
//! 这里就是唯一事实来源：插件把它序列化成 JSON 行，渲染器反序列化回同一个结构。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 一次出图（或 ping / shutdown）请求。
///
/// 两种内容二选一：
///
/// ```ignore
/// // 1) 直接给整段 HTML —— 调用方自己负责数据绑定
/// RenderRequest::html(full_html, out)
///
/// // 2) 给模板目录 + 模板名 + 数据 —— 渲染器注入 Vue 运行时与数据
/// RenderRequest::template("bangumi-card", payload, out)
///     .with_template_dir(".../assets/templates")
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RenderRequest {
    /// `render`（默认）/ `ping` / `shutdown`。
    pub op: String,
    /// 整段 HTML，优先于 `template`。
    pub html: Option<String>,
    /// 模板名，解析为 `<template_dir>/<name>.html`。
    pub template: Option<String>,
    /// 模板目录。业务插件应当显式给，这样渲染器不必认识任何业务。
    pub template_dir: Option<PathBuf>,
    /// 注入模板的数据（替换 `__IMAGE_DATA__` 占位符）。
    pub data: Value,
    /// 输出路径。TDLib 只接受本地路径，所以结果必须落盘。
    /// 渲染器侧可以省略（写临时目录），插件侧要求必填。
    pub out: Option<PathBuf>,
    /// CSS 视口宽。
    pub width: u32,
    /// CSS 视口高。
    pub height: u32,
    /// 设备像素比：2.0 表示 2× 超采样。
    pub scale: f64,
    /// `jpeg` 或 `png`。
    pub format: String,
    /// JPEG 质量 1..=100。
    pub quality: u8,
    /// 页面就绪判定的 JS 表达式（求值为 `true` 才截图）。
    pub ready_expr: Option<String>,
    /// 渲染超时（毫秒）。
    pub timeout_ms: u64,
}

impl Default for RenderRequest {
    fn default() -> Self {
        Self {
            op: "render".to_owned(),
            html: None,
            template: None,
            template_dir: None,
            data: Value::Null,
            out: None,
            width: 1000,
            height: 550,
            scale: 2.0,
            format: "jpeg".to_owned(),
            quality: 90,
            ready_expr: Some("window.__READY__ === true".to_owned()),
            timeout_ms: 15_000,
        }
    }
}

impl RenderRequest {
    /// 直接渲染一段完整 HTML。
    pub fn html(html: impl Into<String>, out: impl Into<PathBuf>) -> Self {
        Self {
            html: Some(html.into()),
            out: Some(out.into()),
            // 没有模板就没有注入点，别等 `__READY__`。
            ready_expr: None,
            ..Default::default()
        }
    }

    /// 用模板 + 数据渲染。
    pub fn template(name: impl Into<String>, data: Value, out: impl Into<PathBuf>) -> Self {
        Self {
            template: Some(name.into()),
            data,
            out: Some(out.into()),
            ..Default::default()
        }
    }

    /// 指定模板目录（业务插件应该显式指定，别依赖渲染器的默认路径）。
    pub fn with_template_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.template_dir = Some(dir.into());
        self
    }

    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn with_scale(mut self, scale: f64) -> Self {
        self.scale = scale;
        self
    }

    pub fn with_quality(mut self, quality: u8) -> Self {
        self.quality = quality.clamp(1, 100);
        self
    }

    pub fn with_format(mut self, format: impl Into<String>) -> Self {
        self.format = format.into();
        self
    }

    pub fn with_ready_expr(mut self, expr: impl Into<String>) -> Self {
        self.ready_expr = Some(expr.into());
        self
    }

    pub fn with_op(mut self, op: impl Into<String>) -> Self {
        self.op = op.into();
        self
    }

    /// 转成子进程能读的 JSON（省略没设置的可选字段，报文更小也更好读）。
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "op": self.op,
            "data": self.data,
            "width": self.width,
            "height": self.height,
            "scale": self.scale,
            "format": self.format,
            "quality": self.quality,
            "timeout_ms": self.timeout_ms,
        });
        let object = value.as_object_mut().expect("上面就是 object");
        if let Some(template) = &self.template {
            object.insert("template".to_owned(), json!(template));
        }
        if let Some(html) = &self.html {
            object.insert("html".to_owned(), json!(html));
        }
        if let Some(dir) = &self.template_dir {
            object.insert("template_dir".to_owned(), json!(dir.display().to_string()));
        }
        if let Some(out) = &self.out {
            object.insert("out".to_owned(), json!(out.display().to_string()));
        }
        if let Some(expr) = &self.ready_expr {
            object.insert("ready_expr".to_owned(), json!(expr));
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_request_carries_all_fields() {
        let request = RenderRequest::template("demo", json!({ "a": 1 }), "/tmp/x.jpg")
            .with_template_dir("/templates")
            .with_size(800, 400)
            .with_scale(1.5);
        let value = request.to_json();
        assert_eq!(value["op"], "render");
        assert_eq!(value["template"], "demo");
        assert_eq!(value["template_dir"], "/templates");
        assert_eq!(value["width"], 800);
        assert_eq!(value["height"], 400);
        assert_eq!(value["scale"], 1.5);
        assert_eq!(value["data"]["a"], 1);
        assert!(value.get("html").is_none());
    }

    #[test]
    fn html_request_skips_ready_expr() {
        let request = RenderRequest::html("<html></html>", "/tmp/x.jpg");
        let value = request.to_json();
        assert!(value.get("template").is_none());
        assert!(value.get("ready_expr").is_none());
        assert_eq!(value["html"], "<html></html>");
    }

    #[test]
    fn quality_is_clamped() {
        assert_eq!(
            RenderRequest::html("x", "/tmp/x.jpg")
                .with_quality(200)
                .quality,
            100
        );
    }

    /// 插件写出去、渲染器读回来必须是同一个结构（这是合并成一份类型的意义）。
    #[test]
    fn json_round_trip_keeps_every_field() {
        let request = RenderRequest::template("card", json!({"n": 2}), "out.jpg")
            .with_template_dir("tpl")
            .with_size(1200, 640)
            .with_scale(2.0)
            .with_format("png")
            .with_quality(88);
        let raw = serde_json::to_string(&request.to_json()).unwrap();
        let back: RenderRequest = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.template.as_deref(), Some("card"));
        assert_eq!(back.template_dir.unwrap().to_str(), Some("tpl"));
        assert_eq!(back.out.unwrap().to_str(), Some("out.jpg"));
        assert_eq!((back.width, back.height), (1200, 640));
        assert_eq!(back.format, "png");
        assert_eq!(back.quality, 88);
        assert_eq!(back.data["n"], 2);
    }

    /// 老调用方只发 `{"op":"ping"}` 也得能反序列化（容器级 default）。
    #[test]
    fn bare_ping_uses_defaults() {
        let request: RenderRequest = serde_json::from_str(r#"{"op":"ping"}"#).unwrap();
        assert_eq!(request.op, "ping");
        assert_eq!(request.width, 1000);
        assert!(request.out.is_none());
    }
}
