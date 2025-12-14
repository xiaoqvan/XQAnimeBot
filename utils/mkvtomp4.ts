import { execSync } from "child_process";
import path from "path";
import fs from "fs";

export async function mkvToMp4(mkv: string): Promise<string> {
  try {
    await ensureFFmpeg();

    const hasSub = checkHasSubtitles(mkv);
    const outDir = path.resolve(process.cwd(), "cache");
    if (!fs.existsSync(outDir)) fs.mkdirSync(outDir, { recursive: true });

    const base = path.basename(mkv, path.extname(mkv));
    const hash = Buffer.from(
      base + fs.statSync(mkv).mtimeMs + fs.statSync(mkv).size
    )
      .toString("hex")
      .slice(0, 10);
    const outPath = path.join(outDir, `${base}_burn_${hash}.mp4`);

    if (hasSub) {
      // 使用 subtitles 过滤器将内嵌或外部字幕烧录到视频上
      const safePath = mkv.replace(/'/g, "\\'");
      const vf = `subtitles='${safePath}'`;
      execSync(
        `ffmpeg -y -i "${mkv}" -vf "${vf}" -c:v libx264 -preset veryfast -crf 23 -c:a aac -b:a 192k "${outPath}"`,
        { stdio: ["ignore", "ignore", "ignore"] }
      );
    } else {
      // 没有字幕，做简单的转封装/转码为 MP4（保证兼容性）
      execSync(
        `ffmpeg -y -err_detect ignore_err -fflags +genpts -i "${mkv}" -c:v libx264 -preset veryfast -profile:v high -level 4.1 -pix_fmt yuv420p -movflags +faststart -crf 23 -c:a aac -b:a 192k -ar 48000 -ac 2 -map_metadata -1 -map_chapters -1 "${outPath}"`,
        { stdio: ["ignore", "ignore", "ignore"] }
      );
    }

    return outPath;
  } catch (e) {
    throw new Error(`MKV 转 MP4 失败: ${(e as Error).message}`);
  }
}

function ensureFFmpeg() {
  try {
    execSync("ffmpeg -version", { stdio: "ignore" });
    execSync("ffprobe -version", { stdio: "ignore" });
    return Promise.resolve();
  } catch {
    return Promise.reject(
      new Error("未检测到 ffmpeg 或 ffprobe，请先安装 ffmpeg。")
    );
  }
}

function checkHasSubtitles(filePath: string): boolean {
  try {
    const out = execSync(
      `ffprobe -v error -select_streams s -show_entries stream=index -of csv=p=0 "${filePath}"`,
      { stdio: ["ignore", "pipe", "ignore"] }
    )
      .toString()
      .trim();
    return out.length > 0;
  } catch {
    return false;
  }
}
