import { remote } from "parse-torrent";
import logger from "@log/index.ts";

export type TorrentFormat = "mkv" | "mp4" | "mixed" | "unknown";

/**
 * 通过 parse-torrent 远程获取种子元数据（仅几 KB），
 * 检查其中包含的视频文件格式。
 *
 * 目的：在真正下载前判断是否为 MKV（需要烧录字幕），
 * 从而路由到独立的 MKV 处理队列，避免阻塞普通 worker 槽位。
 *
 * @param magnetLink - 磁力链接
 * @returns 检测到的格式类型
 *   - "mkv": 只包含 MKV 文件
 *   - "mp4": 只包含 MP4 文件
 *   - "mixed": 同时包含 MKV 和 MP4
 *   - "unknown": 无法获取元数据或无视频文件
 */
export async function checkTorrentFormat(
  magnetLink: string
): Promise<TorrentFormat> {
  try {
    const parsed = await new Promise<any>((resolve, reject) => {
      (remote as any)(
        magnetLink,
        { timeout: 30_000 },
        (err: Error | null, parsed: any) => {
          if (err) reject(err);
          else resolve(parsed);
        }
      );
    });

    const files: { name?: string; path?: string; length?: number }[] =
      parsed.files ?? [];

    if (files.length === 0) {
      // 无文件列表，尝试从 parsed.name 推断
      const name: string = parsed.name ?? "";
      if (name.toLowerCase().endsWith(".mkv")) return "mkv";
      if (name.toLowerCase().endsWith(".mp4")) return "mp4";
      return "unknown";
    }

    let hasMkv = false;
    let hasMp4 = false;

    for (const f of files) {
      const fileName = f.name ?? f.path ?? "";
      const lower = fileName.toLowerCase();
      if (lower.endsWith(".mkv")) hasMkv = true;
      else if (lower.endsWith(".mp4")) hasMp4 = true;
    }

    if (hasMkv && !hasMp4) return "mkv";
    if (hasMp4 && !hasMkv) return "mp4";
    if (hasMkv && hasMp4) return "mixed";
    return "unknown";
  } catch (err) {
    logger.warn(err, `[checkTorrentFormat] 获取种子元数据失败，标记为 unknown: ${magnetLink.slice(0, 60)}...`);
    return "unknown";
  }
}
