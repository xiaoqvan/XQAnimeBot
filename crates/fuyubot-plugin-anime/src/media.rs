//! `media.ffmpeg` 的薄客户端。
//!
//! 只包链路真的会用的六个动作：在不在、探测、出方案、抽字幕、起转码、查进度。
//! 转码是**异步**的：`transcode` 起完线程立刻返回 job id，完成与否要轮询
//! `job_status`（ffmpeg 插件同时会发 `ffmpeg.progress` 事件，但这条直线链路
//! 一次只处理一个文件，轮询比订阅事件少一个状态机）。
//!
//! 响应形状由 ffmpeg 插件决定，这里不做二次建模：`probe` 平铺 `MediaInfo`，
//! `plan` 是 `{plan, media}`，`extract_subtitle` 是 `{extracted, output?, ...}`，
//! `job_status` 是 `{job}`。业务层要什么字段自己读，避免两边各维护一份结构体、
//! 改一边忘一边。
//!
//! 调用约定与 `qb.rs` 一致：服务返回 `{"ok":true, ...}`，失败时
//! `call_service` 已经把 `error` 提成 `String`，这里统一转成
//! [`anime_core::Error::Service`]，日志里能一眼看出是宿主里的服务坏了。

use std::path::{Path, PathBuf};

use anime_core::{Error, Result};
use fuyubot_abi::HostApi;
use fuyubot_service::{call_service, service_available};
use serde_json::{Value, json};

const SERVICE: &str = "media.ffmpeg";

/// 媒体客户端。`HostApi` 是 `Copy` 的，整个结构可以随手复制进线程。
#[derive(Debug, Clone, Copy)]
pub struct Media {
    host: HostApi,
}

impl Media {
    pub fn new(host: HostApi) -> Self {
        Self { host }
    }

    /// 插件装没装、起没起。转码是链路里可选的增强：不在线就直发原文件。
    pub fn available(&self) -> bool {
        service_available(&self.host, SERVICE, "transcode")
    }

    /// 读输入文件的真实参数：`video_codec` / `audio_codec` / `has_audio` /
    /// `duration_ms` / `has_subtitle` / `subtitle_streams` 等，形状见 ffmpeg
    /// 插件的 `MediaInfo::to_json`。
    pub fn probe(&self, input: &Path) -> Result<Value> {
        self.call("probe", json!({ "input": path_text(input) }))
    }

    /// 问「这个输入转成那个输出该用什么参数」。返回 `{plan, media}`。
    ///
    /// `subtitle` 是已经抽出来的外挂字幕；非空时方案会带
    /// `-vf subtitles=…`，并且**强制重编码**。
    pub fn plan(
        &self,
        input: &Path,
        output: &Path,
        intent: &str,
        subtitle: Option<&Path>,
    ) -> Result<Value> {
        let mut payload = json!({
            "input": path_text(input),
            "output": path_text(output),
            "intent": intent,
        });
        if let Some(subtitle) = subtitle {
            payload["subtitle"] = json!(path_text(subtitle));
        }
        self.call("plan", payload)
    }

    /// 把内嵌字幕抽成外挂 `.ass`。失败是错误，`Ok(None)` 只表示「没有字幕流」。
    pub fn extract_subtitle(&self, input: &Path, output: &Path) -> Result<Option<PathBuf>> {
        let reply = self.call(
            "extract_subtitle",
            json!({
                "input": path_text(input),
                "output": path_text(output),
            }),
        )?;
        if reply.get("extracted").and_then(Value::as_bool) != Some(true) {
            return Ok(None);
        }
        let path = reply
            .get("output")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| output.to_path_buf());
        Ok(Some(path))
    }

    /// 起一个转码任务，返回 job id。**立刻返回**，完成与否查 [Self::job_status]。
    ///
    /// `subtitle` 非空时 ffmpeg 会把字幕烧进画面。
    pub fn transcode(
        &self,
        input: &Path,
        output: &Path,
        intent: &str,
        subtitle: Option<&Path>,
    ) -> Result<String> {
        let mut payload = json!({
            "input": path_text(input),
            "output": path_text(output),
            "intent": intent,
        });
        if let Some(subtitle) = subtitle {
            payload["subtitle"] = json!(path_text(subtitle));
        }
        let reply = self.call("transcode", payload)?;
        reply
            .get("job_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| Error::Service {
                service: SERVICE.to_owned(),
                message: format!("transcode 没有返回 job_id：{reply}"),
            })
    }

    /// 按大小分段。返回各段路径（顺序即播放顺序）。
    ///
    /// `duration_ms` 只用来估算分段时长；传 0 会退化成最短 10 秒一段。
    pub fn split(
        &self,
        input: &Path,
        out_dir: &Path,
        max_bytes: u64,
        duration_ms: u64,
    ) -> Result<Vec<PathBuf>> {
        let reply = self.call(
            "split",
            json!({
                "input": path_text(input),
                "out_dir": path_text(out_dir),
                "max_bytes": max_bytes,
                "duration_ms": duration_ms,
            }),
        )?;
        let parts = reply
            .get("parts")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if parts.is_empty() {
            return Err(Error::Service {
                service: SERVICE.to_owned(),
                message: "分段没有产出任何文件".to_owned(),
            });
        }
        Ok(parts)
    }

    /// 查任务状态。返回 `job` 对象，`state` 取值
    /// `running` / `done` / `failed` / `canceled`。
    pub fn job_status(&self, id: &str) -> Result<Value> {
        let reply = self.call("job_status", json!({ "id": id }))?;
        reply.get("job").cloned().ok_or_else(|| Error::Service {
            service: SERVICE.to_owned(),
            message: format!("job_status 没有返回 job：{reply}"),
        })
    }

    fn call(&self, method: &str, input: Value) -> Result<Value> {
        call_service(&self.host, SERVICE, method, &input).map_err(|message| Error::Service {
            service: SERVICE.to_owned(),
            message,
        })
    }
}

/// 服务参数只收 UTF-8 字符串。Windows 路径用 `display` 原样给出，
/// 不额外做分隔符转换——ffmpeg 自己能认反斜杠。
fn path_text(path: &Path) -> String {
    path.display().to_string()
}
