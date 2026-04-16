import type { animeItem } from "../types/rss.d.ts";

import { downloadTorrentFromUrl } from "./torrent.ts";
import { getQBClient } from "../qBittorrent/index.ts";
import { promises as fs } from "fs";
import { extname } from "path";

import logger from "@log/index.ts";
import { mkvToMp4 } from "../utils/mkvtomp4.ts";
import { splitVideoByMaxSize } from "../utils/cuttingvideo.ts";
import type { TorrentInfo } from "../types/qb.js";

export interface ExtendedTorrentInfo extends TorrentInfo {
    segments: string[]
}

const TORRENT_MAX_SIZE = 2 * 1024 * 1024 * 1024;

/**
 * 下载并验证种子文件
 * @param item - 动漫条目包含种子信息
 * @returns 已下载并验证的种子对象
 * @throws 当下载失败或文件不符合要求时抛出异常
 */
export async function downloadAndValidateTorrent(
    item: animeItem,
): Promise<ExtendedTorrentInfo> {
    let failMessage = `种子下载失败: ${item.title}, magnet: ${item.magnet}`;
    const QBclient = await getQBClient();
    const torrent = await downloadTorrentFromUrl(item.magnet, item.title);
    const extendedTorrent = torrent as ExtendedTorrentInfo;
    if (!extendedTorrent) {
        logger.error(failMessage);
        throw new Error(failMessage);
    }

    if (extendedTorrent.content_path) {
        if (Array.isArray(extendedTorrent.content_path)) {
            return extendedTorrent;
        }
        try {
            const stats = await fs.stat(extendedTorrent.content_path);
            if (stats.isDirectory()) {
                logger.warn(
                    `下载路径是文件夹，跳过: ${extendedTorrent.content_path} (${item.title})`
                );
                await QBclient.deleteTorrent(extendedTorrent.hash, true);
                throw new Error(`下载路径是文件夹: ${item.title}`);
            }
            let currentVideoPath = extendedTorrent.content_path;
            let currentVideoStats = stats;
            let convertedFromMkv = false;
            const fileExt = extname(currentVideoPath).toLowerCase();

            if (currentVideoStats.isFile() && fileExt === ".mkv") {
                logger.warn(
                    `下载文件为 MKV，先转换为 MP4: ${currentVideoPath} (${item.title})`
                );
                const sourceVideoPath = currentVideoPath;
                const convertedVideoPath = await mkvToMp4(sourceVideoPath);
                await fs.unlink(sourceVideoPath).catch(() => { });
                currentVideoPath = convertedVideoPath;
                extendedTorrent.content_path = convertedVideoPath;
                currentVideoStats = await fs.stat(currentVideoPath);
                convertedFromMkv = true;
            }

            const currentFileExt = extname(currentVideoPath).toLowerCase();
            if (!(currentVideoStats.isFile() && currentFileExt === ".mp4")) {
                failMessage += "，文件类型不符合要求（非 MKV/MP4）";
                logger.error(failMessage);
                await QBclient.deleteTorrent(extendedTorrent.hash, true);
                throw new Error(failMessage);
            }

            if (currentVideoStats.size > TORRENT_MAX_SIZE) {
                logger.warn(`视频大小 ${(
                    currentVideoStats.size /
                    (1024 * 1024 * 1024)
                ).toFixed(2)} GiB，超过限制，尝试分段: ${item.title}`);
                try {
                    const videoPath = await splitVideoByMaxSize({
                        input: currentVideoPath,
                        outDir: "cache/videos",
                        maxGiB: 1.95,
                    });
                    extendedTorrent.segments = videoPath;

                    if (convertedFromMkv) {
                        await fs.unlink(currentVideoPath).catch(() => { });
                    }
                    return extendedTorrent;
                } catch (error) {
                    logger.error("分段视频失败", error);
                    throw error;
                }
            }

            return extendedTorrent;
        } catch (err) {
            logger.error("检查下载路径类型时出错", err);
            await QBclient.deleteTorrent(extendedTorrent.hash, true);
            throw err;
        }
    }
    failMessage += "，文件类型不符合要求（非 MKV）";
    logger.error(failMessage);
    await QBclient.deleteTorrent(extendedTorrent.hash, true);
    throw new Error(failMessage);
}

/**
 * 从 qBittorrent 中删除指定种子及其数据
 * @param torrentId - 种子哈希或 ID
 */
export async function removeTorrentAndData(torrentId: string) {
    const QBclient = await getQBClient();
    await QBclient.deleteTorrent(torrentId, true);
}