//! 冒烟测试：拿本机真实的 ffmpeg 走一遍「探测 → 选方案」。
//!
//! ```bash
//! cargo run -p fuyubot-plugin-ffmpeg --example plan_smoke
//! ```
//!
//! 干三件事：
//! 1. 生成一个 2 秒的测试视频（h264 + aac）；
//! 2. 探测本机 ffmpeg 的能力，并把每个硬件编码器**真编一帧**验证；
//! 3. 对同一个文件给出两种意图的方案：`compat`（应命中 copy）与 `small`（应选 hevc）。

use std::path::PathBuf;
use std::process::Command;

use fuyubot_plugin_ffmpeg::capability::{Capabilities, is_hw_encoder, probe_media};
use fuyubot_plugin_ffmpeg::job::{JobStatus, Sink, run_blocking};
use fuyubot_plugin_ffmpeg::plan::{Intent, PlanOptions, build};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join("fuyubot-ffmpeg-smoke");
    std::fs::create_dir_all(&dir)?;
    let sample = dir.join("sample.mp4");
    let remuxed = dir.join("remux.mp4");
    let small = dir.join("small.mp4");

    let mut caps = Capabilities::detect(None)?;
    println!("ffmpeg: {}", caps.version);
    println!("路径: {}", caps.ffmpeg.display());

    if !sample.exists() {
        let status = Command::new(&caps.ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=d=2:s=640x360:r=24",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&sample)
            .status()?;
        if !status.success() {
            return Err("生成测试视频失败（本机 ffmpeg 可能缺少 libx264）".into());
        }
    }

    let info = probe_media(caps.ffprobe.as_deref(), &caps.ffmpeg, &sample)?;
    println!(
        "输入: {} / {} + {} / {}ms",
        info.video_codec, info.audio_codec, info.container, info.duration_ms
    );

    // 硬件编码器实测：列表里有 ≠ 能用。
    let hardware: Vec<String> = caps
        .encoders
        .iter()
        .filter(|name| is_hw_encoder(name))
        .cloned()
        .collect();
    println!("\n硬件编码器实测（{} 个候选中）", hardware.len());
    for encoder in &hardware {
        let works = caps.works(encoder);
        println!(
            "  {:<22} {}",
            encoder,
            if works { "可用" } else { "不可用" }
        );
    }

    for (label, intent, output) in [
        ("compat", Intent::Compat, &remuxed),
        ("small", Intent::Small, &small),
    ] {
        let opts = PlanOptions {
            intent,
            target_ext: "mp4".to_owned(),
            ..Default::default()
        };
        let plan = build(
            &info,
            &sample.to_string_lossy(),
            &output.to_string_lossy(),
            &opts,
            &mut caps,
        );
        println!(
            "\n[{label}] mode={} encoder={} hardware={}",
            plan.mode, plan.encoder, plan.hardware
        );
        println!("  理由: {}", plan.reason);
        println!("  参数: {}", plan.args.join(" "));

        // 真跑一遍：这一步同时验证参数正确、进度解析可用。
        let sink = PrintSink { label };
        let status = run_blocking(
            &sink,
            &caps.ffmpeg,
            &plan,
            sample.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
            info.duration_ms,
        );
        println!(
            "  结果: {:?}｜{:.1}%｜耗时 {}ms｜输出存在={}",
            status.state,
            status.percent * 100.0,
            status.elapsed_ms,
            output.exists()
        );
        if let Some(err) = status.error {
            println!("  错误: {err}");
        }
    }

    println!("\n产物目录: {}", PathBuf::from(&dir).display());
    Ok(())
}

/// 命令行里的进度出口：打印关键节点，证明「实时进度」真的在流出来。
struct PrintSink {
    label: &'static str,
}

impl Sink for PrintSink {
    fn progress(&self, status: &JobStatus) {
        println!(
            "  [{}] {:>5.1}%  speed={}x  fps={:.0}  frame={}",
            self.label,
            status.percent * 100.0,
            status.speed,
            status.fps,
            status.frame
        );
    }

    fn finished(&self, event: &'static str, _status: &JobStatus) {
        println!("  [{}] event={event}", self.label);
    }
}
