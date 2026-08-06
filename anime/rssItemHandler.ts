import logger from "@log/index.ts";
import { hasTorrentTitle, hasAnimeSend } from "../database/query.ts";
import { extractEpisodeByAI, parseInfo } from "../utils/animeParser.ts";
import { fetchBangumiTags, fetchBangumiTeam, fetchBangumiTorrent } from "./get.ts";
import { handleNewAnime, handleExistingAnime } from "./animeHandlers.ts";
import { checkTorrentFormat } from "../utils/checkTorrentFormat.ts";
import { downloadAndValidateTorrent, removeTorrentAndData } from "../qBittorrent/download.ts";
import { getQBClient } from "../qBittorrent/index.ts";
import type { AnimeProcessorManager } from "./AnimeProcessorManager.ts";
import type { RssAnimeItem, animeItem } from "../types/rss.d.ts";
import type { Client } from "tdl";

/**
 * 处理单个 RSS 动漫条目的完整入口流程：
 * 检查重复 → 提取字幕组 → 按平台解析详情 → 调用 {@link animeDownload} 分发
 *
 * @param client - TDLib 客户端实例
 * @param item - 来自 RSS 源的原始动漫条目
 * @param manager - 并发处理管理器，用于全程更新进度阶段
 */
export async function handleRssAnimeItem(
    client: Client,
    item: RssAnimeItem,
    manager: AnimeProcessorManager
): Promise<void> {
    manager.updateProgress(item.title, "检查种子缓存");

    const torrentExists = await hasTorrentTitle(item.title);
    if (torrentExists) return;

    // 从标题开头提取字幕组名称（支持 [SubGroup] 和 【SubGroup】 两种格式）
    const match = item.title.match(/^(?:\[([^\]]+)]|【([^】]+)】)/);
    if (!match) return;

    const raw = match[1] || match[2] || '';
    const fansub = raw
        .split(/\s*[&/|｜、]\s*/)
        .map((s) => s.trim())
        .filter(Boolean);

    if (!fansub || fansub.length === 0) return;

    manager.updateProgress(item.title, "解析RSS信息");

    let newitem: animeItem | undefined;

    if (item.type === "bangumi") {
        newitem = await parseBangumiItem(item, fansub, manager);
    } else if (item.type === "dmhy" || item.type === "acgnx") {
        newitem = await parseDmhyOrAcgnxItem(item, fansub);
    } else {
        return;
    }

    if (!newitem) return;

    // 预检查种子格式：若为 MKV（需烧录字幕），路由到独立的 MKV 处理队列
    // 避免 MKV 下载+转码过程中长时间占用普通 worker 槽位
    if (newitem.magnet) {
        manager.updateProgress(item.title, "检查种子格式");
        const format = await checkTorrentFormat(newitem.magnet);
        if (format === "mkv") {
            manager.updateProgress(item.title, "路由到MKV队列");
            await manager.enqueueMkv(client, newitem);
            return;
        }

        // 格式预检失败（超时等），回退到下载完成后判断
        if (format === "unknown") {
            manager.updateProgress(item.title, "格式预检超时，下载后判断");
            const onStage = (stage: string) =>
                manager.updateProgress(item.title, stage, { animeName: newitem.names[0] });
            const torrent = await downloadAndValidateTorrent(newitem, manager, onStage, true);
            if (!torrent) return;

            if (torrent.isMkv) {
                // MKV：这里已经下载完成并暂停了种子。
                // 若直接重新入队，MKV worker 会再次 downloadAndReturnPath 命中同一种子，
                // 而该种子处于暂停状态、不会进入 seeding 状态，导致等待死循环，
                // 最终种子与视频都无法清理。
                // 因此先删除已暂停的种子（保留文件，避免重复下载 MKV），
                // MKV worker 重新添加后会自动 recheck 已存在的文件并继续烧录流程。
                const QBclientForMkv = await getQBClient();
                await QBclientForMkv.deleteTorrent(torrent.hash, false).catch(() => { });
                manager.updateProgress(item.title, "下载完成（MKV），路由到MKV队列");
                await manager.enqueueMkv(client, newitem);
                return;
            }
            // 非 MKV：删除种子（保留文件），走正常主线程流程
            const QBclient = await getQBClient();
            await QBclient.deleteTorrent(torrent.hash, false).catch(() => { });
        }
    }

    await animeDownload(client, newitem, manager);
}

/** 导出给 AnimeProcessorManager 的 MKV worker 使用 */
export { animeDownload };

/**
 * 解析 bangumi.moe 类型的 RSS 条目，补充字幕组/标签/多语言名称信息
 *
 * @param item - bangumi.moe RSS 原始条目
 * @param fansub - 从标题提取到的字幕组列表
 * @param manager - 并发处理管理器，用于更新进度阶段
 * @returns 完整填充的 {@link animeItem}，解析失败时返回 undefined
 */
async function parseBangumiItem(
    item: RssAnimeItem & { type: "bangumi" },
    fansub: string[],
    manager: AnimeProcessorManager
): Promise<animeItem | undefined> {
    manager.updateProgress(item.title, "获取Bangumi详情");

    const torrentInfo = await fetchBangumiTorrent(item.id);

    let team: { name?: string }[] = [];
    if (torrentInfo.team_id) {
        team = await fetchBangumiTeam(torrentInfo.team_id);
    } else {
        team = [{ name: fansub[0] }];
    }

    const tags =
        torrentInfo.tag_ids && torrentInfo.tag_ids.length > 0
            ? await fetchBangumiTags(torrentInfo.tag_ids)
            : [];

    // 从 tags 中找到 type === "bangumi" 的条目并提取多语言名称
    const bangumiTag = tags.find(
        (tag: {
            type?: string;
            locale?: { zh_cn?: string; ja?: string; en?: string };
        }) => tag.type === "bangumi"
    );

    const nameLocales = bangumiTag
        ? {
            cn: bangumiTag.locale.zh_cn || "",
            jp: bangumiTag.locale.ja || "",
            en: bangumiTag.locale.en || "",
        }
        : { cn: "", jp: "", en: "" };

    const infoq = parseInfo(item.title, team[0]?.name ?? null);
    if (!infoq) return undefined;

    if (!infoq.episode || infoq.episode === "未知") {
        const aiEpisode = await extractEpisodeByAI(item.title, infoq.names);
        if (aiEpisode) {
            infoq.episode = aiEpisode;
        }
    }

    // 将多语言名合并进 infoq.names，去重去空
    const localeNames = [nameLocales.cn, nameLocales.jp, nameLocales.en]
        .filter((s) => typeof s === "string" && s.trim() !== "")
        .map((s) => s.trim());

    infoq.names = Array.isArray(infoq.names)
        ? infoq.names.map((s) => (typeof s === "string" ? s.trim() : "")).filter(Boolean)
        : [];
    infoq.names = Array.from(new Set([...infoq.names, ...localeNames])).filter(Boolean);

    // 白名单机制：names 为空则跳过
    if (!infoq.names || infoq.names.length === 0 || !team[0]?.name) return undefined;

    return {
        title: item.title,
        pubDate: item.pubDate,
        link: item.link,
        magnet: torrentInfo.magnet,
        team: team[0]?.name ?? fansub[0],
        fansub,
        ...infoq,
    };
}

/**
 * 解析动漫花园（dmhy）或末日动漫（acgnx）类型的 RSS 条目
 *
 * @param item - dmhy / acgnx RSS 原始条目
 * @param fansub - 从标题提取到的字幕组列表
 * @returns 完整填充的 {@link animeItem}，标题解析失败时返回 undefined
 */
async function parseDmhyOrAcgnxItem(
    item: RssAnimeItem & { type: "dmhy" | "acgnx" },
    fansub: string[]
): Promise<animeItem | undefined> {
    const infoq = parseInfo(item.title, item.author);
    if (!infoq) return undefined;

    if (!infoq.episode || infoq.episode === "未知") {
        const aiEpisode = await extractEpisodeByAI(item.title, infoq.names);
        if (aiEpisode) {
            infoq.episode = aiEpisode;
        }
    }

    return {
        title: item.title,
        pubDate: item.pubDate,
        magnet: item.magnet,
        link: item.link,
        team: item.author,
        fansub,
        ...infoq,
    };
}

/**
 * 根据数据库查询结果，将条目分发给新番处理器或已有番处理器
 *
 * @param client - TDLib 客户端实例
 * @param item - 已解析的动漫 BT 条目
 * @param manager - 并发处理管理器，用于更新进度阶段
 */
async function animeDownload(
    client: Client,
    item: animeItem,
    manager: AnimeProcessorManager
): Promise<void> {
    manager.updateProgress(item.title, "查询数据库", { animeName: item.names[0] });

    const anime = await hasAnimeSend(item.names);

    if (!anime) {
        logger.info(`✨ 发现潜在新番: ${item.title}`);
        manager.updateProgress(item.title, "处理新番剧", { animeName: item.names[0] });
        await handleNewAnime(client, item, manager);
        return;
    }

    logger.info(`找到匹配的番剧，准备下载: ${item.title}`);
    manager.updateProgress(item.title, "处理已有番剧", {
        animeName: anime.name_cn || anime.name,
    });
    await handleExistingAnime(client, item, anime, manager);
}
