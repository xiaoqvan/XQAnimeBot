import type { Torrent } from "../types/torrent.ts";
import type { animeItem } from "../types/anime.js";

import { downloadTorrentFromUrl } from "./torrent.ts";
import { getQBClient } from "../qBittorrent/index.ts";
import { promises as fs } from "fs";
import { extname } from "path";

import logger from "@log/index.ts";
import { mkvToMp4 } from "../utils/mkvtomp4.ts";

const TORRENT_MAX_SIZE = 2 * 1024 * 1024 * 1024;

/**
 * 下载并验证种子文件
 * @param item - 动漫条目包含种子信息
 * @returns 已下载并验证的种子对象
 * @throws 当下载失败或文件不符合要求时抛出异常
 */
export async function downloadAndValidateTorrent(
    item: animeItem,
): Promise<Torrent> {
    let failMessage = `种子下载失败: ${item.title}, magnet: ${item.magnet}`;
    const QBclient = await getQBClient();
    const torrent = await downloadTorrentFromUrl(item.magnet, item.title);
    if (!torrent) {
        logger.error(failMessage);
        throw new Error(failMessage);
    }

    if (torrent.totalSize > TORRENT_MAX_SIZE) {
        logger.warn(
            `种子文件过大(${(torrent.totalSize / 1024 / 1024 / 1024).toFixed(
                2
            )}GB): ${item.title}, 已跳过`
        );
        await QBclient.removeTorrent(torrent.id, true);
        throw new Error(`种子文件过大: ${item.title}`);
    }

    if (torrent.raw.content_path) {
        try {
            const stats = await fs.stat(torrent.raw.content_path);
            if (stats.isDirectory()) {
                logger.warn(
                    `下载路径是文件夹，跳过: ${torrent.raw.content_path} (${item.title})`
                );
                await QBclient.removeTorrent(torrent.id, true);
                throw new Error(`下载路径是文件夹: ${item.title}`);
            }
            const fileExt = extname(torrent.raw.content_path).toLowerCase();
            if (stats.isFile() && fileExt === ".mkv") {
                logger.warn(
                    `下载文件为 MKV，尝试转换为 MP4: ${torrent.raw.content_path} (${item.title})`
                );
                const video = await mkvToMp4(torrent.raw.content_path);
                fs.unlink(torrent.raw.content_path).catch(() => { });
                torrent.raw.content_path = video;
                // 不需要删除QBclient中的种子，保留以便后续处理
                return torrent;
            }
        } catch (err) {
            logger.error("检查下载路径类型时出错", err);
            await QBclient.removeTorrent(torrent.id, true);
            throw err;
        }
    }
    failMessage += "，文件类型不符合要求（非 MKV）";
    logger.error(failMessage);
    await QBclient.removeTorrent(torrent.id, true);
    throw new Error(failMessage);
}

export async function removeTorrentAndData(torrentId: string) {
    const QBclient = await getQBClient();
    await QBclient.removeTorrent(torrentId, true);
}