import logger from "@log/index.ts";
import parseTorrent from "parse-torrent";
import { animeinfo } from "./get.ts";
import { updateAnimeBtdata } from "../database/update.ts";
import { addCacheItem, addTorrent, saveAnime } from "../database/create.ts";
import { getMessageLink } from "@TDLib/function/get.ts";
import { sendMegToAnime, sendMegToCache, sendMegToNavAnime } from "./sendAnime.ts";
import { buildAndSaveAnimeFromInfo } from "../utils/buildAnimeinfo.ts";
import { combineFansub } from "../utils/index.ts";
import { downloadAndValidateTorrent, removeTorrentAndData } from "../qBittorrent/download.ts";
import { matchBangumiEpisode } from "../utils/matcher.ts";
import {
    promptAdminProvideAnimeInfo,
    promptAdminConfirmAnime,
    promptAdminConfirmAnimeEpisodes,
} from "./adminPrompts.ts";
import type { AnimeProcessorManager } from "./AnimeProcessorManager.ts";
import type { albumMessageType, anime as animeType, animeItem } from "../types/anime.d.ts";
import type { Client } from "tdl";
import type { message } from "tdlib-types";

/**
 * 从磁力链接中同步解析 infoHash，失败时返回 undefined
 * 解析失败不应影响主流程，仅影响进度追踪精度
 *
 * @param magnet - 磁力链接字符串
 * @returns infoHash 十六进制字符串，或 undefined
 */
function tryParseMagnetHash(magnet: string): string | undefined {
    try {
        const parsed = parseTorrent(magnet);
        return parsed?.infoHash ?? undefined;
    } catch {
        return undefined;
    }
}

/**
 * 从消息列表中提取用于数据库写入的相册消息数据
 *
 * @param msgList - 过滤后的有效消息数组
 * @returns albumMessageType 数组
 */
function extractAlbumMsgData(msgList: message[]): albumMessageType[] {
    return msgList.map((msg) => ({
        chat_id: msg.chat_id,
        message_id: msg.id,
        topic_id: msg.topic_id,
        videoid:
            msg.content._ === "messageVideo"
                ? msg.content.video.video.remote.id
                : undefined,
        unique_id:
            msg.content._ === "messageVideo"
                ? msg.content.video.video.remote.unique_id
                : undefined,
    }));
}

/**
 * 将消息响应（单条或相册）标准化为有序消息数组
 *
 * @param megResult - sendMessage / sendMessageAlbum 的原始返回值
 * @returns 过滤掉 null 的有效 message 数组
 */
function normalizeMsgResult(
    megResult: message | { _: "messages"; messages: (message | null)[] }
): message[] {
    return megResult._ === "messages"
        ? megResult.messages.filter((m): m is message => m !== null)
        : [megResult];
}

/**
 * 处理全新番剧（数据库中不存在）的完整流程：
 * 1. 搜索 bangumi.tv 获取番剧元数据
 * 2. 创建缓存条目，记录种子信息防止重复处理
 * 3. 下载 BT 视频文件
 * 4. 将视频发送到管理员缓冲频道（待审核）
 * 5. 更新数据库中的 btdata 记录
 * 6. 通知管理员审核番剧信息及集数匹配
 *
 * @param client - TDLib 客户端实例
 * @param item - 已完整解析的动漫 BT 条目
 * @param manager - 并发处理管理器，用于全程更新进度阶段
 */
export async function handleNewAnime(
    client: Client,
    item: animeItem,
    manager: AnimeProcessorManager
): Promise<void> {
    manager.updateProgress(item.title, "搜索番剧信息");
    const searchAnime = await animeinfo(item.names[0]);

    // 先创建缓存条目和种子记录，防止并发时重复处理同一条目
    const Cache_id = await addCacheItem(item);
    await addTorrent(item.magnet, "等待下载", item.title);

    if (!searchAnime.data || searchAnime.data.length === 0) {
        manager.updateProgress(item.title, "通知管理员提供番剧信息");
        promptAdminProvideAnimeInfo(client, Cache_id, item);
        return;
    }

    const anime = await buildAndSaveAnimeFromInfo(searchAnime.data[0], true);

    // 解析磁力 hash 以供进度追踪（失败不阻断主流程）
    const magnetHash = tryParseMagnetHash(item.magnet);
    manager.updateProgress(item.title, "下载BT种子", {
        animeName: anime.name_cn || anime.name,
        ...(magnetHash ? { torrentHash: magnetHash } : {}),
    });

    const torrent = await downloadAndValidateTorrent(item);
    if (!torrent) return;

    manager.updateProgress(item.title, "发送视频到缓冲频道");
    const animeMeg = await sendMegToCache(
        client,
        anime,
        item,
        torrent.content_path,
        torrent.segments
    );
    removeTorrentAndData(torrent.hash).catch(() => { });

    if (!animeMeg) {
        logger.error("发送动漫消息失败");
        throw new Error("发送动漫消息失败");
    }

    manager.updateProgress(item.title, "更新数据库");

    const megList = normalizeMsgResult(animeMeg);
    const primaryMeg = megList[0];
    if (!primaryMeg) throw new Error("发送动漫消息失败: 无有效消息");

    const allMsgData = extractAlbumMsgData(megList);
    const allVideoids = allMsgData
        .map((m) => m.videoid)
        .filter((id): id is string => !!id);
    const allUniqueIds = allMsgData
        .map((m) => m.unique_id)
        .filter((id): id is string => !!id);

    const animeLink = await getMessageLink(client, primaryMeg.chat_id, primaryMeg.id);

    await updateAnimeBtdata(
        anime.id,
        undefined,
        combineFansub(item.fansub),
        item.episode || "未知",
        {
            chat_id: primaryMeg.chat_id,
            message_id: primaryMeg.id,
            thread_id:
                primaryMeg.topic_id?._ === "messageTopicForum"
                    ? primaryMeg.topic_id.forum_topic_id
                    : 0,
            link: animeLink.link,
        },
        item.title,
        item.source,
        item.names,
        allVideoids[0],
        allUniqueIds[0],
        Cache_id,
        true,
        allVideoids.length > 1 ? allVideoids : undefined,
        allUniqueIds.length > 1 ? allUniqueIds : undefined,
        allMsgData.length > 1 ? allMsgData : undefined
    );

    const matchResult = matchBangumiEpisode(anime, item.episode);
    if (matchResult.status !== "MATCHED") {
        manager.updateProgress(item.title, "通知管理员确认集数");
        await promptAdminConfirmAnimeEpisodes(client, anime, Cache_id, item, matchResult);
        return;
    }

    manager.updateProgress(item.title, "通知管理员确认番剧信息");
    await promptAdminConfirmAnime(client, anime, matchResult.episodeId, Cache_id, item);
}

/**
 * 处理数据库中已存在番剧的新集数 BT：
 * 1. 匹配 BT 集数与 bangumi 章节 ID
 * 2. 下载 BT 视频文件
 * 3a. 若集数匹配成功：发送到正式动漫频道，更新 btdata 和导航消息
 * 3b. 若集数匹配失败：发送到缓冲频道，通知管理员人工确认集数
 *
 * @param client - TDLib 客户端实例
 * @param item - 已完整解析的动漫 BT 条目
 * @param anime - 数据库中已存在的番剧文档
 * @param manager - 并发处理管理器，用于全程更新进度阶段
 */
export async function handleExistingAnime(
    client: Client,
    item: animeItem,
    anime: animeType,
    manager: AnimeProcessorManager
): Promise<void> {
    const matchResult = matchBangumiEpisode(anime, item.episode);
    await addTorrent(item.magnet, "等待下载", item.title);

    // 解析磁力 hash 以供进度追踪（失败不阻断主流程）
    const magnetHash = tryParseMagnetHash(item.magnet);
    manager.updateProgress(item.title, "下载BT种子", {
        animeName: anime.name_cn || anime.name,
        ...(magnetHash ? { torrentHash: magnetHash } : {}),
    });

    const torrent = await downloadAndValidateTorrent(item);

    // ── 集数匹配失败：视频发到缓冲频道并通知管理员 ──
    if (matchResult.status !== "MATCHED") {
        manager.updateProgress(item.title, "发送视频到缓冲频道（集数待确认）");
        const animeMeg = await sendMegToCache(
            client,
            anime,
            item,
            torrent.content_path,
            torrent.segments
        );
        if (!animeMeg) {
            logger.error("发送动漫消息失败");
            throw new Error("发送动漫消息失败");
        }

        manager.updateProgress(item.title, "更新数据库");
        const cacheMegList = normalizeMsgResult(animeMeg);
        const primaryCacheMeg = cacheMegList[0];
        if (!primaryCacheMeg) throw new Error("发送动漫消息失败: 无有效消息");

        const cacheAllMsgData = extractAlbumMsgData(cacheMegList);
        const cacheAllVideoids = cacheAllMsgData
            .map((m) => m.videoid)
            .filter((id): id is string => !!id);
        const cacheAllUniqueIds = cacheAllMsgData
            .map((m) => m.unique_id)
            .filter((id): id is string => !!id);

        const canimeid = await saveAnime(anime, true);
        const animeLink = await getMessageLink(
            client,
            primaryCacheMeg.chat_id,
            primaryCacheMeg.id
        );
        const Cache_id = await addCacheItem(item);

        await updateAnimeBtdata(
            canimeid,
            undefined,
            combineFansub(item.fansub),
            item.episode || "未知",
            {
                chat_id: primaryCacheMeg.chat_id,
                message_id: primaryCacheMeg.id,
                thread_id:
                    primaryCacheMeg.topic_id?._ === "messageTopicForum"
                        ? primaryCacheMeg.topic_id.forum_topic_id
                        : 0,
                link: animeLink.link,
            },
            item.title,
            item.source,
            item.names,
            cacheAllVideoids[0],
            cacheAllUniqueIds[0],
            Cache_id,
            true,
            cacheAllVideoids.length > 1 ? cacheAllVideoids : undefined,
            cacheAllUniqueIds.length > 1 ? cacheAllUniqueIds : undefined,
            cacheAllMsgData.length > 1 ? cacheAllMsgData : undefined
        );

        await removeTorrentAndData(torrent.hash);
        manager.updateProgress(item.title, "通知管理员确认集数");
        await promptAdminConfirmAnimeEpisodes(client, anime, Cache_id, item, matchResult);
        return;
    }

    // ── 集数匹配成功：视频发到正式动漫频道 ──
    manager.updateProgress(item.title, "发送视频到动漫频道");
    const animeMeg = await sendMegToAnime(
        client,
        anime,
        item,
        torrent.content_path,
        matchResult.episodeId,
        torrent.segments
    );

    if (!animeMeg) {
        await removeTorrentAndData(torrent.hash);
        throw new Error(`发送动漫消息失败: ${item.title}`);
    }
    await removeTorrentAndData(torrent.hash);

    manager.updateProgress(item.title, "更新数据库");
    const animeMegList = normalizeMsgResult(animeMeg);
    const primaryAnimeMeg = animeMegList[0];
    if (!primaryAnimeMeg)
        throw new Error(`发送动漫消息失败: 无有效消息 ${item.title}`);

    const animeAllMsgData = extractAlbumMsgData(animeMegList);
    const animeAllVideoids = animeAllMsgData
        .map((m) => m.videoid)
        .filter((id): id is string => !!id);
    const animeAllUniqueIds = animeAllMsgData
        .map((m) => m.unique_id)
        .filter((id): id is string => !!id);

    const animeLink = await getMessageLink(
        client,
        primaryAnimeMeg.chat_id,
        primaryAnimeMeg.id
    );

    await updateAnimeBtdata(
        anime.id,
        matchResult.episodeId,
        combineFansub(item.fansub),
        item.episode || "未知",
        {
            chat_id: primaryAnimeMeg.chat_id,
            message_id: primaryAnimeMeg.id,
            thread_id:
                primaryAnimeMeg.topic_id?._ === "messageTopicForum"
                    ? primaryAnimeMeg.topic_id.forum_topic_id
                    : 0,
            link: animeLink.link,
        },
        item.title,
        item.source,
        item.names,
        animeAllVideoids[0],
        animeAllUniqueIds[0],
        undefined,
        false,
        animeAllVideoids.length > 1 ? animeAllVideoids : undefined,
        animeAllUniqueIds.length > 1 ? animeAllUniqueIds : undefined,
        animeAllMsgData.length > 1 ? animeAllMsgData : undefined
    );

    manager.updateProgress(item.title, "更新导航消息");
    await sendMegToNavAnime(client, anime.id);
}
