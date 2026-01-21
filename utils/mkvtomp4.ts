import { promises as fs } from "fs";
import path from "path";
import { spawn } from "child_process";

/**
 * 执行外部命令（异步，不阻塞事件循环）
 *
 * @param cmd 要执行的命令（如 ffmpeg / ffprobe）
 * @param args 命令参数数组
 * @returns Promise<void> 命令成功完成时 resolve，失败时 reject
 */
function run(cmd: string, args: string[]) {
  return new Promise<void>((resolve, reject) => {
    const p = spawn(cmd, args, { stdio: "ignore" });
    p.on("error", reject);
    p.on("close", (code) => {
      code === 0 ? resolve() : reject(new Error(`${cmd} exited with ${code}`));
    });
  });
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
 * @returns 生成的 MP4 文件路径
 */
export async function mkvToMp4(mkv: string): Promise<string> {
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
    return outPath;
  } catch { }

  const subtitleIndex = await findSimplifiedChineseSubtitleIndex(mkv);
  const hasSub = subtitleIndex !== null || (await hasAnySubtitles(mkv));

  if (hasSub) {
    const safePath = mkv.replace(/'/g, "\\'");
    const vf =
      subtitleIndex !== null
        ? `subtitles='${safePath}':si=${subtitleIndex}`
        : `subtitles='${safePath}'`;

    await run("ffmpeg", [
      "-y",
      "-i",
      mkv,
      "-vf",
      vf,
      "-c:v",
      "libx264",
      "-preset",
      "veryfast",
      "-crf",
      "23",
      "-c:a",
      "aac",
      "-b:a",
      "192k",
      outPath,
    ]);
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
    ]);
  }

  return outPath;
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

        for (const s of streams) {
          const lang = (s.tags?.language || "").toLowerCase();
          const title = (s.tags?.title || "").toLowerCase();

          const isZH =
            ["chi", "zho", "chs", "zh-hans"].includes(lang) ||
            title.includes("简体") ||
            title.includes("chs") ||
            title.includes("simplified");

          if (isZH && typeof s.index === "number") {
            return resolve(s.index);
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
