import { execSync } from "child_process";
import path from "path";
import fs from "fs";
import axios from "axios";
import type { BtData } from "../types/anime.ts";
/**
 * 将 Unix 时间戳（以秒为单位）转换为格式化的日期字符串。
 *
 * @param date - 以秒为单位的 Unix 时间戳。
 * @param timezone - 时区，如 'Asia/Shanghai'，默认为 UTC。
 * @returns 格式化的日期字符串，格式为 "YYYY-MM-DD HH:mm:ss"。
 */
export function formattedDate(date: number, timezone = "UTC") {
  const dateObj = new Date(date * 1000);

  if (timezone === "UTC") {
    return dateObj.toISOString().replace("T", " ").split(".")[0];
  }

  const options: Intl.DateTimeFormatOptions = {
    timeZone: timezone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  };

  const formatter = new Intl.DateTimeFormat("sv-SE", options);
  return formatter.format(dateObj).replace(",", "");
}

/**
 * 下载文件到 cache 目录，返回本地路径
 * @param url 文件下载地址
 * @returns 下载后的本地文件路径
 */
export async function downloadFile(url: string): Promise<string> {
  const cacheDir = path.resolve(process.cwd(), "cache");
  if (!fs.existsSync(cacheDir)) {
    fs.mkdirSync(cacheDir, { recursive: true });
  }
  const baseName = path.basename(new URL(url).pathname) || `file_${Date.now()}`;
  // 生成 6 位随机十六进制 hash
  const randomHash = Math.random().toString(16).slice(2, 8);
  // 插入 hash 到文件名（如 a.png -> a_xxxxx.png）
  const ext = path.extname(baseName);
  const nameWithoutExt = path.basename(baseName, ext);
  const fileName = `${nameWithoutExt}_${randomHash}${ext}`;
  const filePath = path.join(cacheDir, fileName);

  const writer = fs.createWriteStream(filePath);
  const response = await axios.get(url, { responseType: "stream" });
  await new Promise<void>((resolve, reject) => {
    response.data.pipe(writer);
    writer.on("finish", () => resolve());
    writer.on("error", reject);
  });
  return filePath;
}

/**
 * 提取视频元数据：宽度、高度、时长（秒）、封面截图路径。
 * @param videoPath 视频文件路径
 * @returns {Promise<{ width: number, height: number, duration: number, coverPath: string }>}
 * @throws 如果未安装 ffmpeg 或 ffprobe，则抛出错误
 */
export async function extractVideoMetadata(videoPath: string): Promise<{
  /** 视频宽度 */
  width: number;
  /** 视频高度 */
  height: number;
  /** 视频时长，单位秒 */
  duration: number;
  /** 视频封面截图路径 */
  coverPath: string;
}> {
  // 检查 ffmpeg 和 ffprobe 是否可用
  try {
    execSync("ffmpeg -version", { stdio: "ignore" });
    execSync("ffprobe -version", { stdio: "ignore" });
  } catch {
    throw new Error("未检测到 ffmpeg 或 ffprobe，请先安装 ffmpeg。");
  }

  // 获取元数据（宽高、时长，兼容MKV等格式）
  let width = 0,
    height = 0,
    duration = 0;
  let ffprobeStreamOutput = "";
  let ffprobeFormatOutput = "";

  try {
    ffprobeStreamOutput = execSync(
      `ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of default=noprint_wrappers=1:nokey=0 "${videoPath}"`,
      { stdio: ["ignore", "pipe", "ignore"] }
    ).toString();
    ffprobeFormatOutput = execSync(
      `ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=0 "${videoPath}"`,
      { stdio: ["ignore", "pipe", "ignore"] }
    ).toString();
  } catch {
    throw new Error("ffprobe 获取视频信息失败");
  }
  ffprobeStreamOutput.split("\n").forEach((line) => {
    if (line.startsWith("width=")) width = parseInt(line.replace("width=", ""));
    if (line.startsWith("height="))
      height = parseInt(line.replace("height=", ""));
  });
  // 优先用format.duration，stream没有则兜底
  const durationMatch = ffprobeFormatOutput.match(/duration=([0-9.]+)/);
  if (durationMatch) {
    duration = Math.floor(parseFloat(durationMatch[1]));
  }
  if (!duration) {
    // 兜底尝试从视频流获取
    const streamDurationMatch = ffprobeStreamOutput.match(/duration=([0-9.]+)/);
    if (streamDurationMatch)
      duration = Math.floor(parseFloat(streamDurationMatch[1]));
  }
  if (!width || !height || !duration) {
    throw new Error("未能正确解析视频元数据");
  }

  // 生成封面截图，文件名添加hash避免冲突
  const coverDir = path.resolve(process.cwd(), "cache");
  if (!fs.existsSync(coverDir)) fs.mkdirSync(coverDir, { recursive: true });

  let fileHash = "";
  try {
    const stat = fs.statSync(videoPath);
    const base = path.basename(videoPath);
    fileHash = Buffer.from(base + stat.mtimeMs + stat.size)
      .toString("hex")
      .slice(0, 12);
  } catch {
    fileHash = Math.random().toString(16).slice(2, 10);
  }
  const coverPath = path.join(
    coverDir,
    `cover_${fileHash}_${Date.now()}_${Math.floor(Math.random() * 10000)}.jpg`
  );
  try {
    // 截取第10秒的帧作为封面
    execSync(
      `ffmpeg -y -ss 10 -i "${videoPath}" -frames:v 1 -q:v 2 "${coverPath}"`,
      { stdio: ["ignore", "ignore", "ignore"] }
    );
  } catch {
    throw new Error("ffmpeg 截图失败");
  }

  return { width, height, duration, coverPath };
}

/**
 * 格式化字幕组名称，为特殊字幕组添加source后缀
 * @param subGroup - 原始字幕组名称
 * @param [source] - 视频来源（如：Baha, bilibili等）
 * @returns 格式化后的字幕组名称
 */
export function formatSubGroupName(
  subGroup: string,
  source: string | undefined
) {
  // 特殊字幕组列表，需要根据source区分
  const specialGroups = ["ANi", "黒ネズミたち"];

  // 检查是否为特殊字幕组
  if (specialGroups.includes(subGroup)) {
    if (source) {
      return `${subGroup}_${source}`;
    }
    // 如果没有source，直接返回原名称
    return subGroup;
  }

  // 非特殊字幕组直接返回原名称
  return subGroup;
}

/**
 * 在 btdata 中查找 cache_id 匹配的 episodeData
 * @param btdata - anime.btdata
 * @param cacheId - 要匹配的 cache_id（数字或字符串）
 * @returns 找到则返回 { group, episode }，否则返回 null
 */
export function findEpisodeByCacheId(
  btdata: BtData,
  cacheId: string | number,
  title: string | undefined
) {
  const target = String(cacheId);

  // 1) 优先按 cache_id 精确匹配（支持 number | string 存储）
  for (const [group, episodes] of Object.entries(btdata)) {
    if (!Array.isArray(episodes)) continue;
    for (const ep of episodes) {
      if (!ep) continue;
      if (ep.cache_id && String(ep.cache_id) === target) {
        return { group, episode: ep };
      }
    }
  }

  // 2) 回退：若提供 title，则尝试按标题/其它 id 字段匹配（大小写不敏感）
  if (title) {
    const tRaw = String(title);
    const tLower = tRaw.toLowerCase();
    for (const [group, episodes] of Object.entries(btdata)) {
      if (!Array.isArray(episodes)) continue;
      for (const ep of episodes) {
        if (!ep) continue;
        // 候选字段：title, videoid, unique_id, episode
        const candidates: string[] = [];
        if (ep.title) candidates.push(String(ep.title));
        if ((ep as any).videoid) candidates.push(String((ep as any).videoid));
        if ((ep as any).unique_id)
          candidates.push(String((ep as any).unique_id));
        if ((ep as any).episode) candidates.push(String((ep as any).episode));

        for (const c of candidates) {
          if (c === tRaw || c.toLowerCase() === tLower) {
            return { group, episode: ep };
          }
        }
      }
    }
  }

  return null;
}

/**
 * 从对象中移除指定的键，返回一个新的对象
 * @param obj - 原始对象
 * @param keys - 要移除的键数组
 * @returns 去掉指定键后的新对象
 */
export function omit<T extends object, K extends keyof T>(
  obj: T,
  keys: K[]
): Omit<T, K> {
  const clone = { ...obj };
  for (const key of keys) {
    delete clone[key];
  }
  return clone;
}
