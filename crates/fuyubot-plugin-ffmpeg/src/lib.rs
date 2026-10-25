//! 转码插件：注册 `media.ffmpeg` 服务。
//!
//! 三个能力，别的插件按名字取：
//!
//! | 方法 | 干什么 |
//! | --- | --- |
//! | `capabilities` | 这台机器的 ffmpeg 会什么：硬件编码器（**实测可用**）、版本、硬件加速 |
//! | `plan` | 给输入 + 意图，返回**选好的方案**（编码器 + 完整参数 + 为什么选它） |
//! | `transcode` | 真跑：起任务立刻返回 id，进度走事件总线，也可以 `job_status` 轮询 |
//! | `extract_subtitle` | 把内嵌字幕抽成外挂 `.ass`，给「烧录字幕」用 |
//! | `split` | 按大小 `-c copy` 分段（TG 2GB 上限） |
//!
//! 「高度优化」落在三件事上：
//!
//! 1. **能 copy 就不 encode**——容器装得下原编码时零损失、几秒完成；
//! 2. **硬件编码器要实测**——列表里有 ≠ 能用，每个候选都真编一帧，结果缓存；
//! 3. **进度实时推**——`ffmpeg.progress` 事件（含 speed/fps/eta），不是让调用方猜。
//!
//! ```text
//! 业务插件 ──get_service("media.ffmpeg")──▶ 本插件 ──ffmpeg 子进程──▶ 文件
//!                                              │
//!                                              └─host.emit("ffmpeg.progress", {...})
//! ```

pub mod capability;
pub mod job;
pub mod plan;

use std::ffi::{c_char, c_void};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use capability::{Capabilities, probe_media};
use fuyubot_abi::{HostApi, LogLevel, Plugin};
use fuyubot_service::{
    Handler, Service, Settings, error, ok, opt_bool, opt_str, opt_u64, parse, require_str,
};
use plan::{Intent, PlanOptions, SubtitleBurn};
use serde_json::{Value, json};

const SERVICE: &str = "media.ffmpeg";
const PLUGIN: &str = "ffmpeg";
/// 抽字幕是同步小动作，但坏文件可能让 ffmpeg 卡住，给个上限。
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(120);
/// 分段是 `-c copy`，正常几十秒；给足余量避免大文件误判超时。
const SPLIT_TIMEOUT: Duration = Duration::from_secs(600);

/// 启动配置：
///
/// ```json
/// { "ffmpeg": "D:/runtime/ffmpeg/ffmpeg.exe", "probe_on_start": true }
/// ```
#[derive(Debug, Clone, Default)]
struct Config {
    binary: Option<PathBuf>,
    probe_on_start: bool,
}

static SETTINGS: Settings<Config> = Settings::new();
static CAPS: OnceLock<Mutex<Option<Capabilities>>> = OnceLock::new();

struct FfmpegPlugin {
    host: HostApi,
}

impl Plugin for FfmpegPlugin {
    fn init(host: HostApi) -> Result<Self, String> {
        Service::new(
            SERVICE,
            vec![
                ("ping", svc_ping as Handler),
                ("capabilities", svc_capabilities as Handler),
                ("probe", svc_probe as Handler),
                ("plan", svc_plan as Handler),
                ("transcode", svc_transcode as Handler),
                ("extract_subtitle", svc_extract_subtitle as Handler),
    ("split", svc_split as Handler),
                ("job_status", svc_job_status as Handler),
                ("job_list", svc_job_list as Handler),
                ("job_cancel", svc_job_cancel as Handler),
                ("prune", svc_prune as Handler),
            ],
        )
        .register(&host)?;
        let _ = HOST.set(host.clone());
        Ok(Self { host })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        let raw: Value = serde_json::from_str(config_json).unwrap_or(Value::Null);
        let config = Config {
            binary: opt_str(&raw, "ffmpeg").map(PathBuf::from),
            probe_on_start: opt_bool(&raw, "probe_on_start").unwrap_or(true),
        };
        SETTINGS.set(config.clone());

        if config.probe_on_start {
            match warm_capabilities() {
                Ok(caps) => {
                    let hardware = caps.to_json()["encoders"]["hardware"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(Value::as_str)
                                .filter(|name| caps.probed_true(name))
                                .count()
                        })
                        .unwrap_or(0);
                    self.host.log(
                        LogLevel::Info,
                        &format!(
                            "[{PLUGIN}] {}｜硬件编码器 {hardware} 个实测可用",
                            caps.version
                        ),
                    );
                }
                Err(err) => self.host.log(
                    LogLevel::Warn,
                    &format!("[{PLUGIN}] 探测失败：{err}（转码时会重试）"),
                ),
            }
        }
        Ok(())
    }
}

fuyubot_abi::define_plugin!(FfmpegPlugin, "fuyubot.ffmpeg");

fn caps_slot() -> &'static Mutex<Option<Capabilities>> {
    CAPS.get_or_init(|| Mutex::new(None))
}

/// 探测一次并放进槽位。已有就复用。
fn warm_capabilities() -> Result<Capabilities, String> {
    {
        let guard = caps_slot()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(caps) = guard.as_ref() {
            return Ok(caps.clone());
        }
    }
    let config = SETTINGS.get().unwrap_or_default();
    let caps = Capabilities::detect(config.binary)?;
    let mut guard = caps_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(caps.clone());
    Ok(caps)
}

fn with_caps<T>(run: impl FnOnce(&mut Capabilities) -> Result<T, String>) -> Result<T, String> {
    if caps_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_none()
    {
        warm_capabilities()?;
    }
    let mut guard = caps_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let caps = guard
        .as_mut()
        .ok_or_else(|| "ffmpeg 能力未就绪".to_owned())?;
    run(caps)
}

// ─── 方法实现 ──────────────────────────────────────────────────────────────

unsafe extern "C" fn svc_ping(_ctx: *mut c_void, _input: *const c_char) -> *const c_char {
    let result = with_caps(|caps| {
        Ok(json!({
            "version": caps.version,
            "ffmpeg": caps.ffmpeg.display().to_string(),
            "jobs": job::list().len(),
        }))
    });
    reply_result(result)
}

unsafe extern "C" fn svc_capabilities(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    let result = (|| -> Result<Value, String> {
        let parsed = parse(input)?;
        let deep = opt_bool(&parsed, "probe_hardware").unwrap_or(false);
        with_caps(|caps| {
            let mut payload = caps.to_json();
            if deep {
                // 深探测：只测硬件编码器（软件编码器实测意义不大，还慢）。
                let candidates: Vec<String> = payload["encoders"]["hardware"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                for encoder in candidates {
                    caps.works(&encoder);
                }
                payload["probed"] = json!(caps.probed_map());
            }
            Ok(payload)
        })
    })();
    reply_result(result)
}

unsafe extern "C" fn svc_probe(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    let result = (|| -> Result<Value, String> {
        let parsed = parse(input)?;
        let input_path = require_str(&parsed, "input")?.to_owned();
        with_caps(|caps| {
            let info = probe_media(
                caps.ffprobe.as_deref(),
                &caps.ffmpeg,
                std::path::Path::new(&input_path),
            )?;
            Ok(info.to_json())
        })
    })();
    reply_result(result)
}

unsafe extern "C" fn svc_plan(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    let result = (|| -> Result<Value, String> {
        let parsed = parse(input)?;
        let input_path = require_str(&parsed, "input")?.to_owned();
        let output_path = require_str(&parsed, "output")?.to_owned();
        let opts = plan_options(&parsed);
        with_caps(|caps| {
            let media = probe_media(
                caps.ffprobe.as_deref(),
                &caps.ffmpeg,
                std::path::Path::new(&input_path),
            )?;
            let plan = plan::build(&media, &input_path, &output_path, &opts, caps);
            Ok(json!({ "plan": plan.to_json(), "media": media.to_json() }))
        })
    })();
    reply_result(result)
}

unsafe extern "C" fn svc_transcode(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    let result = (|| -> Result<Value, String> {
        let parsed = parse(input)?;
        let input_path = require_str(&parsed, "input")?.to_owned();
        let output_path = require_str(&parsed, "output")?.to_owned();
        if input_path == output_path {
            return Err("输入与输出是同一个文件，拒绝执行".to_owned());
        }
        let opts = plan_options(&parsed);
        let host = HOST
            .get()
            .cloned()
            .ok_or_else(|| "插件未初始化".to_owned())?;

        // 方案 + 时长一起算：时长是进度百分比的分母，没它就只能报「已处理多久」。
        let (plan, media) = with_caps(|caps| {
            let media = probe_media(
                caps.ffprobe.as_deref(),
                &caps.ffmpeg,
                std::path::Path::new(&input_path),
            )?;
            let plan = plan::build(&media, &input_path, &output_path, &opts, caps);
            Ok((plan, media))
        })?;

        let ffmpeg = with_caps(|caps| Ok(caps.ffmpeg.clone()))?;
        let job_id = job::start(
            host,
            ffmpeg,
            plan.clone(),
            input_path.clone(),
            output_path.clone(),
            media.duration_ms,
        );
        if let Some(rest) = job_id.strip_prefix("ERR:") {
            return Err(format!("起转码线程失败：{rest}"));
        }
        Ok(json!({
            "job_id": job_id,
            "plan": plan.to_json(),
            "duration_ms": media.duration_ms,
        }))
    })();
    reply_result(result)
}

/// 抽字幕：把内嵌字幕导出成外挂 `.ass`，给「烧录」这一步用。
///
/// 入参 `{ input, output, stream? }`，`stream` 是字幕流序号（`0:s:N` 的 N），
/// 不传就按「简体中文优先、否则第一条」自己挑。没有字幕流时返回
/// `{"extracted": false}` 而**不是报错**——调用方据此走「不烧字幕」的 compat 方案。
unsafe extern "C" fn svc_extract_subtitle(
    _ctx: *mut c_void,
    input: *const c_char,
) -> *const c_char {
    let result = (|| -> Result<Value, String> {
        let parsed = parse(input)?;
        let input_path = require_str(&parsed, "input")?.to_owned();
        let output_path = require_str(&parsed, "output")?.to_owned();
        if input_path == output_path {
            return Err("输入与输出是同一个文件，拒绝执行".to_owned());
        }
        let requested = opt_u64(&parsed, "stream").map(|stream| stream as u32);

        // 只借一下二进制路径：抽字幕要起子进程（几秒），不能一直占着能力锁，
        // 否则同时来的 plan / transcode 都得排队。
        let (ffmpeg, ffprobe) = with_caps(|caps| Ok((caps.ffmpeg.clone(), caps.ffprobe.clone())))?;
        let media = probe_media(
            ffprobe.as_deref(),
            &ffmpeg,
            std::path::Path::new(&input_path),
        )?;
        let stream = match requested {
            Some(si) => media.subtitle_streams.iter().find(|stream| stream.si == si),
            None => plan::choose_subtitle_stream(&media.subtitle_streams),
        };
        let Some(stream) = stream else {
            return Ok(json!({
                "extracted": false,
                "reason": if media.has_subtitle {
                    "指定的字幕流不存在"
                } else {
                    "没有字幕流"
                },
                "subtitle_streams": media
                    .subtitle_streams
                    .iter()
                    .map(capability::SubtitleStream::to_json)
                    .collect::<Vec<_>>(),
            }));
        };

        if let Some(parent) = std::path::Path::new(&output_path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("创建字幕目录失败（{}）：{err}", parent.display()))?;
        }

        // -map 用的是字幕流序号（si），不是 ffprobe 的全局 index。
        let map = format!("0:s:{}", stream.si);
        let args = [
            "-hide_banner",
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-i",
            input_path.as_str(),
            "-map",
            map.as_str(),
            "-c:s",
            "ass",
            output_path.as_str(),
        ];
        capability::run_ffmpeg(&ffmpeg, &args, EXTRACT_TIMEOUT)?;

        let bytes = std::fs::metadata(&output_path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        if bytes == 0 {
            return Err(format!("抽出的字幕是空文件：{output_path}"));
        }
        Ok(json!({
            "extracted": true,
            "output": output_path,
            "si": stream.si,
            "codec": stream.codec,
            "language": stream.language,
            "title": stream.title,
            "bytes": bytes,
        }))
    })();
    reply_result(result)
}

/// 按大小分段（TG 上传上限）。
///
/// 入参 `{ input, out_dir, max_bytes, duration_ms, min_segment_sec?, prefix? }`：
/// 用 `-c copy` 切，不重新编码（秒级完成）。返回 `{ parts: [...], segment_sec }`。
/// 调用方要自己检查每段是否仍超限：关键帧对齐会让某段略大于平均值。
unsafe extern "C" fn svc_split(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    let result = (|| -> Result<Value, String> {
        let parsed = parse(input)?;
        let input_path = require_str(&parsed, "input")?.to_owned();
        let out_dir = require_str(&parsed, "out_dir")?.to_owned();
        let max_bytes = opt_u64(&parsed, "max_bytes").unwrap_or(1_900_000_000);
        let duration_ms = opt_u64(&parsed, "duration_ms").unwrap_or(0);
        let min_sec = opt_u64(&parsed, "min_segment_sec").unwrap_or(10);
        let size = std::fs::metadata(&input_path)
            .map_err(|err| format!("读不到输入文件（{input_path}）：{err}"))?
            .len();
        let segment_sec = plan::segment_seconds(size, duration_ms, max_bytes, min_sec);

        std::fs::create_dir_all(&out_dir)
            .map_err(|err| format!("创建分段目录失败（{out_dir}）：{err}"))?;
        let stem = std::path::Path::new(&input_path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("video");
        // 前缀只留字母数字和 -_：文件名会被拼进 ffmpeg 参数。
        let safe: String = stem
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
            .collect();
        let prefix = format!("{safe}-part-");
        let pattern = std::path::Path::new(&out_dir).join(format!("{prefix}%03d.mp4"));
        let pattern_text = pattern.to_string_lossy().replace('\\', "/");

        let (ffmpeg, _ffprobe) = with_caps(|caps| Ok((caps.ffmpeg.clone(), caps.ffprobe.clone())))?;
        let segment_text = segment_sec.to_string();
        let args = [
            "-hide_banner",
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-i",
            input_path.as_str(),
            "-c",
            "copy",
            "-map",
            "0",
            "-f",
            "segment",
            "-segment_time",
            segment_text.as_str(),
            "-reset_timestamps",
            "1",
            pattern_text.as_str(),
        ];
        capability::run_ffmpeg(&ffmpeg, &args, SPLIT_TIMEOUT)?;

        let mut parts: Vec<String> = std::fs::read_dir(&out_dir)
            .map_err(|err| format!("读分段目录失败（{out_dir}）：{err}"))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(&prefix))
            })
            .map(|entry| entry.path().to_string_lossy().replace('\\', "/"))
            .collect();
        parts.sort();
        if parts.is_empty() {
            return Err("未生成任何分段文件".to_owned());
        }
        Ok(json!({ "parts": parts, "segment_sec": segment_sec }))
    })();
    reply_result(result)
}

unsafe extern "C" fn svc_job_status(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    let result = (|| -> Result<Value, String> {
        let parsed = parse(input)?;
        let id = require_str(&parsed, "id")?;
        job::get(id)
            .map(|status| json!({ "job": status }))
            .ok_or_else(|| format!("没有这个任务：{id}"))
    })();
    reply_result(result)
}

unsafe extern "C" fn svc_job_list(_ctx: *mut c_void, _input: *const c_char) -> *const c_char {
    ok(json!({ "jobs": job::list() }))
}

unsafe extern "C" fn svc_job_cancel(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    let result = (|| -> Result<Value, String> {
        let parsed = parse(input)?;
        let id = require_str(&parsed, "id")?;
        if job::cancel(id) {
            Ok(json!({ "canceled": id }))
        } else {
            Err(format!("没有这个任务：{id}"))
        }
    })();
    reply_result(result)
}

unsafe extern "C" fn svc_prune(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    let parsed = parse(input).unwrap_or(Value::Null);
    let keep = opt_u64(&parsed, "keep").unwrap_or(10) as usize;
    let removed = job::prune(keep);
    ok(json!({ "removed": removed, "kept": keep }))
}

// ─── 公共 ──────────────────────────────────────────────────────────────────

static HOST: OnceLock<HostApi> = OnceLock::new();

fn plan_options(input: &Value) -> PlanOptions {
    let output = opt_str(input, "output").unwrap_or("out.mp4");
    let target_ext = output
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_else(|| "mp4".to_owned());
    PlanOptions {
        intent: Intent::parse(opt_str(input, "intent").unwrap_or("compat")),
        crf: input
            .get("crf")
            .and_then(Value::as_u64)
            .map(|value| value.clamp(0, 51) as u32),
        preset: opt_str(input, "preset").map(str::to_owned),
        target_ext,
        keep_audio: opt_bool(input, "keep_audio").unwrap_or(true),
        subtitle: opt_str(input, "subtitle")
            .filter(|path| !path.trim().is_empty())
            .map(|path| {
                let mut burn = SubtitleBurn::new(path);
                if let Some(index) = opt_u64(input, "subtitle_stream") {
                    burn = burn.with_stream(index as u32);
                }
                burn
            }),
    }
}

fn reply_result(result: Result<Value, String>) -> *const c_char {
    match result {
        Ok(fields) => ok(fields),
        Err(message) => error(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_extension_drives_container_flags() {
        let opts = plan_options(&json!({ "output": "C:/x/a.MKV" }));
        assert_eq!(opts.target_ext, "mkv");
        let opts = plan_options(&json!({ "output": "a.mp4" }));
        assert_eq!(opts.target_ext, "mp4");
        // 没有扩展名时按 mp4 处理。
        assert_eq!(plan_options(&json!({})).target_ext, "mp4");
    }

    #[test]
    fn intent_and_crf_are_parsed() {
        let opts = plan_options(&json!({ "intent": "small", "crf": 999, "keep_audio": false }));
        assert_eq!(opts.intent, Intent::Small);
        assert_eq!(opts.crf, Some(51));
        assert!(!opts.keep_audio);
        assert_eq!(plan_options(&json!({})).intent, Intent::Compat);
    }

    #[test]
    fn subtitle_parameters_are_passed_through() {
        let opts = plan_options(&json!({
            "output": "C:/work/第01集.mp4",
            "subtitle": "C:/work/第01集-sub.ass",
            "subtitle_stream": 1,
        }));
        let burn = opts.subtitle.expect("应当解析出字幕参数");
        assert_eq!(burn.path, "C:/work/第01集-sub.ass");
        assert_eq!(burn.stream_index, Some(1));
        // 不传字幕时保持 None，方案仍然可以走 copy。
        assert!(
            plan_options(&json!({ "output": "a.mp4" }))
                .subtitle
                .is_none()
        );
        // 空白字符串不能当成路径。
        assert!(
            plan_options(&json!({ "subtitle": "   " }))
                .subtitle
                .is_none()
        );
    }
}
