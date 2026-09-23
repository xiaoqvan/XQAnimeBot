import { promises as fs } from "fs";
import path from "path";
import { spawn } from "child_process";

/**
 * 执行外部命令（异步，不阻塞事件循环）
 *
 * @param cmd 要执行的命令（如 ffmpeg / ffprobe）
 * @param args 命令参数数组
 * @param onStderr - 可选，实时接收 stderr 片段（用于解析 ffmpeg 进度）
 * @returns Promise<void> 命令成功完成时 resolve，失败时 reject
 */
function run(cmd: string, args: string[], onStderr?: (chunk: string) => void) {
  return new Promise<void>((resolve, reject) => {
    const p = spawn(cmd, args, { stdio: ["ignore", "ignore", "pipe"] });
    let stderr = "";
    p.stderr?.on("data", (d) => {
      const s = d.toString();
      stderr += s;
      onStderr?.(s);
    });
    p.on("error", (err) => reject(new Error(`${cmd} 启动失败: ${err.message}`)));
    p.on("close", (code) => {
      if (code === 0) {
        resolve();
      } else {
        const detail = stderr.trim() ? `\nstderr: ${stderr.trim()}` : "";
        reject(new Error(`${cmd} exited with ${code}${detail}`));
      }
    });
  });
}

/** 将 ffmpeg 的 HH:MM:SS.ms 时间码转为秒 */
function parseTimecodeToSeconds(tc: string): number {
  const m = tc.match(/^(\d+):(\d+):(\d+(?:\.\d+)?)$/);
  if (!m) return 0;
  return Number(m[1]) * 3600 + Number(m[2]) * 60 + Number(m[3]);
}

/**
 * 读取视频总时长（秒），失败返回 0
 */
async function probeDurationSeconds(file: string): Promise<number> {
  return new Promise((resolve) => {
    const p = spawn(
      "ffprobe",
      [
        "-v", "error",
        "-show_entries", "format=duration",
        "-of", "default=noprint_wrappers=1:nokey=1",
        file,
      ],
      { stdio: ["ignore", "pipe", "ignore"] }
    );
    let out = "";
    p.stdout?.on("data", (d) => (out += d.toString()));
    p.on("close", () => {
      const n = parseFloat(out.trim());
      resolve(Number.isFinite(n) && n > 0 ? n : 0);
    });
    p.on("error", () => resolve(0));
  });
}

/**
 * 解析 ffmpeg stderr 进度（time=HH:MM:SS.ms），换算为百分比回调。
 * 只保留最近缓冲，避免长视频 stderr 累积。
 */
function createFfmpegProgressReporter(
  totalDurationSec: number,
  onProgress?: (percent: number) => void
): (chunk: string) => void {
  let buf = "";
  let lastReported = -1;
  return (chunk: string) => {
    if (!onProgress || totalDurationSec <= 0) return;
    buf += chunk;
    if (buf.length > 4000) buf = buf.slice(-4000);

    // ffmpeg 进度行示例：frame=  123 fps= 45 q=28.0 size=... time=00:01:23.45 bitrate=... speed=1.2x
    const matches = buf.match(/time=(\d+:\d+:\d+(?:\.\d+)?)/g);
    if (!matches?.length) return;
    const last = matches[matches.length - 1]!.slice("time=".length);
    const sec = parseTimecodeToSeconds(last);
    if (sec <= 0) return;

    const raw = Math.max(0, Math.min(99.9, (sec / totalDurationSec) * 100));
    const percent = Math.round(raw * 10) / 10;
    // 变化不足 0.5% 不回调，避免刷爆 progressMap
    if (lastReported >= 0 && Math.abs(percent - lastReported) < 0.5) return;
    lastReported = percent;
    onProgress(percent);
  };
}

/**
 * 将 MKV 转换为 MP4
 *
 * 逻辑顺序：
 * 1. 若存在【简体中文字幕】→ 优先烧录
 * 2. 否则若存在字幕 → 使用默认字幕
 * 3. 否则 → 无字幕兼容性转码
 *
 * 启用缓存（基于文件名 + mtime + size）
 *
 * @param mkv MKV 文件路径
 * @param onProgress - 可选，转码进度百分比回调（0-100）
 * @returns 生成的 MP4 文件路径
 */
export async function mkvToMp4(
  mkv: string,
  onProgress?: (percent: number) => void
): Promise<string> {
  await ensureFFmpeg();

  const stat = await fs.stat(mkv);
  const base = path.basename(mkv, path.extname(mkv));

  const hash = Buffer.from(`${base}_${stat.mtimeMs}_${stat.size}`)
    .toString("hex")
    .slice(0, 10);

  const outDir = path.resolve(process.cwd(), "cache");
  await fs.mkdir(outDir, { recursive: true });

  const outPath = path.join(outDir, `${base}_burn_${hash}.mp4`);

  try {
    await fs.access(outPath);
    onProgress?.(100);
    return outPath;
  } catch { }

  const totalDurationSec = await probeDurationSeconds(mkv);
  const reportProgress = createFfmpegProgressReporter(totalDurationSec, onProgress);
  onProgress?.(0);

  const subtitleIndex = await findSimplifiedChineseSubtitleIndex(mkv);
  const hasSub = subtitleIndex !== null || (await hasAnySubtitles(mkv));

  try {
    if (hasSub) {
      // 先将字幕流提取为独立 .ass 文件，避免 subtitles 滤镜重新解析原文件时
      const subStreamIndex = subtitleIndex ?? 0;
      const subPath = path.join(outDir, `${base}_sub_${hash}.ass`);
      let subExtracted = false;

      try {
        await run("ffmpeg", [
          "-y",
          "-i",
          mkv,
          "-map",
          `0:s:${subStreamIndex}`,
          subPath,
        ]);
        subExtracted = true;
      } catch {
        // 字幕提取失败，降级为无字幕转码
      }

      if (subExtracted) {
        const safeSubPath = subPath
          .replace(/\\/g, "\\\\")
          .replace(/:/g, "\\:")
          .replace(/'/g, "\\'");

        try {
          await run("ffmpeg", [
            "-y",
            "-i",
            mkv,
            "-vf",
            `subtitles='${safeSubPath}'`,
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "20",
            "-c:a",
            "copy",
            outPath,
          ], reportProgress);
        } finally {
          await fs.unlink(subPath).catch(() => { });
        }
      } else {
        // 降级：无字幕兼容性转码
        await run("ffmpeg", [
          "-y",
          "-err_detect",
          "ignore_err",
          "-fflags",
          "+genpts",
          "-i",
          mkv,
          "-c:v",
          "libx264",
          "-preset",
          "veryfast",
          "-profile:v",
          "high",
          "-level",
          "4.1",
          "-pix_fmt",
          "yuv420p",
          "-movflags",
          "+faststart",
          "-crf",
          "23",
          "-c:a",
          "aac",
          "-b:a",
          "192k",
          "-ar",
          "48000",
          "-ac",
          "2",
          "-map_metadata",
          "-1",
          "-map_chapters",
          "-1",
          outPath,
        ], reportProgress);
      }
    } else {
      await run("ffmpeg", [
        "-y",
        "-err_detect",
        "ignore_err",
        "-fflags",
        "+genpts",
        "-i",
        mkv,
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-profile:v",
        "high",
        "-level",
        "4.1",
        "-pix_fmt",
        "yuv420p",
        "-movflags",
        "+faststart",
        "-crf",
        "23",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        "-ar",
        "48000",
        "-ac",
        "2",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        outPath,
      ], reportProgress);
    }

    onProgress?.(100);
    return outPath;
  } catch (err) {
    // 转码失败：清掉半成品，避免 cache 目录堆积
    await fs.unlink(outPath).catch(() => { });
    throw err;
  }
}


let ffmpegChecked = false;

/**
 * 确保系统已安装 ffmpeg 与 ffprobe
 *
 * - 仅在首次调用时执行检测
 * - 后续调用直接跳过
 *
 * @returns Promise<void>
 * @throws 当 ffmpeg / ffprobe 不存在时抛出异常
 */
async function ensureFFmpeg() {
  if (ffmpegChecked) return;

  await Promise.all([
    run("ffmpeg", ["-version"]),
    run("ffprobe", ["-version"]),
  ]);

  ffmpegChecked = true;
}

/**
 * 检测是否存在任意字幕流
 *
 * @param file MKV 文件路径
 */
async function hasAnySubtitles(file: string): Promise<boolean> {
  return new Promise((resolve) => {
    const p = spawn("ffprobe", [
      "-v",
      "error",
      "-select_streams",
      "s",
      "-show_entries",
      "stream=index",
      "-of",
      "csv=p=0",
      file,
    ]);

    let out = "";
    p.stdout?.on("data", (d) => (out += d));

    p.on("close", () => resolve(out.trim().length > 0));
    p.on("error", () => resolve(false));
  });
}

/**
 * 查找 MKV 中的简体中文字幕流索引
 *
 * 判断依据（满足其一即可）：
 * - language: chi / zho / chs / zh-Hans
 * - title 包含: 简体 / CHS / Simplified
 *
 * @param file MKV 文件路径
 * @returns Promise<number | null> 字幕流索引（si），不存在则返回 null
 */
async function findSimplifiedChineseSubtitleIndex(
  file: string
): Promise<number | null> {
  return new Promise((resolve) => {
    const p = spawn("ffprobe", [
      "-v",
      "error",
      "-select_streams",
      "s",
      "-show_entries",
      "stream=index:stream_tags=language,title",
      "-of",
      "json",
      file,
    ]);

    let out = "";
    p.stdout?.on("data", (d) => (out += d));

    p.on("close", () => {
      try {
        const data = JSON.parse(out);
        const streams = data.streams ?? [];

        for (let i = 0; i < streams.length; i++) {
          const s = streams[i];
          const lang = (s.tags?.language || "").toLowerCase();
          const title = (s.tags?.title || "").toLowerCase();

          const isZH =
            ["chi", "zho", "chs", "zh-hans"].includes(lang) ||
            title.includes("简体") ||
            title.includes("chs") ||
            title.includes("simplified");

          if (isZH) {
            return resolve(i);
          }
        }

        resolve(null);
      } catch {
        resolve(null);
      }
    });

    p.on("error", () => resolve(null));
  });
}
