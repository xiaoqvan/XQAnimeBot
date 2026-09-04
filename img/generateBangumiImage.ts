/**
 * Bangumi 导航封面图片生成模块
 *
 * 从 img/index.ts 中提取的通用图片生成逻辑，复用 bangumi.vue 模板。
 * 供 sendMegToNavAnime 在更新导航消息时调用，生成带有评分/章节/Staff等信息的封面图。
 *
 * 特性：
 * - 数据尽量使用本地数据库已有数据，减少 API 调用
 * - 计算哈希去重：相同内容不重复生成图片，直接使用缓存的 file_id
 * - 复用 img_cache 集合，与 /bangumi 命令共享缓存
 */

import fs from "fs/promises";
import { generateImage } from "@function/genImg.ts";
import { getImgCache } from "@db/query.ts";
import crypto from "crypto";
import logger from "@log/index.ts";
import type { bangumiAnime } from "../types/bangumi.d.ts";
import type { anime as animeType } from "../types/anime.d.ts";

// ─── 内部类型（与 img/index.ts 一致） ───────────────────────────────────────

interface BangumiEpisode {
    id: number;
    ep: number;
    sort: number;
    name: string;
    name_cn: string;
    airdate: string;
    type: number;
}

interface EpisodeInfo {
    total: number;
    data: {
        airdate: string;
        name: string;
        name_cn: string;
        ep: number;
        sort: number;
        id: number;
        type: number;
        [key: string]: unknown;
    }[];
}

// ─── 工具函数（从 img/index.ts 提取） ──────────────────────────────────────

const STAFF_KEYS = ["原作", "导演", "动画制作", "音乐", "系列构成", "脚本"];

type InfoboxItem = { key: string; value: string | { v: string }[] };

function getInfoboxMap(infobox: bangumiAnime["infobox"]): Record<string, string> {
    const map: Record<string, string> = {};
    if (!Array.isArray(infobox)) return map;
    for (const item of infobox) {
        const typed = item as InfoboxItem;
        if (typeof typed.value === "string") {
            map[typed.key] = typed.value;
        } else if (Array.isArray(typed.value)) {
            map[typed.key] = typed.value.map((v) => v.v).join(" / ");
        }
    }
    return map;
}

function getDisplayStaff(infoboxMap: Record<string, string>): { key: string; value: string; align?: string }[] {
    return STAFF_KEYS.map((key, idx) => ({
        key,
        value: infoboxMap[key] || "",
        align: idx % 2 === 1 ? "right" : undefined,
    }));
}

function getRatingLabel(score: number): string {
    if (score >= 8.5) return "神作";
    if (score >= 7.5) return "力荐";
    if (score >= 6.5) return "推荐";
    if (score >= 5.5) return "还行";
    if (score >= 4.5) return "不过不失";
    if (score >= 3.5) return "较差";
    if (score >= 2.5) return "差";
    if (score >= 1.5) return "很差";
    return "不忍直视";
}

/** 从 episodeList 中计算已播出集数（基于北京时间今天） */
function computeAiredEpisodes(episodes: BangumiEpisode[]): {
    mainEpisodes: BangumiEpisode[];
    otherEpisodes: { type: number; label: string; episodes: BangumiEpisode[] }[];
    airedCount: number;
    currentEpisode: BangumiEpisode | null;
    nextEpisode: BangumiEpisode | null;
} {
    const EPISODE_TYPE_LABELS: Record<number, string> = {
        1: "SP", 2: "OP", 3: "ED", 4: "预告", 5: "MAD", 6: "其他",
    };

    const mainEpisodes = episodes.filter((ep) => ep.type === 0);
    const rawOtherEpisodes = episodes.filter((ep) => ep.type !== 0);

    const otherGroups = (() => {
        const groups = new Map<number, BangumiEpisode[]>();
        for (const ep of rawOtherEpisodes) {
            const list = groups.get(ep.type) ?? [];
            list.push(ep);
            groups.set(ep.type, list);
        }
        return Array.from(groups.entries())
            .sort(([a], [b]) => a - b)
            .map(([type, eps]) => ({
                type,
                label: EPISODE_TYPE_LABELS[type] || `类型${type}`,
                episodes: eps,
            }));
    })();

    const todayStr = new Date().toISOString().slice(0, 10).replace(/-/g, "");
    const airedEps = mainEpisodes.filter((ep) => {
        if (!ep.airdate || !ep.airdate.trim()) return false;
        return ep.airdate.replace(/-/g, "") <= todayStr;
    });
    const airedCount = airedEps.length;
    const currentEp = airedEps.length > 0
        ? airedEps.sort((a, b) => b.sort - a.sort)[0]
        : null;
    const nextEp = currentEp
        ? mainEpisodes.find((ep) => ep.sort > currentEp.sort)
        : mainEpisodes.find((ep) => ep.airdate && ep.airdate.trim());

    return {
        mainEpisodes,
        otherEpisodes: otherGroups,
        airedCount,
        currentEpisode: currentEp ?? null,
        nextEpisode: nextEp ?? null,
    };
}

// ─── 公开接口 ───────────────────────────────────────────────────────────────

export interface GenerateBangumiImageInput {
    /** Bangumi API 返回的条目完整信息（通过 getSubjectById 获取） */
    subjectData: bangumiAnime;
    /** 集数信息（通过 getEpisodeInfo 获取），可选，为空时不显示章节网格 */
    episodeInfo?: EpisodeInfo | null;
    /** 本地数据库中的番剧信息（用于补充 tags/names 等） */
    anime?: animeType | null;
    /** 活跃字幕组列表（从 resources 集合提取），将在图片标签下方显示 */
    fansubs?: string[];
}

export interface GenerateBangumiImageResult {
    /** 本地图片路径（新生成的），未生成时为 undefined */
    path?: string;
    /** 图片内容哈希，用于去重判断 */
    hash: string;
    /** Telegram 缓存的 file_id（命中缓存时存在） */
    file_id?: string;
}

/**
 * 生成 Bangumi 导航封面图片
 *
 * 使用 bangumi.vue 模板渲染 SVG，转 PNG/JPG 后输出。
 * 内置哈希去重：相同的评分/章节/staff 数据不会重复生成图片，
 * 而是直接返回缓存的 Telegram file_id。
 *
 * @param input - 包含 subjectData、episodeInfo、anime 的输入数据
 * @returns 生成结果，包含 path / hash / file_id
 */
export async function generateBangumiNavImage(
    input: GenerateBangumiImageInput,
): Promise<GenerateBangumiImageResult> {
    const { subjectData, episodeInfo, anime, fansubs: rawFansubs } = input;

    // ── 过滤不需要在封面图片中显示的字幕组（含 source 后缀变体） ──
    const EXCLUDED_FANSUB_PREFIXES = ["ANi", "黒ネズミたち"];
    const fansubs = (rawFansubs ?? []).filter(
        (f) => !EXCLUDED_FANSUB_PREFIXES.some((prefix) => f === prefix || f.startsWith(prefix + "_")),
    );

    // ── 1. 构建 Vue 模板 props ──
    const infoboxMap = getInfoboxMap(subjectData.infobox || []);
    const displayStaff = getDisplayStaff(infoboxMap);
    const score = subjectData.rating?.score ?? 0;
    const ratingLabel = getRatingLabel(score);
    const ratingEmojiIdx = score >= 8.5 ? 4 : score >= 6.5 ? 3 : score >= 5.5 ? 2 : score >= 3.5 ? 1 : 0;
    const generatedAt = new Date().toISOString().slice(0, 10).replace(/-/g, "/");

    // ── 2. 构建章节数据 ──
    let mainEpisodes: BangumiEpisode[] = [];
    let otherEpisodes: { type: number; label: string; episodes: BangumiEpisode[] }[] = [];
    let airedCount = 0;
    let currentEpNum = 0;
    let currentEpName = "";
    let currentEpAirDate = "";
    let nextEpNum = 0;
    let nextEpName = "";
    let nextEpAirDate = "";

    if (episodeInfo?.data && Array.isArray(episodeInfo.data)) {
        const episodes = episodeInfo.data.map((ep) => ({
            id: ep.id,
            ep: ep.ep,
            sort: ep.sort,
            name: ep.name,
            name_cn: ep.name_cn,
            airdate: ep.airdate,
            type: ep.type,
        }));

        const result = computeAiredEpisodes(episodes);
        mainEpisodes = result.mainEpisodes;
        otherEpisodes = result.otherEpisodes;
        airedCount = result.airedCount;
        currentEpNum = result.currentEpisode?.sort ?? 0;
        currentEpName = result.currentEpisode?.name_cn || result.currentEpisode?.name || "";
        currentEpAirDate = result.currentEpisode?.airdate || "";
        nextEpNum = result.nextEpisode?.sort ?? 0;
        nextEpName = result.nextEpisode?.name_cn || result.nextEpisode?.name || "";
        nextEpAirDate = result.nextEpisode?.airdate || "";
    }

    // ── 3. 计算柱状图百分比 ──
    const ratingCounts = subjectData.rating?.count || {};
    const maxCount = Math.max(1, ...Object.values(ratingCounts));
    const barPcts: number[] = [];
    for (let i = 10; i >= 1; i--) {
        barPcts.push(Math.max(3, Math.round(((ratingCounts[i as keyof typeof ratingCounts] || 0) / maxCount) * 100)));
    }

    // ── 4. 收藏统计 ──
    const col = subjectData.collection || { on_hold: 0, dropped: 0, wish: 0, collect: 0, doing: 0 };
    const fmtNum = (n: number) => (n >= 1000 ? (n / 1000).toFixed(1) + "K" : String(n));

    // ── 5. 标签（优先使用本地数据，否则用 API 数据） ──
    const tags = (anime?.tags?.length ? anime.tags : (subjectData.tags || []).map((t) => t.name)).slice(0, 12);

    // ── 6. 分页计算 ──
    const episodesPerPage = 30;
    const activePageIdx = currentEpNum > 0
        ? Math.min(
            Math.floor((currentEpNum - 1) / episodesPerPage),
            Math.ceil(mainEpisodes.length / episodesPerPage) - 1,
        )
        : 0;

    // ── 7. 读取 rate_emo.gif（spritesheet） ──
    let rateEmoDataUri = "";
    let rateEmoW = 170;
    let rateEmoH = 34;
    try {
        const rateEmoPath = new URL("rate_emo.gif", import.meta.url);
        const rateEmoBuffer = await fs.readFile(rateEmoPath);
        rateEmoDataUri = `data:image/gif;base64,${rateEmoBuffer.toString("base64")}`;
        const meta = await (await import("sharp")).default(rateEmoBuffer).metadata();
        if (meta.width && meta.height) {
            rateEmoW = meta.width;
            rateEmoH = meta.height;
        }
    } catch {
        // rate_emo.gif 不存在时不阻塞流程
    }

    // ── 8. 构建 props ──
    const props = {
        id: subjectData.id,
        name: subjectData.name || "",
        name_cn: subjectData.name_cn || "",
        images: subjectData.images || {},
        date: subjectData.date || "",
        platform: subjectData.platform || "",
        total_episodes: subjectData.total_episodes || 0,
        rating: subjectData.rating || { rank: 0, total: 0, count: {}, score: 0 },
        tags: tags.map((t: string | { name: string; count?: number }) =>
            typeof t === "string" ? { name: t, count: 0 } : t,
        ).slice(0, 12),
        infoboxMap,
        collection: {
            on_hold: col.on_hold ?? 0,
            dropped: col.dropped ?? 0,
            wish: col.wish ?? 0,
            collect: col.collect ?? 0,
            doing: col.doing ?? 0,
            on_hold_fmt: fmtNum(col.on_hold ?? 0),
            wish_fmt: fmtNum(col.wish ?? 0),
            collect_fmt: fmtNum(col.collect ?? 0),
            doing_fmt: fmtNum(col.doing ?? 0),
        },
        displayStaff,
        ratingLabel,
        ratingEmojiIdx,
        generatedAt,
        rateEmoDataUri,
        rateEmoW,
        rateEmoH,
        barPcts,
        episodes: mainEpisodes,
        otherEpisodes,
        airedCount,
        episodesPerPage,
        activePageIdx,
        currentEpNum,
        currentEpName,
        currentEpAirDate,
        nextEpNum,
        nextEpName,
        nextEpAirDate,
        fansubs: fansubs ?? [],
    };

    // ── 9. 计算内容哈希（用于去重） ──
    const hashInput = JSON.stringify({
        id: subjectData.id,
        score,
        ratingCounts,
        total: episodeInfo?.total,
        airdates: mainEpisodes.map((ep) => ep.airdate),
        episodeSorts: mainEpisodes.map((ep) => ep.sort),
        tags,
        fansubs,
    });
    const hash = crypto.createHash("sha256").update(hashInput, "utf8").digest("hex");

    // ── 10. 检查缓存 ──
    const cachedFileId = await getImgCache(hash);
    if (cachedFileId) {
        logger.debug(`[generateBangumiNavImage] 命中图片缓存: subject=${subjectData.id} hash=${hash.slice(0, 8)}`);
        return { hash, file_id: cachedFileId };
    }

    // ── 11. 读取 Vue 模板并生成图片 ──
    const templateStr = await fs.readFile(
        new URL("bangumi.vue", import.meta.url),
        "utf-8",
    );

    const imageResult = await generateImage(
        { width: 1000, height: 550, quality: 2 },
        templateStr,
        props,
    );

    if (!imageResult.path) {
        throw new Error(`生成导航封面图片失败: subject=${subjectData.id}`);
    }

    // ── 12. 将 file_id 缓存会在上传到 Telegram 后由调用方设置 ──
    return {
        path: imageResult.path,
        hash,
    };
}
