//! 能力探测：这台机器上 ffmpeg 会什么。
//!
//! 「自动选最佳工具」的前提是**别信配置，先问事实**：
//!
//! 1. `ffmpeg -encoders` 能列出所有编解码器，但硬件编码器在列表里 ≠ 能跑——
//!    NVIDIA 没驱动、QSV 没设备时照样会列出来，一编码就炸；
//! 2. 所以每个候选硬件编码器都要**真编一帧**才算「可用」，结果进缓存，只测一次；
//! 3. ffprobe 用来读输入文件的真实参数（编码、时长），时长是算进度百分比的分母。

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// 一次探测的结果。
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub ffmpeg: PathBuf,
    pub ffprobe: Option<PathBuf>,
    pub version: String,
    pub encoders: BTreeSet<String>,
    pub hwaccels: BTreeSet<String>,
    /// 编码器 → 是否真的能用（真编一帧的结果，带缓存）。
    probed: HashMap<String, bool>,
}

/// 探测时给单个子进程的上限：GPU 初始化卡死时不能让插件跟着卡死。
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

impl Capabilities {
    /// 找 ffmpeg 并问清楚它会什么。`preferred` 优先（配置里显式给路径时）。
    pub fn detect(preferred: Option<PathBuf>) -> Result<Self, String> {
        let ffmpeg = match preferred {
            Some(path) if path.exists() => path,
            Some(path) => return Err(format!("配置的 ffmpeg 不存在：{}", path.display())),
            None => find_binary("ffmpeg").ok_or_else(|| {
                "找不到 ffmpeg：请放到运行时目录（与插件同级），或设置 FFMPEG_DIR/配置 binary"
                    .to_owned()
            })?,
        };

        let version = run_capture(&ffmpeg, &["-hide_banner", "-version"])
            .map(|text| text.lines().next().unwrap_or_default().trim().to_owned())
            .unwrap_or_default();

        let encoders = run_capture(&ffmpeg, &["-hide_banner", "-encoders"])
            .map(|text| parse_encoders(&text))
            .unwrap_or_default();

        let hwaccels = run_capture(&ffmpeg, &["-hide_banner", "-hwaccels"])
            .map(|text| parse_hwaccels(&text))
            .unwrap_or_default();

        Ok(Self {
            ffprobe: find_binary("ffprobe"),
            ffmpeg,
            version,
            encoders,
            hwaccels,
            probed: HashMap::new(),
        })
    }

    pub fn has(&self, encoder: &str) -> bool {
        self.encoders.contains(encoder)
    }

    /// 编码器是否真能跑：列表里有 + 真编一帧成功。结果缓存，每个只测一次。
    pub fn works(&mut self, encoder: &str) -> bool {
        if let Some(known) = self.probed.get(encoder) {
            return *known;
        }
        if !self.has(encoder) {
            self.probed.insert(encoder.to_owned(), false);
            return false;
        }
        let usable = self.probe_encoder(encoder);
        self.probed.insert(encoder.to_owned(), usable);
        usable
    }

    /// 用 0.1 秒的纯色测试源编一帧，能退出 0 就认为可用。
    ///
    /// 这一步看着多余，实际上是整套「自动选择」里最值钱的一步：
    /// 少了它，`h264_nvenc` 在没装驱动的机器上会把每一次转码都变成失败重试。
    fn probe_encoder(&self, encoder: &str) -> bool {
        let args = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=256x144:d=0.1",
            "-frames:v",
            "1",
            "-c:v",
            encoder,
            "-f",
            "null",
            "-",
        ];
        run_with_timeout(&self.ffmpeg, &args, PROBE_TIMEOUT)
            .map(|status| status.success())
            .unwrap_or(false)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "ffmpeg": self.ffmpeg.display().to_string(),
            "ffprobe": self.ffprobe.as_ref().map(|p| p.display().to_string()),
            "version": self.version,
            "hwaccels": self.hwaccels,
            "encoders": {
                "hardware": self.encoders.iter().filter(|e| is_hw_encoder(e)).collect::<Vec<_>>(),
                "software": self.encoders.iter().filter(|e| is_sw_encoder(e)).collect::<Vec<_>>(),
            },
            "probed": self.probed,
        })
    }

    /// 只读缓存：不触发新的探测。给启动日志统计用。
    pub fn probed_true(&self, encoder: &str) -> bool {
        self.probed.get(encoder).copied().unwrap_or(false)
    }

    pub fn probed_map(&self) -> HashMap<String, bool> {
        self.probed.clone()
    }
}

#[cfg(test)]
impl Capabilities {
    /// 测试用：假定列出的编码器都「实测可用」，避免单测真的去编视频。
    pub(crate) fn from_encoders_for_test(encoders: &[&str]) -> Self {
        let set: BTreeSet<String> = encoders.iter().map(|name| (*name).to_owned()).collect();
        let probed = set.iter().map(|name| (name.clone(), true)).collect();
        Self {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: None,
            version: "test".to_owned(),
            encoders: set,
            hwaccels: BTreeSet::new(),
            probed,
        }
    }
}

// ─── 输入文件信息 ──────────────────────────────────────────────────────────

/// 转码要用到的输入参数。
#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub duration_ms: i64,
    pub video_codec: String,
    pub audio_codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub bitrate_kbps: u64,
    pub container: String,
    pub has_audio: bool,
    pub has_subtitle: bool,
}

impl MediaInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "duration_ms": self.duration_ms,
            "video_codec": self.video_codec,
            "audio_codec": self.audio_codec,
            "width": self.width,
            "height": self.height,
            "fps": self.fps,
            "bitrate_kbps": self.bitrate_kbps,
            "container": self.container,
            "has_audio": self.has_audio,
            "has_subtitle": self.has_subtitle,
        })
    }

    /// MP4 能直接放的编码组合——满足就不必重编码，`-c copy` 秒出。
    pub fn is_mp4_ready(&self) -> bool {
        let video_ok = matches!(self.video_codec.as_str(), "h264" | "hevc" | "av1");
        let audio_ok = !self.has_audio || matches!(self.audio_codec.as_str(), "aac" | "mp3");
        video_ok && audio_ok
    }
}

/// 用 ffprobe 读输入。没有 ffprobe 时退化成「解析 ffmpeg -i 的 stderr」。
pub fn probe_media(
    ffprobe: Option<&Path>,
    ffmpeg: &Path,
    input: &Path,
) -> Result<MediaInfo, String> {
    if let Some(ffprobe) = ffprobe
        && let Ok(text) = run_capture(
            ffprobe,
            &[
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
            ]
            .iter()
            .map(|arg| *arg)
            .chain(std::iter::once(
                input.to_str().ok_or("输入路径不是合法 UTF-8")?,
            ))
            .collect::<Vec<_>>(),
        )
        && let Ok(value) = serde_json::from_str::<Value>(&text)
    {
        return Ok(parse_probe_json(&value));
    }
    // 退化路径：`ffmpeg -i` 的信息在 stderr，且退出码非 0（没有输出文件）。
    let text = run_capture_stderr(
        ffmpeg,
        &["-hide_banner", "-i", input.to_str().ok_or("路径非 UTF-8")?],
    )
    .unwrap_or_default();
    Ok(parse_ffmpeg_info(&text))
}

fn parse_probe_json(value: &Value) -> MediaInfo {
    let mut info = MediaInfo::default();
    if let Some(format) = value.get("format") {
        info.duration_ms = format
            .get("duration")
            .and_then(Value::as_str)
            .and_then(|text| text.parse::<f64>().ok())
            .map(|secs| (secs * 1000.0) as i64)
            .unwrap_or(0);
        info.bitrate_kbps = format
            .get("bit_rate")
            .and_then(Value::as_str)
            .and_then(|text| text.parse::<u64>().ok())
            .map(|bps| bps / 1000)
            .unwrap_or(0);
        info.container = format
            .get("format_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
    }
    if let Some(streams) = value.get("streams").and_then(Value::as_array) {
        for stream in streams {
            match stream.get("codec_type").and_then(Value::as_str) {
                Some("video") if info.video_codec.is_empty() => {
                    info.video_codec = stream
                        .get("codec_name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    info.width = stream.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                    info.height = stream.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                    info.fps = stream
                        .get("avg_frame_rate")
                        .and_then(Value::as_str)
                        .and_then(parse_rational)
                        .unwrap_or(0.0);
                }
                Some("audio") if info.audio_codec.is_empty() => {
                    info.audio_codec = stream
                        .get("codec_name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    info.has_audio = true;
                }
                Some("subtitle") => info.has_subtitle = true,
                _ => {}
            }
        }
    }
    info
}

fn parse_rational(text: &str) -> Option<f32> {
    let (num, den) = text.split_once('/')?;
    let num: f32 = num.parse().ok()?;
    let den: f32 = den.parse().ok()?;
    if den == 0.0 { None } else { Some(num / den) }
}

/// 退化解析：从 `Duration: 00:24:00.04` 和 `Stream #0:0 ... Video: h264` 里抠。
fn parse_ffmpeg_info(text: &str) -> MediaInfo {
    let mut info = MediaInfo::default();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Duration:") {
            let stamp = rest.split(',').next().unwrap_or_default().trim();
            info.duration_ms = parse_timecode(stamp);
        } else if line.contains("Video:") && info.video_codec.is_empty() {
            info.video_codec = codec_after(line, "Video:");
            info.width = dimension_after(line, 'x').0;
            info.height = dimension_after(line, 'x').1;
        } else if line.contains("Audio:") && !info.has_audio {
            info.audio_codec = codec_after(line, "Audio:");
            info.has_audio = true;
        }
    }
    info
}

fn codec_after(line: &str, marker: &str) -> String {
    line.split_once(marker)
        .map(|(_, rest)| {
            rest.split([' ', ','])
                .find(|token| !token.is_empty() && !token.contains('('))
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default()
}

fn dimension_after(line: &str, sep: char) -> (u32, u32) {
    // 找形如 `1920x1080` 的片段。
    for token in line.split_whitespace() {
        if let Some((w, h)) = token.split_once(sep)
            && let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.trim_end_matches(',').parse::<u32>())
        {
            return (w, h);
        }
    }
    (0, 0)
}

/// `HH:MM:SS.xx` → 毫秒。
pub fn parse_timecode(text: &str) -> i64 {
    let mut parts = text.trim().split(':').rev();
    let seconds: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let minutes: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let hours: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    ((hours * 3600.0 + minutes * 60.0 + seconds) * 1000.0) as i64
}

// ─── 解析器 ────────────────────────────────────────────────────────────────

/// `-encoders` 的每一行形如 ` V....D h264_nvenc  NVIDIA NVENC H.264 encoder`。
fn parse_encoders(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(flags) = parts.next() else { continue };
        // 只认形如 `V.....` / `A....D` 的标志列，避免把标题行也吃进去。
        if !flags.starts_with('V') && !flags.starts_with('A') && !flags.starts_with('S') {
            continue;
        }
        if let Some(name) = parts.next() {
            // 表头 `V..... = Video` 里的 `=` 不是编码器名；名字只允许字母数字下划线连字符。
            if name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
            {
                out.insert(name.to_owned());
            }
        }
    }
    out
}

/// `-hwaccels` 输出第一行是 `Hardware acceleration methods:`，后面一行一个。
fn parse_hwaccels(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && !line.starts_with("Hardware") && *line != "ffmpeg version"
        })
        .map(|line| line.to_owned())
        .collect()
}

pub fn is_hw_encoder(name: &str) -> bool {
    const SUFFIXES: [&str; 6] = ["_nvenc", "_qsv", "_amf", "_videotoolbox", "_vaapi", "_mf"];
    SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

pub fn is_sw_encoder(name: &str) -> bool {
    matches!(
        name,
        "libx264" | "libx265" | "libsvtav1" | "libaom-av1" | "libvpx-vp9" | "libopenh264"
    )
}

// ─── 进程工具 ──────────────────────────────────────────────────────────────

/// 找可执行文件：`FFMPEG_DIR` → 当前 exe 目录 → PATH。
pub fn find_binary(name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("FFMPEG_DIR") {
        candidates.push(PathBuf::from(dir).join(&file));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(&file));
        candidates.push(dir.join("ffmpeg").join(&file));
        candidates.push(dir.join("../ffmpeg").join(&file));
    }
    candidates.extend(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join(&file)),
    );
    candidates.into_iter().find(|path| path.is_file())
}

fn run_capture(binary: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("执行 {} 失败：{err}", binary.display()))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_capture_stderr(binary: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("执行 {} 失败：{err}", binary.display()))?;
    Ok(String::from_utf8_lossy(&output.stderr).into_owned())
}

/// 带超时地跑一个进程：超时就 kill，避免 GPU 初始化卡死拖住探测。
fn run_with_timeout(
    binary: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("启动 {} 失败：{err}", binary.display()))?;

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{} 超过 {}s 未返回，已终止",
                        binary.display(),
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(format!("等待进程失败：{err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENCODERS: &str = "\
Encoders:
 V..... = Video
 ------
 V....D libx264              libx264 H.264 (codec h264)
 V....D libx265              libx265 H.265 (codec hevc)
 V....D h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)
 V....D hevc_qsv             HEVC (Intel Quick Sync Video) (codec hevc)
 A....D aac                  AAC (Advanced Audio Coding)
";

    #[test]
    fn encoder_table_is_parsed() {
        let encoders = parse_encoders(ENCODERS);
        assert!(encoders.contains("libx264"));
        assert!(encoders.contains("h264_nvenc"));
        assert!(encoders.contains("aac"));
        // 标题行不能被当成编码器。
        assert!(!encoders.contains("Encoders:"));
        assert!(!encoders.contains("="));
    }

    #[test]
    fn hw_and_sw_encoders_are_classified() {
        assert!(is_hw_encoder("h264_nvenc"));
        assert!(is_hw_encoder("hevc_videotoolbox"));
        assert!(!is_hw_encoder("libx264"));
        assert!(is_sw_encoder("libx264"));
        assert!(!is_sw_encoder("h264_nvenc"));
    }

    #[test]
    fn timecode_parsing_matches_progress_format() {
        assert_eq!(parse_timecode("00:00:01.50"), 1500);
        assert_eq!(parse_timecode("01:02:03.04"), 3_723_040);
        assert_eq!(parse_timecode("00:24:00"), 1_440_000);
    }

    #[test]
    fn probe_json_maps_to_media_info() {
        let value: Value = serde_json::from_str(
            r#"{
              "format": { "duration": "1440.5", "bit_rate": "4000000", "format_name": "matroska,webm" },
              "streams": [
                { "codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080,
                  "avg_frame_rate": "24000/1001" },
                { "codec_type": "audio", "codec_name": "aac" },
                { "codec_type": "subtitle", "codec_name": "ass" }
              ]
            }"#,
        )
        .unwrap();
        let info = parse_probe_json(&value);
        assert_eq!(info.duration_ms, 1_440_500);
        assert_eq!(info.video_codec, "h264");
        assert_eq!(info.audio_codec, "aac");
        assert!(info.has_audio && info.has_subtitle);
        assert!((info.fps - 23.976).abs() < 0.01);
        assert!(info.is_mp4_ready());
    }

    #[test]
    fn mp4_ready_rejects_incompatible_audio() {
        let info = MediaInfo {
            video_codec: "h264".to_owned(),
            audio_codec: "flac".to_owned(),
            has_audio: true,
            ..Default::default()
        };
        assert!(!info.is_mp4_ready());
    }

    #[test]
    fn ffmpeg_info_fallback_parses_duration_and_codec() {
        let text = "\
  Duration: 00:24:00.04, start: 0.000000, bitrate: 4000 kb/s
    Stream #0:0: Video: h264 (High), yuv420p, 1920x1080, 23.98 fps
    Stream #0:1: Audio: aac (LC), 48000 Hz, stereo
";
        let info = parse_ffmpeg_info(text);
        assert_eq!(info.duration_ms, 1_440_040);
        assert_eq!(info.video_codec, "h264");
        assert_eq!((info.width, info.height), (1920, 1080));
        assert_eq!(info.audio_codec, "aac");
    }
}
