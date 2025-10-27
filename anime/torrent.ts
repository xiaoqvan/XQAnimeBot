import { addTorrent } from "../database/create.ts";
import { getQBClient } from "../qBittorrent/index.ts";
import parseTorrent, { remote, toMagnetURI } from "parse-torrent";
import logger from "@log/index.ts";
import { updateTorrentStatus } from "../database/update.ts";
import type { Torrent } from "../types/torrent.ts";

const QBclient = await getQBClient();

/**
 * 通用的 QB 请求重试封装
 * @param fn - 要执行的请求函数
 * @param maxRetries - 最大重试次数
 * @param initialDelay - 初始延迟时间（毫秒）
 * @returns - 请求结果
 */
async function qbRequestWithRetry<T>(
  fn: () => Promise<T>,
  maxRetries = 3,
  initialDelay = 5000
): Promise<T> {
  let attempt = 0;
  let delay = initialDelay;
  let lastErr: any;

  while (attempt <= maxRetries) {
    try {
      return await fn();
    } catch (err: any) {
      lastErr = err;
      attempt++;
      if (attempt > maxRetries) break;
      logger.warn(
        `QB 请求失败（第 ${attempt}/${maxRetries} 次尝试）。${Math.round(
          delay / 1000
        )} 秒后重试: ${err instanceof Error ? err.message : err}`
      );
      // 指数退避
      await wait(delay);
      delay = Math.min(delay * 2, 10000);
    }
  }

  // 最后一次尝试失败，抛出原始错误
  throw lastErr;
}

/**
 * 下载种子文件并返回文件路径
 * @param url - 种子文件的URL
 * @param title - 任务标题
 * @returns - Torrent - 种子信息
 */
export async function downloadTorrentFromUrl(
  url: string,
  title: string
): Promise<Torrent | null> {
  // 如果传入的就是磁力链接，直接使用；否则从种子文件解析磁力链接并支持重试
  const isMagnet =
    typeof url === "string" && url.trim().toLowerCase().startsWith("magnet:");
  const magnetLink = isMagnet
    ? url
    : await retryRequest(async () => {
        return await getMagnetFromTorrent(url);
      });

  if (isMagnet) logger.debug("传入的是磁力链接，跳过解析: ", magnetLink);
  await addTorrent(magnetLink, "等待元数据", title);
  const torrent = await downloadAndReturnPath(magnetLink, title);

  return torrent;
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
): Promise<Torrent | null> {
  // 检查输入
  if (typeof magnetLink !== "string") return null;

  // 使用重试封装来调用 QBclient.addMagnet，防止偶发超时导致抛出
  await qbRequestWithRetry(() => QBclient.addMagnet(magnetLink));

  const hash = await getMagnetHash(magnetLink);

  let torrent: any | Torrent;

  // 循环直到找到对应 hash 的 torrent（每 2 秒重试一次）
  while (!torrent) {
    try {
      // 使用重试封装获取全部数据
      const data = await qbRequestWithRetry(() => QBclient.getAllData());
      torrent =
        data && data.torrents
          ? data.torrents.find((t) => {
              try {
                return t && t.raw && t.raw.hash === hash;
              } catch {
                return false;
              }
            })
          : null;
      if (torrent) break;
    } catch (err) {
      logger.warn(
        `获取种子列表失败，稍后重试: ${
          err instanceof Error ? err.message : err
        }`
      );
    }
    logger.debug(`未找到 hash=${hash} 的种子，2 秒后重试...`);
    await wait(2000);
  }

  logger.debug(
    `\x1b[36m[QBclient][${torrent.id}][${title}]\x1b[0m \x1b[32m种子已添加，等待元数据...\x1b[0m`
  );

  // 1. 等待种子信息获取（has_metadata）
  while (true) {
    // 使用 getTorrent 替代 getAllData，并加重试
    const torrentId = torrent?.id;
    if (!torrentId) {
      // 若暂时没有 id，等待并重试
      await wait(1000);
      continue;
    }
    const data = await qbRequestWithRetry(() => QBclient.getAllData());
    // 兼容 t 或 t.torrent 等两种返回结构，保留旧的 torrent 对象以防接口短暂返回 null
    torrent = data.torrents.find((t) => t.id === torrentId) || torrent;

    // 检查多种可能的位置上的 has_metadata 字段
    const raw = torrent?.raw || torrent;
    const hasMetadata = raw?.has_metadata === true;

    // 仍保留 progress > 0 作为后备判断
    const progressReady =
      typeof torrent?.progress === "number" && torrent.progress > 0;

    if (hasMetadata || progressReady) break;
    await wait(5000); // 每3秒轮询一次
  }

  logger.debug(
    `\x1b[36m[QBclient][${torrent.id}][${title}]\x1b[0m \x1b[32m元数据已获取，开始下载\x1b[0m`
  );
  await updateTorrentStatus(title, "下载中");

  // 2. 等待下载完成
  while (!torrent.isCompleted) {
    await wait(10000); // 每5秒检查一次
    const data = await QBclient.getAllData();
    // 兼容返回结构，若为空则保留上次的 torrent 对象
    torrent = data.torrents.find((t) => t.id === torrent.id) || torrent;
  }

  await updateTorrentStatus(title, "下载完成");
  return torrent as Torrent;
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
    remote(url, { timeout: 60 * 1000 }, (err: Error | null, parsed) => {
      if (err) return reject(err);
      resolve(toMagnetURI(parsed));
    });
  });
}

/**
 * 获取磁力链接的哈希值
 * @param {string} magnetLink
 * @returns {Promise<string|null>}
 */
async function getMagnetHash(magnetLink: string) {
  const parsed = await parseTorrent(magnetLink);
  return parsed.infoHash;
}

// 等待函数
function wait(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
