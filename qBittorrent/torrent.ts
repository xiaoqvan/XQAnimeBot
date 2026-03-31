import { addTorrent } from "../database/create.ts";
import { getQBClient } from "./index.ts";
import parseTorrent, { remote, toMagnetURI } from "parse-torrent";
import logger from "@log/index.ts";
import { updateTorrentStatus } from "../database/update.ts";
import type { TorrentInfo } from "../types/qb.d.ts";

const QBclient = await getQBClient();

const seedingStates = [
  'stoppedUP',
  'stalledUP',
  "forcedUP",
  "uploading"
];

/**
 * 下载种子文件并返回文件路径
 * @param url - 种子文件的URL
 * @param title - 任务标题
 * @returns - Torrent - 种子信息
 */
export async function downloadTorrentFromUrl(
  url: string,
  title: string
): Promise<TorrentInfo | null> {
  // 如果传入的就是磁力链接，直接使用；否则从种子文件解析磁力链接并支持重试
  const isMagnet = url.trim().toLowerCase().startsWith("magnet:");
  const magnetLink = isMagnet
    ? url
    : await retryRequest(async () => {
      return await getMagnetFromTorrent(url);
    });

  if (isMagnet) logger.debug("传入的是磁力链接，跳过解析: ", magnetLink);
  await addTorrent(magnetLink, "等待元数据", title);
  return await downloadAndReturnPath(magnetLink, title);
}

/**
 * 下载并返回文件路径
 * @param magnetLink - 磁力链接
 * @param title - 任务标题
 * @returns - 下载的种子信息
 */
export async function downloadAndReturnPath(
  magnetLink: string,
  title: string
): Promise<TorrentInfo | null> {
  const { infoHash } = await getMagnetHash(magnetLink);
  const hash = infoHash;

  if (!hash) {
    throw new Error("无法从磁力链接中提取 infoHash");
  }

  let torrent: TorrentInfo | null = null;

  // 首先检查是否已经存在相同 hash 的种子，避免重复添加
  try {
    const data = await QBclient.getTorrentByHash(hash);

    torrent = data

    if (torrent) {
      logger.debug(`已存在 hash=${hash} 的种子，跳过添加，直接开始判断状态`);
    } else {
      await QBclient.addTorrentByMagnet(magnetLink);
    }
  } catch (err) {
    logger.warn(
      `检查现有种子时出错，将尝试添加磁力链接: ${err instanceof Error ? err.message : err
      }`
    );
    await QBclient.addTorrentByMagnet(magnetLink);
  }


  // 1. 等待种子信息获取（has_metadata）
  while (true) {
    const data = await QBclient.getTorrentByHash(hash);
    torrent = data;
    if (!torrent) {
      logger.warn(`获取种子信息失败，hash=${hash}`);
      await wait(2000);
      continue;
    }

    const hasMetadata = torrent?.has_metadata === true;

    // 仍保留 progress > 0 作为后备判断
    const progressReady =
      typeof torrent?.progress === "number" && torrent.progress > 0;

    if (hasMetadata || progressReady) break;
    await wait(5000); // 每3秒轮询一次
  }

  // 元数据获取后进行空间校验：当文件大小达到剩余空间的 2 倍时直接报错
  const transferInfo = await QBclient.getTransferInfo();
  const freeSpaceOnDisk = transferInfo.free_space_on_disk;
  const torrentSize = torrent.total_size || torrent.size || 0;
  if (
    typeof freeSpaceOnDisk === "number" &&
    freeSpaceOnDisk > 0 &&
    torrentSize >= freeSpaceOnDisk * 2
  ) {
    throw new Error(
      `磁盘空间不足：文件大小 ${torrentSize} 字节，当前剩余空间 ${freeSpaceOnDisk} 字节（需要至少文件大小的 1/2 以上可用空间）。`
    );
  }

  logger.debug(
    `\x1b[36m[QBclient][${torrent.hash}][${title}]\x1b[0m \x1b[32m元数据已获取，开始下载\x1b[0m`
  );
  await updateTorrentStatus(title, "下载中");

  // 2. 等待下载完成
  while (torrent && (!seedingStates.includes(torrent.state))) {
    await wait(5000); // 每10秒检查一次
    const data = await QBclient.getTorrentByHash(hash);
    torrent = data;
    if (!torrent) {
      logger.warn(`下载过程中获取种子信息失败，hash=${hash}`);
      continue;
    }
    logger.debug(
      `\x1b[36m[QBclient][${torrent.hash}][${title}][${torrent.state}][${torrent.state === "uploading" ? "完成" : "未完成"}]\x1b[0m 下载中... 进度: ${(
        (torrent.progress || 0) * 100
      ).toFixed(2)}%`
    );
  }
  logger.debug(
    `\x1b[36m[QBclient][${torrent?.hash}][${title}][${torrent?.state}][${torrent?.state === "uploading" ? "完成" : "未完成"}]\x1b[0m 下载完成... 进度: ${(
      (torrent?.progress || 0) * 100
    ).toFixed(2)}%`
  );
  await updateTorrentStatus(title, "下载完成");
  return torrent
}

/**
 * 重试请求函数
 * @param requestFn - 请求函数
 * @param maxRetries - 最大重试次数
 * @param delay - 重试间隔（毫秒）
 * @returns 请求结果
 */
async function retryRequest(
  requestFn: () => Promise<any>,
  maxRetries = 3,
  delay = 5000
) {
  let lastError;

  for (let i = 0; i <= maxRetries; i++) {
    try {
      return await requestFn();
    } catch (error) {
      lastError = error;

      if (i === maxRetries) {
        throw lastError;
      }

      logger.warn(`请求失败，${delay / 1000}秒后进行第${i + 1}次重试...`);
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }
}

/**
 * 从种子文件URL获取磁力链接
 * @param url - 种子文件的URL
 * @returns 磁力链接
 */
export async function getMagnetFromTorrent(url: string): Promise<string> {
  return new Promise((resolve, reject) => {
    // some versions of `parse-torrent` have callback-only typings; cast to any
    // to call the 3-argument form (url, opts, cb) and keep runtime behavior.
    (remote as any)(
      url,
      { timeout: 60 * 1000 },
      (err: Error | null, parsed: any) => {
        if (err) return reject(err);
        resolve(toMagnetURI(parsed));
      }
    );
  });
}

/**
 * 获取磁力链接的哈希值
 * @param magnetLink
 * @returns 磁力链接的 infoHash
 */
async function getMagnetHash(magnetLink: string) {
  const parsed = parseTorrent(magnetLink);
  return parsed;
}

// 等待函数
function wait(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
