//! 转码方案选择：同一个输入，不同意图走不同的路。
//!
//! 判断顺序是**从便宜到贵**：
//!
//! 1. **能不转就不转**——目标容器能直接装下现有编码就 `-c copy`，几秒完成；
//! 2. **能用硬件就用硬件**——`nvenc` / `qsv` / `amf` / `videotoolbox`，但必须先通过
//!    真编一帧的探测（见 [`Capabilities::works`]）；
//! 3. **最后才是软件编码**——`libx264` 兼容性最好，`libx265` 体积最小。
//!
//! 选中的方案会带一句 `reason`，出问题时日志里能直接看出「为什么选它」，
//! 不用回头读代码。

use crate::capability::{Capabilities, MediaInfo, SubtitleStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// 只换容器，画质一点不损失（默认）。
    Remux,
    /// 兼容优先：h264，什么设备都能播。
    Compat,
    /// 体积优先：hevc。
    Small,
    /// 速度优先：只考虑硬件编码器。
    Fast,
}

impl Intent {
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "remux" | "copy" => Intent::Remux,
            "small" | "hevc" | "compress" => Intent::Small,
            "fast" | "speed" => Intent::Fast,
            _ => Intent::Compat,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Intent::Remux => "remux",
            Intent::Compat => "compat",
            Intent::Small => "small",
            Intent::Fast => "fast",
        }
    }
}

/// 要烧进画面的字幕：通常是先抽出来的外挂 `.ass` / `.srt`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleBurn {
    /// 外挂字幕文件路径，Windows 的盘符/反斜杠都行，转义交给本模块。
    pub path: String,
    /// 字幕文件内部的流序号，单流文件就是 `0`；多流时才需要 `si=`。
    pub stream_index: Option<u32>,
}

impl SubtitleBurn {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            stream_index: None,
        }
    }

    pub fn with_stream(mut self, index: u32) -> Self {
        self.stream_index = Some(index);
        self
    }
}

#[derive(Debug, Clone)]
pub struct PlanOptions {
    pub intent: Intent,
    pub crf: Option<u32>,
    pub preset: Option<String>,
    pub target_ext: String,
    pub keep_audio: bool,
    /// 要烧录的字幕。`Some` 时**强制重编码**：`-c copy` 没法过滤镜。
    pub subtitle: Option<SubtitleBurn>,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            intent: Intent::Compat,
            crf: None,
            preset: None,
            target_ext: "mp4".to_owned(),
            keep_audio: true,
            subtitle: None,
        }
    }
}

/// 这条字幕是不是简体中文。语言和标题都看：MKV 的 `language` 经常是 `und`、
/// 甚至干脆没写，标题里反而标了「简体 / 简中 / CHS」。
///
/// 繁体标记一票否决：语言只写 `zh` 时，标题里的「繁 / CHT / Hant」比语言码更可信。
pub fn is_simplified_chinese(stream: &SubtitleStream) -> bool {
    let language = stream.language.trim().to_ascii_lowercase();
    let title = stream.title.trim().to_lowercase();
    if language.contains("hant")
        || language == "cht"
        || title.contains('繁')
        || title.contains("cht")
        || title.contains("hant")
        || title.contains("traditional")
    {
        return false;
    }
    matches!(
        language.as_str(),
        "chi" | "zho" | "zh" | "chs" | "zh-hans" | "zh-cn" | "zh-sg" | "zh-hans-cn"
    ) || language.starts_with("zh-hans")
        || title.contains('简')
        || title.contains("chi")
        || title.contains("zh")
        || title.contains("chs")
        || title.contains("simplified")
}

/// 挑要烧的那条字幕：优先简体中文，没有就第一条。
///
/// 返回的是 `si`（字幕流内的序号），因为 `-map 0:s:N` 和滤镜的 `si=`
/// 都是按「第几条字幕」数，而不是 ffprobe 的全局 `index`。
pub fn choose_subtitle_stream(streams: &[SubtitleStream]) -> Option<&SubtitleStream> {
    streams
        .iter()
        .find(|stream| is_simplified_chinese(stream))
        .or_else(|| streams.first())
}

/// 把字幕路径转成能塞进 `subtitles=` 滤镜的样子。
///
/// ffmpeg 的滤镜描述要过**两层**解析，坑都在这里（ffmpeg 7.1 实测）：
///
/// - **盘符冒号** `C:` 会被当成滤镜选项分隔符 → 写成 `C\:`；
/// - **反斜杠** 在滤镜里是转义前缀，和 Windows 路径天然打架 → 统一换成 `/`
///   （ffmpeg 在 Windows 上认正斜杠，省掉一层反斜杠转义）；
/// - **单引号** 会把后面的整段路径吞掉 → 落成「关引号 + 三个反斜杠 + 引号 +
///   重新开引号」。三个反斜杠是因为第一层解析吃掉两个、留下 `\'`，
///   第二层再把它还原成 `'`；少一个就会得到一个被悄悄丢掉的引号。
/// 按大小上限算分段时长（秒）。
///
/// 对齐 TS `splitVideoByMaxSize`：按平均码率估算，留 2% 余量，最少 10 秒。
/// 真正切的时候可能因为关键帧对齐而略超，调用方要检查每段大小。
pub fn segment_seconds(size_bytes: u64, duration_ms: u64, max_bytes: u64, min_sec: u64) -> u64 {
    if duration_ms == 0 || size_bytes == 0 || max_bytes == 0 {
        return min_sec.max(1);
    }
    let duration_sec = (duration_ms as f64 / 1000.0).max(1.0);
    let avg_bytes_per_sec = size_bytes as f64 / duration_sec;
    if avg_bytes_per_sec <= 0.0 {
        return min_sec.max(1);
    }
    let budget = (max_bytes as f64) * 0.98;
    let seconds = (budget / avg_bytes_per_sec).floor() as u64;
    seconds.max(min_sec.max(1))
}

pub fn escape_subtitles_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len() + 8);
    for ch in path.chars() {
        match ch {
            '\\' => escaped.push('/'),
            ':' => escaped.push_str("\\:"),
            '\'' => escaped.push_str("'\\\\\\''"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// 组装 `subtitles='…'[:si=N]`。
pub fn subtitles_filter(burn: &SubtitleBurn) -> String {
    let mut filter = format!("subtitles='{}'", escape_subtitles_path(&burn.path));
    if let Some(index) = burn.stream_index {
        filter.push_str(&format!(":si={index}"));
    }
    filter
}

#[derive(Debug, Clone)]
pub struct Plan {
    /// `copy` 或 `encode`。
    pub mode: &'static str,
    pub encoder: String,
    pub hardware: bool,
    /// 完整参数（含 `-i 输入` 与输出路径）。
    pub args: Vec<String>,
    pub reason: String,
}

impl Plan {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "mode": self.mode,
            "encoder": self.encoder,
            "hardware": self.hardware,
            "reason": self.reason,
            "args": self.args,
        })
    }
}

/// 候选编码器：顺序就是优先级。
const H264_CANDIDATES: [(&str, &str); 5] = [
    ("h264_nvenc", "NVIDIA"),
    ("h264_qsv", "Intel QSV"),
    ("h264_amf", "AMD AMF"),
    ("h264_videotoolbox", "Apple VideoToolbox"),
    ("libx264", "软件编码"),
];

const HEVC_CANDIDATES: [(&str, &str); 6] = [
    ("hevc_nvenc", "NVIDIA"),
    ("hevc_qsv", "Intel QSV"),
    ("hevc_amf", "AMD AMF"),
    ("hevc_videotoolbox", "Apple VideoToolbox"),
    ("libx265", "软件编码"),
    ("libx264", "软件编码（无 x265 时降级）"),
];

/// 给一个输入挑方案。`caps` 需要 `&mut` 是因为硬件探测结果要写进缓存。
pub fn build(
    media: &MediaInfo,
    input: &str,
    output: &str,
    opts: &PlanOptions,
    caps: &mut Capabilities,
) -> Plan {
    // ① 免转：容器能装下现有编码，且用户没有强制重编码。
    //    要烧字幕时不能走这条路：滤镜必须重编码，`-c copy` + `-vf` 会直接失败。
    if opts.subtitle.is_none()
        && opts.intent != Intent::Fast
        && media.is_mp4_ready()
        && !needs_reencode_for(opts.intent, media)
    {
        let mut args = base_args(input);
        args.push("-map".into());
        args.push("0:v:0".into());
        if media.has_audio {
            args.push("-map".into());
            args.push("0:a:0".into());
        }
        args.push("-c".into());
        args.push("copy".into());
        push_container_flags(&mut args, &opts.target_ext, &media.video_codec);
        args.push(output.to_owned());
        return Plan {
            mode: "copy",
            encoder: String::new(),
            hardware: false,
            args,
            reason: format!(
                "输入已是 {} + {}，容器 {} 可直接装下 → 无转码直出（画质零损失，速度最快）",
                media.video_codec,
                if media.has_audio {
                    media.audio_codec.as_str()
                } else {
                    "无音轨"
                },
                opts.target_ext
            ),
        };
    }

    // ② 选编码器：硬件优先，软件兜底。
    let candidates: &[(&str, &str)] = if opts.intent == Intent::Small {
        &HEVC_CANDIDATES
    } else {
        &H264_CANDIDATES
    };
    let mut chosen: Option<(&str, &str, bool)> = None;
    for (encoder, vendor) in candidates {
        let hardware = crate::capability::is_hw_encoder(encoder);
        if opts.intent == Intent::Fast && !hardware {
            continue;
        }
        if caps.works(encoder) {
            chosen = Some((encoder, vendor, hardware));
            break;
        }
    }
    let (encoder, vendor, hardware) = chosen.unwrap_or(("libx264", "软件编码", false));

    let reason = if hardware {
        format!("{vendor} 硬件编码器 {encoder} 实测可用，优先于软件编码")
    } else if opts.intent == Intent::Fast {
        format!("要求速度优先但无可用硬件编码器，回退 {encoder}")
    } else if caps.has(encoder) {
        format!("无可用硬件编码器，回退 {encoder}（{vendor}）")
    } else {
        format!("{encoder} 不可用，仍按默认参数尝试（大概率会失败，请检查 ffmpeg 构建）")
    };

    let mut args = base_args(input);
    args.push("-map".into());
    args.push("0:v:0".into());
    args.push("-c:v".into());
    args.push(encoder.to_owned());
    push_video_quality(&mut args, encoder, opts);

    // 音频：源是 aac/mp3 就直通，否则转 aac——重编码音频既慢又没意义。
    if opts.keep_audio && media.has_audio {
        args.push("-map".into());
        args.push("0:a:0".into());
        if matches!(media.audio_codec.as_str(), "aac" | "mp3") {
            args.push("-c:a".into());
            args.push("copy".into());
        } else {
            args.push("-c:a".into());
            args.push("aac".into());
            args.push("-b:a".into());
            args.push("160k".into());
        }
    } else {
        args.push("-an".into());
    }

    // 烧字幕：`-vf` 挂在视频输出上，上面已经显式 `-map 0:v:0`。
    if let Some(burn) = &opts.subtitle {
        args.push("-vf".into());
        args.push(subtitles_filter(burn));
    }

    push_container_flags(&mut args, &opts.target_ext, encoder);
    args.push(output.to_owned());

    let subtitle_note = opts
        .subtitle
        .as_ref()
        .map(|burn| format!("｜烧录字幕 {}", file_name_of(&burn.path)))
        .unwrap_or_default();

    Plan {
        mode: "encode",
        encoder: encoder.to_owned(),
        hardware,
        args,
        reason: format!("[{}] {reason}{subtitle_note}", opts.intent.as_str()),
    }
}

/// 只取文件名部分，日志里不用把整条 Windows 路径刷出来。
fn file_name_of(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

/// `Remux` 意图下，即使容器能装也允许直通（它的存在就是「只要换个壳」）。
fn needs_reencode_for(intent: Intent, media: &MediaInfo) -> bool {
    match intent {
        Intent::Small => !matches!(media.video_codec.as_str(), "hevc"),
        Intent::Remux | Intent::Compat | Intent::Fast => false,
    }
}

fn base_args(input: &str) -> Vec<String> {
    vec![
        "-hide_banner".to_owned(),
        // 服务进程里没有交互终端，-nostdin 防止 ffmpeg 抢走 stdin。
        "-nostdin".to_owned(),
        "-y".to_owned(),
        "-i".to_owned(),
        input.to_owned(),
    ]
}

fn push_video_quality(args: &mut Vec<String>, encoder: &str, opts: &PlanOptions) {
    let crf = opts.crf.unwrap_or(match encoder {
        "libx265" => 24,
        "libx264" => 20,
        _ => 23,
    });
    match encoder {
        // NVENC：VBR + CQ，`-b:v 0` 表示不设目标码率，纯按质量走。
        "h264_nvenc" | "hevc_nvenc" => {
            args.extend(["-preset", "p5", "-rc", "vbr", "-cq"].map(str::to_owned));
            args.push(crf.to_string());
            args.push("-b:v".into());
            args.push("0".into());
        }
        // QSV：用 global_quality（对应 crf 语义），开 look_ahead 提升质量。
        "h264_qsv" | "hevc_qsv" => {
            args.push("-global_quality".into());
            args.push(crf.to_string());
            args.push("-look_ahead".into());
            args.push("1".into());
        }
        // AMF：CQP 模式，I/P 帧量化参数分开给。
        "h264_amf" | "hevc_amf" => {
            args.extend(["-rc", "cqp", "-qp_i", "-qp_p"].map(String::from));
            args.push((crf + 2).to_string());
            args.push(crf.to_string());
        }
        "h264_videotoolbox" | "hevc_videotoolbox" => {
            args.push("-q:v".into());
            args.push((100_i64 - crf as i64 * 2).clamp(1, 100).to_string());
        }
        _ => {
            args.push("-crf".into());
            args.push(crf.to_string());
            args.push("-preset".into());
            args.push(opts.preset.clone().unwrap_or_else(|| {
                if encoder == "libx265" {
                    "medium"
                } else {
                    "slow"
                }
                .to_owned()
            }));
        }
    }
}

fn push_container_flags(args: &mut Vec<String>, ext: &str, video_tag_source: &str) {
    match ext {
        "mp4" | "m4v" => {
            // 首帧可播：边下边播的场景必须有。
            args.push("-movflags".into());
            args.push("+faststart".into());
            // hevc 在 mp4 里要打 hvc1 tag，否则 Apple 设备不认。
            if video_tag_source.contains("hevc") || video_tag_source.contains("265") {
                args.push("-tag:v".into());
                args.push("hvc1".into());
            }
        }
        "mkv" => {
            args.push("-c:s".into());
            args.push("copy".into());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(codec: &str, audio: &str) -> MediaInfo {
        MediaInfo {
            duration_ms: 1_440_000,
            video_codec: codec.to_owned(),
            audio_codec: audio.to_owned(),
            has_audio: !audio.is_empty(),
            ..Default::default()
        }
    }

    fn caps_with(encoders: &[&str]) -> Capabilities {
        Capabilities::from_encoders_for_test(encoders)
    }

    #[test]
    fn segment_seconds_respects_size_budget() {
        // 2GB / 24 分钟 ≈ 1.39MB/s；1.9GB 的 98% 预算 ≈ 1340 秒。
        let seconds = segment_seconds(2_000_000_000, 24 * 60 * 1000, 1_900_000_000, 10);
        assert!((1200..=1400).contains(&seconds), "{seconds}");
    }

    #[test]
    fn segment_seconds_falls_back_to_minimum() {
        assert_eq!(segment_seconds(100, 0, 1000, 10), 10);
        assert_eq!(segment_seconds(0, 1000, 1000, 10), 10);
        // 预算比一分钟的码率还小 → 退到最小时长。
        assert_eq!(segment_seconds(1_000_000, 1_000_000, 1_000, 10), 10);
    }
    #[test]
    fn compatible_input_is_remuxed_without_reencoding() {
        let mut caps = caps_with(&["libx264", "libx265"]);
        let plan = build(
            &media("h264", "aac"),
            "in.mkv",
            "out.mp4",
            &PlanOptions::default(),
            &mut caps,
        );
        assert_eq!(plan.mode, "copy");
        assert!(plan.args.contains(&"copy".to_owned()));
        assert!(plan.reason.contains("无转码直出"));
        assert_eq!(plan.args.last().unwrap(), "out.mp4");
    }

    #[test]
    fn hardware_encoder_wins_when_it_actually_works() {
        let mut caps = caps_with(&["h264_nvenc", "libx264"]);
        let plan = build(
            &media("hevc", "flac"),
            "in.mkv",
            "out.mp4",
            &PlanOptions::default(),
            &mut caps,
        );
        assert_eq!(plan.mode, "encode");
        assert_eq!(plan.encoder, "h264_nvenc");
        assert!(plan.hardware);
        assert!(plan.args.iter().any(|arg| arg == "-cq"));
    }

    #[test]
    fn fast_intent_skips_software_encoders() {
        let mut caps = caps_with(&["libx264"]);
        let opts = PlanOptions {
            intent: Intent::Fast,
            ..Default::default()
        };
        let plan = build(&media("hevc", "aac"), "in.mkv", "out.mp4", &opts, &mut caps);
        // 没有硬件编码器时仍然要给出可跑的方案，只是 reason 里说明降级了。
        assert_eq!(plan.encoder, "libx264");
        assert!(plan.reason.contains("回退"), "{}", plan.reason);
    }

    #[test]
    fn small_intent_prefers_hevc() {
        let mut caps = caps_with(&["libx264", "libx265"]);
        let opts = PlanOptions {
            intent: Intent::Small,
            ..Default::default()
        };
        let plan = build(&media("h264", "aac"), "in.mkv", "out.mp4", &opts, &mut caps);
        assert_eq!(plan.encoder, "libx265");
        assert!(
            plan.args.iter().any(|arg| arg == "hvc1"),
            "hevc 要打 hvc1 tag"
        );
    }

    #[test]
    fn audio_is_copied_when_already_compatible() {
        let mut caps = caps_with(&["libx264"]);
        // 视频必须重编码（mpeg2 → h264），音频本身兼容，应直通。
        let plan = build(
            &media("mpeg2video", "aac"),
            "in.ts",
            "out.mp4",
            &PlanOptions::default(),
            &mut caps,
        );
        assert_eq!(plan.mode, "encode");
        let text = plan.args.join(" ");
        assert!(text.contains("-c:a copy"), "{text}");
    }

    #[test]
    fn incompatible_audio_is_transcoded() {
        let mut caps = caps_with(&["libx264"]);
        let plan = build(
            &media("hevc", "flac"),
            "in.mkv",
            "out.mp4",
            &PlanOptions::default(),
            &mut caps,
        );
        let text = plan.args.join(" ");
        assert!(text.contains("-c:a aac"), "{text}");
        assert!(text.contains("-b:a 160k"), "{text}");
    }

    #[test]
    fn mp4_output_gets_faststart() {
        let mut caps = caps_with(&["libx264"]);
        let plan = build(
            &media("mpeg2video", "ac3"),
            "in.ts",
            "out.mp4",
            &PlanOptions::default(),
            &mut caps,
        );
        assert!(plan.args.iter().any(|arg| arg == "+faststart"));
    }

    #[test]
    fn subtitle_path_escapes_drive_colon() {
        assert_eq!(escape_subtitles_path("C:/a/b.ass"), r"C\:/a/b.ass");
    }

    #[test]
    fn subtitle_path_keeps_spaces_because_filter_is_quoted() {
        assert_eq!(
            escape_subtitles_path("C:/a b/第 01 集.ass"),
            r"C\:/a b/第 01 集.ass"
        );
        assert_eq!(
            subtitles_filter(&SubtitleBurn::new("C:/a b/x.ass")),
            r"subtitles='C\:/a b/x.ass'"
        );
    }

    #[test]
    fn subtitle_path_escapes_single_quote() {
        // 三个反斜杠是 ffmpeg 的两层解析吃出来的：第一层留下反斜杠加引号，
        // 第二层才还原成单个引号。只写一个会把引号弄丢（会导致文件打不开）。
        assert_eq!(escape_subtitles_path("C:/it's.ass"), r"C\:/it'\\\''s.ass");
        assert_eq!(
            subtitles_filter(&SubtitleBurn::new("C:/it's.ass")),
            r"subtitles='C\:/it'\\\''s.ass'"
        );
    }

    #[test]
    fn subtitle_path_normalizes_windows_separators() {
        assert_eq!(escape_subtitles_path(r"C:\a\b.ass"), r"C\:/a/b.ass");
    }

    #[test]
    fn subtitle_stream_index_is_appended_as_si() {
        let burn = SubtitleBurn::new("C:/a.ass").with_stream(2);
        assert_eq!(subtitles_filter(&burn), r"subtitles='C\:/a.ass':si=2");
    }

    fn subtitle(si: u32, language: &str, title: &str) -> SubtitleStream {
        SubtitleStream {
            index: si + 2,
            si,
            codec: "ass".to_owned(),
            language: language.to_owned(),
            title: title.to_owned(),
        }
    }

    #[test]
    fn simplified_chinese_subtitle_wins_over_the_first_one() {
        let streams = [
            subtitle(0, "jpn", ""),
            subtitle(1, "chi", "简体中文"),
            subtitle(2, "eng", ""),
        ];
        assert_eq!(
            choose_subtitle_stream(&streams).map(|stream| stream.si),
            Some(1)
        );
    }

    #[test]
    fn subtitle_title_can_mark_simplified_chinese() {
        // 语言码是 und 时只能看标题。
        let streams = [
            subtitle(0, "und", "English"),
            subtitle(1, "und", "[简中] 字幕"),
        ];
        assert_eq!(
            choose_subtitle_stream(&streams).map(|stream| stream.si),
            Some(1)
        );
        // 繁体不能抢：语言只写 zh 时，标题里的「繁」更可信。
        let streams = [subtitle(0, "zh", "繁體"), subtitle(1, "und", "简体")];
        assert_eq!(
            choose_subtitle_stream(&streams).map(|stream| stream.si),
            Some(1)
        );
        // 只有繁体 + 英文时，仍然退回第一条，而不是「不烧」。
        let streams = [subtitle(0, "zh-Hant", "繁體"), subtitle(1, "eng", "")];
        assert_eq!(
            choose_subtitle_stream(&streams).map(|stream| stream.si),
            Some(0)
        );
    }

    #[test]
    fn first_subtitle_is_used_when_no_chinese_one() {
        let streams = [subtitle(0, "jpn", ""), subtitle(1, "eng", "")];
        assert_eq!(
            choose_subtitle_stream(&streams).map(|stream| stream.si),
            Some(0)
        );
        assert!(choose_subtitle_stream(&[]).is_none());
    }

    #[test]
    fn burning_subtitles_forces_reencode_even_when_remux_is_possible() {
        let mut caps = caps_with(&["libx264"]);
        let opts = PlanOptions {
            subtitle: Some(SubtitleBurn::new("C:/work/第01集-sub.ass").with_stream(1)),
            ..Default::default()
        };
        // h264 + aac 本来可以 -c copy，带字幕就必须重编码。
        let plan = build(&media("h264", "aac"), "in.mkv", "out.mp4", &opts, &mut caps);
        assert_eq!(plan.mode, "encode");
        let text = plan.args.join(" ");
        assert!(
            text.contains(r"-vf subtitles='C\:/work/第01集-sub.ass':si=1"),
            "{text}"
        );
        assert!(!text.contains("-c copy"), "{text}");
        assert!(plan.reason.contains("烧录字幕"), "{}", plan.reason);
    }
}
