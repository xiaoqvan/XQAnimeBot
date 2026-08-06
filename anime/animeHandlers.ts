import logger from "@log/index.ts";
import parseTorrent from "parse-torrent";
import { animeinfo } from "../bangumi/get.ts";
import { saveAnimeResource } from "../database/update.ts";
import { addCacheItem, addTorrent, saveAnime, createPendingReview } from "../database/create.ts";
import { getAnimeById, getEpisodeMetasBySubjectId } from "../database/query.ts";
import { getMessageLink } from "@TDLib/function/get.ts";
import { sendMegToAnime, sendMegToCache, sendMegToNavAnime } from "./sendAnime.ts";
import { buildAndSaveAnimeFromInfo } from "../utils/buildAnimeinfo.ts";
import { combineFansub } from "../utils/index.ts";
import { downloadAndValidateTorrent, removeTorrentAndData } from "../qBittorrent/download.ts";
import { matchBangumiEpisode } from "../utils/matcher.ts";
import { matchAnimeSubject } from "../bangumi/bangumiAgent.ts";
import type { MatchResult } from "../bangumi/bangumiAgent.ts";
import {
    promptAdminProvideAnimeInfo,
    promptAdminConfirmAnimeEpisodes,
    promptAdminConfirmAnimeWithCandidates,
} from "./adminPrompts.ts";
import type { AnimeProcessorManager } from "./AnimeProcessorManager.ts";
import type { anime as animeType } from "../types/anime.d.ts";
import type { animeItem } from "../types/rss.d.ts";
import type { albumMessageType } from "../types/message.d.ts";
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
 *
 * 【先匹配发送，后审核】策略:
 * 1. 使用 matchAnimeSubject（LLM Agent）匹配番剧条目
 * 2. 若匹配置信度高：直接下载、发送到正式动漫频道，然后通知管理员审核
 * 3. 若匹配置信度低或无匹配：回退到原有流程（搜索→下载→发缓冲频道→审核）
 *
 * 【可溯源纠正】:
 * - 审核消息中携带匹配详情（候选列表、置信度、原因），管理员可通过回调纠正
 * - 纠正时使用 rebindCacheResourceAnime 迁移资源归属，并更新双条目导航消息
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
    // 先创建缓存条目和种子记录，防止并发时重复处理同一条目
    const Cache_id = await addCacheItem(item);
    await addTorrent(item.magnet, "等待下载", item.title);

    // ── 第一阶段：使用 LLM Agent 匹配番剧（matchAnimeSubject）──
    manager.updateProgress(item.title, "LLM匹配番剧");
    const matchResult: MatchResult = await matchAnimeSubject(
        { title: item.title, names: item.names, episode: item.episode },
        5, // llmThreshold: 候选超过5个时触发 LLM 决策
    );

    // ── 匹配置信度高 (≥0.9)：先发送后审核 ──
    if (
        matchResult.confidence >= 0.9 &&
        matchResult.subjectId !== undefined
    ) {
        await handleNewAnimeWithConfidentMatch(
            client,
            item,
            matchResult,
            Cache_id,
            manager,
        );
        return;
    }

    // ── 匹配置信度不足：回退到原有流程 ──
    await handleNewAnimeFallback(client, item, matchResult, Cache_id, manager);
}

/**
 * 高置信度匹配路径：直接发送到正式频道，然后通知管理员审核。
 *
 * 关键在于"先发送后审核"——视频已经在动漫频道可见，管理员随后确认/纠正。
 * 纠正时可以溯源到正确的条目并更新导航消息。
 *
 * @param client - TDLib 客户端实例
 * @param item - 已完整解析的动漫 BT 条目
 * @param matchResult - matchAnimeSubject 返回的匹配结果（confidence ≥ 0.9）
 * @param Cache_id - 缓存条目 ID
 * @param manager - 并发处理管理器
 */
async function handleNewAnimeWithConfidentMatch(
    client: Client,
    item: animeItem,
    matchResult: MatchResult,
    Cache_id: number,
    manager: AnimeProcessorManager,
): Promise<void> {
    const subjectId = matchResult.subjectId!;

    // 查询 Bangumi 获取完整番剧信息并保存
    const { getSubjectById } = await import("../bangumi/get.ts");
    const subject = await getSubjectById(subjectId);
    const anime = await buildAndSaveAnimeFromInfo(subject, false, item.names);

    // ── 集数匹配：优先使用 LLM Agent 返回的 episodeId ──
    // 如果 Agent 已经精确匹配到章节 ID，直接使用，无需再调用 matchBangumiEpisode
    let episodeId: number | undefined;
    let episodeMatch: Awaited<ReturnType<typeof matchBangumiEpisode>> | undefined;

    if (matchResult.episodeId !== undefined) {
        episodeId = matchResult.episodeId;
    } else {
        const episodeMetas = await getEpisodeMetasBySubjectId(anime.id);
        episodeMatch = await matchBangumiEpisode(anime, episodeMetas, item.episode);
        if (episodeMatch.status === "MATCHED") {
            episodeId = episodeMatch.episodeId;
        }
    }

    // 解析磁力 hash 以供进度追踪
    const magnetHash = tryParseMagnetHash(item.magnet);
    manager.updateProgress(item.title, "下载BT种子（高置信度匹配）", {
        animeName: anime.name_cn || anime.name,
        ...(magnetHash ? { torrentHash: magnetHash } : {}),
    });

    const onStage = (stage: string) =>
        manager.updateProgress(item.title, stage, { animeName: anime.name_cn || anime.name });
    const torrent = await downloadAndValidateTorrent(item, manager, onStage);
    if (!torrent) return;

    // 无论后续发送/写库成功或失败，都必须清理 qBittorrent 种子及其数据，
    // 避免异常导致种子残留、磁盘被堆满
    try {
        if (!episodeId) {
            // 集数未匹配成功：使用 matchResult.episodeId 或 episodeMatch 判定
            const matchResultForPrompt = episodeMatch ?? {
                status: "NOT_FOUND_IN_DB" as const,
                msg: matchResult.reason || "Agent 未能匹配到集数",
            };
            // 集数匹配失败：仍发送到缓冲频道让管理员确认集数
            manager.updateProgress(item.title, "发送视频到缓冲频道（集数待确认）");
            const animeMeg = await sendMegToCache(
                client, anime, item, torrent.content_path, torrent.segments,
            );

            if (!animeMeg) throw new Error("发送动漫消息失败");

            const megList = normalizeMsgResult(animeMeg);
            const primaryMeg = megList[0];
            if (!primaryMeg) throw new Error("发送动漫消息失败: 无有效消息");

            const allMsgData = extractAlbumMsgData(megList);
            const allVideoids = allMsgData.map(m => m.videoid).filter((id): id is string => !!id);
            const allUniqueIds = allMsgData.map(m => m.unique_id).filter((id): id is string => !!id);

            const animeLink = await getMessageLink(client, primaryMeg.chat_id, primaryMeg.id);
            await saveAnimeResource(
                anime.id, undefined, combineFansub(item.fansub),
                item.episode || "未知",
                {
                    chat_id: primaryMeg.chat_id, message_id: primaryMeg.id,
                    thread_id: primaryMeg.topic_id?._ === "messageTopicForum"
                        ? primaryMeg.topic_id.forum_topic_id : 0,
                    link: animeLink.link,
                },
                item.title, item.source, item.names,
                allVideoids[0], allUniqueIds[0], Cache_id, true,
                allVideoids.length > 1 ? allVideoids : undefined,
                allUniqueIds.length > 1 ? allUniqueIds : undefined,
                allMsgData.length > 1 ? allMsgData : undefined,
            );

            manager.updateProgress(item.title, "通知管理员确认集数");
            await promptAdminConfirmAnimeEpisodes(client, anime, Cache_id, item, matchResultForPrompt);
            return;
        }

        // ── 集数匹配成功：先创建导航消息，再发送视频到正式频道 ──
        // 先发导航消息，确保 AnimeText 生成视频 caption 时 navMessage.link 有值，追踪标记正常显示
        manager.updateProgress(item.title, "创建导航消息");
        await sendMegToNavAnime(client, anime.id);

        // 重新读取一次，确保 sendMegToAnime 使用的 anime 对象包含最新 navMessage.link
        const animeAfterNav = (await getAnimeById(anime.id)) ?? anime;

        // 再发送视频到动漫频道（此时 navMessage.link 已存在，追踪标记会正常显示）
        manager.updateProgress(item.title, "发送视频到动漫频道（高置信度）");
        const animeMeg = await sendMegToAnime(
            client, animeAfterNav, item, torrent.content_path,
            episodeId, torrent.segments,
        );

        if (!animeMeg) throw new Error(`发送动漫消息失败: ${item.title}`);

        manager.updateProgress(item.title, "更新数据库");
        const megList = normalizeMsgResult(animeMeg);
        const primaryMeg = megList[0];
        if (!primaryMeg) throw new Error("发送动漫消息失败: 无有效消息");

        const allMsgData = extractAlbumMsgData(megList);
        const allVideoids = allMsgData.map(m => m.videoid).filter((id): id is string => !!id);
        const allUniqueIds = allMsgData.map(m => m.unique_id).filter((id): id is string => !!id);

        const animeLink = await getMessageLink(client, primaryMeg.chat_id, primaryMeg.id);
        await saveAnimeResource(
            anime.id, episodeId, combineFansub(item.fansub),
            item.episode || "未知",
            {
                chat_id: primaryMeg.chat_id, message_id: primaryMeg.id,
                thread_id: primaryMeg.topic_id?._ === "messageTopicForum"
                    ? primaryMeg.topic_id.forum_topic_id : 0,
                link: animeLink.link,
            },
            item.title, item.source, item.names,
            allVideoids[0], allUniqueIds[0], Cache_id, false,
            allVideoids.length > 1 ? allVideoids : undefined,
            allUniqueIds.length > 1 ? allUniqueIds : undefined,
            allMsgData.length > 1 ? allMsgData : undefined,
        );

        // 再次更新导航消息，补充新发送的资源条目
        manager.updateProgress(item.title, "更新导航消息（补充资源）");
        await sendMegToNavAnime(client, anime.id);

        // ── 创建待审核记录 ──
        manager.updateProgress(item.title, "创建待审核记录");
        const pendingReviewId = await createPendingReview({
            item,
            anime: { ...anime, names: anime.names ?? [] },
            episodeId,
            episodeSort: episodeId, // 由 prompt 内部根据 episodeId 查 sort，这里先用 episodeId 占位
            sentMessages: allMsgData,
            primaryMessage: {
                chat_id: primaryMeg.chat_id,
                message_id: primaryMeg.id,
            },
            matchDetail: matchResult,
        });

        // ── 后审核：通知管理员确认（只携带待审核 ID）──
        manager.updateProgress(item.title, "通知管理员审核（后审核）");
        await promptAdminConfirmAnimeWithCandidates(
            client, anime, episodeId, Cache_id, item,
            matchResult, pendingReviewId, true, // isPostSend: 先发后审模式
        );
    } finally {
        // 无论成功失败都清理 qBittorrent 种子及其数据
        await removeTorrentAndData(torrent.hash);
    }
}

/**
 * 低置信度 / 无匹配回退路径：沿用原有流程（搜索 → 缓冲频道 → 审核）。
 *
 * @param client - TDLib 客户端实例
 * @param item - 已完整解析的动漫 BT 条目
 * @param matchResult - matchAnimeSubject 返回的匹配结果（confidence < 0.9 或无匹配）
 * @param Cache_id - 缓存条目 ID
 * @param manager - 并发处理管理器
 */
async function handleNewAnimeFallback(
    client: Client,
    item: animeItem,
    matchResult: MatchResult,
    Cache_id: number,
    manager: AnimeProcessorManager,
): Promise<void> {
    manager.updateProgress(item.title, "搜索番剧信息（回退路径）");

    // 如果 matchAnimeSubject 给出了 subjectId 但置信度不足，优先尝试直接用该 ID
    let searchAnime;
    if (matchResult.subjectId !== undefined && matchResult.confidence > 0) {
        const { getSubjectById } = await import("../bangumi/get.ts");
        try {
            const subject = await getSubjectById(matchResult.subjectId);
            searchAnime = { data: [subject] };
            logger.info(
                `[handleNewAnime] 回退路径使用 matchAnimeSubject subjectId=${matchResult.subjectId}, confidence=${matchResult.confidence}`,
            );
        } catch {
            // 用 subjectId 获取失败，走普通搜索
        }
    }

    if (!searchAnime) {
        searchAnime = await animeinfo(item.names[0]!);
    }

    if (!searchAnime.data || searchAnime.data.length === 0) {
        manager.updateProgress(item.title, "通知管理员提供番剧信息");
        promptAdminProvideAnimeInfo(client, Cache_id, item);
        return;
    }

    const anime = await buildAndSaveAnimeFromInfo(
        searchAnime.data[0]!, true, item.names,
    );

    const magnetHash = tryParseMagnetHash(item.magnet);
    manager.updateProgress(item.title, "下载BT种子（回退路径）", {
        animeName: anime.name_cn || anime.name,
        ...(magnetHash ? { torrentHash: magnetHash } : {}),
    });

    const onStage = (stage: string) =>
        manager.updateProgress(item.title, stage, { animeName: anime.name_cn || anime.name });
    const torrent = await downloadAndValidateTorrent(item, manager, onStage);
    if (!torrent) return;

    // 无论后续发送/写库成功或失败，都必须在 finally 中清理种子，避免残留
    try {
        manager.updateProgress(item.title, "发送视频到缓冲频道");
        const animeMeg = await sendMegToCache(
            client, anime, item, torrent.content_path, torrent.segments,
        );

        if (!animeMeg) throw new Error("发送动漫消息失败");

        manager.updateProgress(item.title, "更新数据库");
        const megList = normalizeMsgResult(animeMeg);
        const primaryMeg = megList[0];
        if (!primaryMeg) throw new Error("发送动漫消息失败: 无有效消息");

        const allMsgData = extractAlbumMsgData(megList);
        const allVideoids = allMsgData.map(m => m.videoid).filter((id): id is string => !!id);
        const allUniqueIds = allMsgData.map(m => m.unique_id).filter((id): id is string => !!id);

        const animeLink = await getMessageLink(client, primaryMeg.chat_id, primaryMeg.id);
        await saveAnimeResource(
            anime.id, undefined, combineFansub(item.fansub),
            item.episode || "未知",
            {
                chat_id: primaryMeg.chat_id, message_id: primaryMeg.id,
                thread_id: primaryMeg.topic_id?._ === "messageTopicForum"
                    ? primaryMeg.topic_id.forum_topic_id : 0,
                link: animeLink.link,
            },
            item.title, item.source, item.names,
            allVideoids[0], allUniqueIds[0], Cache_id, true,
            allVideoids.length > 1 ? allVideoids : undefined,
            allUniqueIds.length > 1 ? allUniqueIds : undefined,
            allMsgData.length > 1 ? allMsgData : undefined,
        );

        const episodeMetas = await getEpisodeMetasBySubjectId(anime.id);

        // 优先使用 Agent 返回的 episodeId，否则用 matchBangumiEpisode 匹配
        let episodeId: number | undefined = matchResult.episodeId;
        let epMatch: Awaited<ReturnType<typeof matchBangumiEpisode>> | undefined;

        if (!episodeId) {
            epMatch = await matchBangumiEpisode(anime, episodeMetas, item.episode);
            if (epMatch.status === "MATCHED") {
                episodeId = epMatch.episodeId;
            }
        }

        if (!episodeId) {
            const matchResultForPrompt = epMatch ?? {
                status: "NOT_FOUND_IN_DB" as const,
                msg: matchResult.reason || "Agent 未能匹配到集数",
            };
            manager.updateProgress(item.title, "通知管理员确认集数");
            await promptAdminConfirmAnimeEpisodes(client, anime, Cache_id, item, matchResultForPrompt);
            return;
        }

        manager.updateProgress(item.title, "通知管理员确认番剧信息");
        await promptAdminConfirmAnimeWithCandidates(
            client, anime, episodeId, Cache_id, item,
            matchResult,
        );
    } finally {
        // 无论成功失败都清理 qBittorrent 种子及其数据
        await removeTorrentAndData(torrent.hash);
    }
}

/**
 * 处理数据库中已存在番剧的新集数 BT：
 * 1. 匹配 BT 集数与 bangumi 章节 ID
 * 2. 下载 BT 视频文件
 * 3a. 若集数匹配成功：发送到正式动漫频道，更新资源记录和导航消息
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
    const episodeMetas = await getEpisodeMetasBySubjectId(anime.id);
    const matchResult = await matchBangumiEpisode(anime, episodeMetas, item.episode);
    await addTorrent(item.magnet, "等待下载", item.title);

    // 解析磁力 hash 以供进度追踪（失败不阻断主流程）
    const magnetHash = tryParseMagnetHash(item.magnet);
    manager.updateProgress(item.title, "下载BT种子", {
        animeName: anime.name_cn || anime.name,
        ...(magnetHash ? { torrentHash: magnetHash } : {}),
    });

    const onStage = (stage: string) =>
        manager.updateProgress(item.title, stage, { animeName: anime.name_cn || anime.name });
    const torrent = await downloadAndValidateTorrent(item, manager, onStage);
    if (!torrent) {
        logger.warn(`下载种子失败，跳过: ${item.title}`);
        return;
    }

    // 无论后续发送/写库成功或失败，都必须在 finally 中清理种子，避免残留堆满磁盘
    try {
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

            // 将 RSS 解析的名称合并进缓存，避免下次去重失败
            const animeForCache = {
                ...anime,
                names: [
                    ...new Set(
                        [
                            ...(anime.names || []),
                            ...(item.names || []),
                        ].filter((n): n is string => typeof n === "string" && n.trim().length > 0)
                    ),
                ] as string[],
            };
            const canimeid = await saveAnime(animeForCache, true);
            const animeLink = await getMessageLink(
                client,
                primaryCacheMeg.chat_id,
                primaryCacheMeg.id
            );
            const Cache_id = await addCacheItem(item);

            await saveAnimeResource(
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
            throw new Error(`发送动漫消息失败: ${item.title}`);
        }

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

        await saveAnimeResource(
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
    } finally {
        // 无论成功失败都清理 qBittorrent 种子及其数据
        await removeTorrentAndData(torrent.hash);
    }
}
