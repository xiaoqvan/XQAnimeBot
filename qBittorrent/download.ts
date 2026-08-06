import type { animeItem } from "../types/rss.d.ts";

import { downloadTorrentFromUrl } from "./torrent.ts";
import { getQBClient } from "../qBittorrent/index.ts";
import { promises as fs } from "fs";
import { extname } from "path";

import logger from "@log/index.ts";
import { mkvToMp4 } from "../utils/mkvtomp4.ts";
import { splitVideoByMaxSize } from "../utils/cuttingvideo.ts";
import type { TorrentInfo } from "../types/qb.js";
import type { ExclusiveSlotController } from "../anime/AnimeProcessorManager.ts";

export interface ExtendedTorrentInfo extends TorrentInfo {
    segments: string[];
    /** 原始文件是否为 MKV（尚未/正在转换） */
    isMkv?: boolean;
}

/**
 * 下载进度更新回调
 */
export type ProgressCallback = (stage: string) => void;

/**
 * 需要串行执行的重型转换槽位控制器
 */
export type HeavyVideoTaskGate = Pick<ExclusiveSlotController, "withExclusiveSlot">;

const TORRENT_MAX_SIZE = 2 * 1024 * 1024 * 1024;

/**
 * 下载并验证种子文件
 * @param item - 动漫条目包含种子信息
 * @param heavyTaskGate - 重型任务槽位控制器
 * @param onStage - 可选的进度回调，用于外部更新进度显示（如"烧录字幕中"）
 * @param skipConversion - 若为 true，遇到 MKV 时不转码，暂停种子后直接返回（由 MKV 队列处理）
 * @returns 已下载并验证的种子对象
 * @throws 当下载失败或文件不符合要求时抛出异常
 */
export async function downloadAndValidateTorrent(
    item: animeItem,
    heavyTaskGate?: HeavyVideoTaskGate,
    onStage?: ProgressCallback,
    skipConversion?: boolean,
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
        // 记录本次转换/分段产生的临时文件，出错时统一清理，避免 cache 目录堆积占满磁盘
        const tempFilesToClean: string[] = [];
        let currentVideoPath = extendedTorrent.content_path;
        const runHeavyVideoTask = async <T>(task: () => Promise<T>) => {
            if (!heavyTaskGate) {
                return await task();
            }
            return await heavyTaskGate.withExclusiveSlot(task);
        };
        try {
            const stats = await fs.stat(extendedTorrent.content_path);
            if (stats.isDirectory()) {
                logger.warn(
                    `下载路径是文件夹，跳过: ${extendedTorrent.content_path} (${item.title})`
                );
                await QBclient.deleteTorrent(extendedTorrent.hash, true);
                throw new Error(`下载路径是文件夹: ${item.title}`);
            }
            let currentVideoStats = stats;
            let convertedFromMkv = false;
            const fileExt = extname(currentVideoPath).toLowerCase();

            if (currentVideoStats.isFile() && fileExt === ".mkv") {
                if (skipConversion) {
                    // 不下Convert，暂停种子，标记后返回，由 MKV 队列处理后续转码+发送
                    logger.warn(
                        `下载文件为 MKV，暂停种子并移交 MKV 队列: ${currentVideoPath} (${item.title})`
                    );
                    await QBclient.pauseTorrent(extendedTorrent.hash).catch(() => { });
                    extendedTorrent.isMkv = true;
                    return extendedTorrent;
                }

                logger.warn(
                    `下载文件为 MKV，先转换为 MP4: ${currentVideoPath} (${item.title})`
                );
                onStage?.("烧录字幕中");
                const sourceVideoPath = currentVideoPath;
                const convertedVideoPath = await runHeavyVideoTask(() =>
                    mkvToMp4(sourceVideoPath)
                );
                onStage?.("烧录完成");
                await fs.unlink(sourceVideoPath).catch(() => { });
                currentVideoPath = convertedVideoPath;
                tempFilesToClean.push(convertedVideoPath);
                extendedTorrent.content_path = convertedVideoPath;
                currentVideoStats = await fs.stat(currentVideoPath);
                convertedFromMkv = true;
            }

            const currentFileExt = extname(currentVideoPath).toLowerCase();
            if (!(currentVideoStats.isFile() && currentFileExt === ".mp4")) {
                failMessage += "，文件类型不符合要求（非 MKV/MP4）";
                logger.error(failMessage);
                await QBclient.deleteTorrent(extendedTorrent.hash, true);
                await cleanupTempFiles(tempFilesToClean);
                throw new Error(failMessage);
            }

            if (currentVideoStats.size > TORRENT_MAX_SIZE) {
                logger.warn(`视频大小 ${(
                    currentVideoStats.size /
                    (1024 * 1024 * 1024)
                ).toFixed(2)} GiB，超过限制，尝试分段: ${item.title}`);
                onStage?.("分段切割中");
                try {
                    const videoPath = await runHeavyVideoTask(() =>
                        splitVideoByMaxSize({
                            input: currentVideoPath,
                            outDir: "cache/videos",
                            maxGiB: 1.95,
                        })
                    );
                    extendedTorrent.segments = videoPath;
                    // 分段文件发送完成后由 sendMegTo* 清理，此处只记录以便出错时兜底
                    tempFilesToClean.push(...videoPath);

                    if (convertedFromMkv) {
                        await fs.unlink(currentVideoPath).catch(() => { });
                    }
                    return extendedTorrent;
                } catch (error) {
                    logger.error(error, "分段视频失败");
                    // 分段失败：清理已生成的分段文件和转换产物，避免残留
                    await cleanupTempFiles(tempFilesToClean);
                    throw error;
                }
            }

            return extendedTorrent;
        } catch (err) {
            logger.error(err, "检查下载路径类型时出错");
            await QBclient.deleteTorrent(extendedTorrent.hash, true);
            // 出错时清理已生成的转换/分段临时文件
            await cleanupTempFiles(tempFilesToClean);
            throw err;
        }
    }
    failMessage += "，文件类型不符合要求（非 MKV）";
    logger.error(failMessage);
    await QBclient.deleteTorrent(extendedTorrent.hash, true);
    throw new Error(failMessage);
}

/**
 * 批量删除临时文件并抑制单个文件删除失败抛出的错误。
 * 用于在转换/分段失败时清理中间产物，避免 cache 目录堆积。
 * @param paths - 待删除的文件路径数组
 */
async function cleanupTempFiles(paths: string[]): Promise<void> {
    for (const p of paths) {
        try {
            await fs.unlink(p);
        } catch {
            // 文件可能已不存在，忽略删除错误
        }
    }
}

/**
 * 从 qBittorrent 中删除指定种子及其数据。
 *
 * 删除失败会自动重试一次，并记录日志（不再静默吞错）。
 * 返回值表示是否成功删除，调用方无需再 .catch 吞错。
 *
 * @param torrentId - 种子哈希或 ID
 * @returns 删除成功返回 true，失败返回 false
 */
export async function removeTorrentAndData(torrentId: string): Promise<boolean> {
    if (!torrentId) return false;
    let QBclient;
    try {
        QBclient = await getQBClient();
        await QBclient.deleteTorrent(torrentId, true);
        return true;
    } catch (err) {
        logger.warn(err, `删除种子失败（将重试一次）: ${torrentId}`);
    }
    try {
        if (!QBclient) QBclient = await getQBClient();
        await QBclient.deleteTorrent(torrentId, true);
        return true;
    } catch (err) {
        logger.error(err, `删除种子重试仍失败，请手动清理: ${torrentId}`);
        return false;
    }
}