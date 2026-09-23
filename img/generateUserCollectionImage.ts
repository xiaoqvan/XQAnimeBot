/**
 * 用户收藏图片生成模块
 *
 * 参考 generateBangumiImage.ts，使用 user_collection.vue 模板生成用户个人收藏图片。
 * 包含用户头像、观影状态、个人评分、标签、观看进度等个性化信息。
 *
 * 特性：
 * - 复用 generateImage 函数渲染 Vue 模板为图片
 * - 内置哈希去重：相同用户+相同数据不重复生成
 * - 复用 img_cache 集合缓存
 */

import fs from "fs/promises";
import { generateImage } from "@function/genImg.ts";
import { getImgCache } from "@db/query.ts";
import crypto from "crypto";
import logger from "@log/index.ts";
import type { bangumiAnime } from "../types/bangumi.d.ts";

// ─── 内部类型 ────────────────────────────────────────────────────────────────

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

// ─── 工具函数（复用 generateBangumiImage 中的逻辑） ─────────────────────────

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

/** 从 episodeList 中计算已播出集数 */
function computeAiredEpisodes(episodes: BangumiEpisode[]): {
    mainEpisodes: BangumiEpisode[];
    otherEpisodes: { type: number; label: string; episodes: BangumiEpisode[] }[];
    airedCount: number;
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

    return { mainEpisodes, otherEpisodes: otherGroups, airedCount };
}

// ─── 用户观影状态映射 ────────────────────────────────────────────────────────

/** 用户观影状态类型 */
export type WatchStatus = "doing" | "collect" | "wish" | "on_hold" | "dropped";

const WATCH_STATUS_MAP: Record<WatchStatus, { label: string; style: string }> = {
    doing:   { label: "在看", style: "border: 1.5px solid #1677ff; background-color: #e6f4ff; color: #1677ff;" },
    collect: { label: "看过", style: "border: 1.5px solid #52c41a; background-color: #f6ffed; color: #52c41a;" },
    wish:    { label: "想看", style: "border: 1.5px solid #ff5c9d; background-color: #fff0f6; color: #ff5c9d;" },
    on_hold: { label: "搁置", style: "border: 1.5px solid #fa8c16; background-color: #fff7e6; color: #fa8c16;" },
    dropped: { label: "抛弃", style: "border: 1.5px solid #d9d9d9; background-color: #f5f5f5; color: #8c8c8c;" },
};

// ─── 集数样式预计算 ──────────────────────────────────────────────────────────

function buildEpisodeStyles(
    episodeStatusMap: Record<number, string>,
    userWatchingEp: number,
): Record<number, { container: string; text: string }> {
    const styles: Record<number, { container: string; text: string }> = {};

    for (const [sortStr, status] of Object.entries(episodeStatusMap)) {
        const sort = Number(sortStr);
        switch (status) {
            case "dropped":
                styles[sort] = {
                    container: "background-color: #f5f5f5; border: 1.5px solid #d9d9d9;",
                    text: "color: #bfbfbf; text-decoration: line-through; font-weight: 400;",
                };
                break;
            case "want":
                styles[sort] = {
                    container: "background-color: rgba(255,92,157,0.15); border: 1.5px solid rgba(255,92,157,0.3);",
                    text: "color: #ff5c9d; font-weight: 500;",
                };
                break;
            case "watched":
                styles[sort] = {
                    container: "background-color: rgba(22,119,255,0.08); border: 1.5px solid #1677ff;",
                    text: "color: #1677ff; font-weight: 600;",
                };
                break;
            default:
                // unreleased / 未放映
                styles[sort] = {
                    container: "background-color: #f5f5f5; border: 1.5px solid #e8e8e8;",
                    text: "color: #bfbfbf; font-weight: 400;",
                };
        }
    }

    // 对于 episodeStatusMap 中没有但 <= userWatchingEp 的集数也标记为已看
    // （外部已保证传入完整的 statusMap，此处仅作为兜底）

    return styles;
}

// ─── 公开接口 ────────────────────────────────────────────────────────────────

export interface GenerateUserCollectionImageInput {
    /** Bangumi API 返回的条目完整信息 */
    subjectData: bangumiAnime;
    /** 集数信息 */
    episodeInfo?: EpisodeInfo | null;

    // ── 用户信息 ──
    /** 用户头像 data URI 或 URL（可为空） */
    userAvatar?: string | null;
    /** 用户显示名称 */
    userName: string;
    /** 用户 @username（可为空） */
    userUsername?: string | null;
    /** 观影状态 */
    watchStatus: WatchStatus;
    /** 用户当前观看集数 */
    userWatchingEp: number;
    /** 用户个人评分 0-10 */
    userRating: number;
    /** 用户添加的标签 */
    userTags?: string[];
    /** 每集观看状态: key=集数sort, value='watched'|'unreleased'|'want'|'dropped' */
    episodeStatusMap?: Record<number, string>;
}

export interface GenerateUserCollectionImageResult {
    /** 本地图片路径 */
    path?: string;
    /** 图片内容哈希 */
    hash: string;
    /** Telegram 缓存的 file_id */
    file_id?: string;
}

/**
 * 生成用户收藏图片
 *
 * @param input - 用户收藏数据
 * @returns 生成结果
 */
export async function generateUserCollectionImage(
    input: GenerateUserCollectionImageInput,
): Promise<GenerateUserCollectionImageResult> {
    const {
        subjectData,
        episodeInfo,
        userAvatar = null,
        userName,
        userUsername = null,
        watchStatus,
        userWatchingEp,
        userRating,
        userTags = [],
        episodeStatusMap = {},
    } = input;

    // ── 1. 基础数据 ──
    const infoboxMap = getInfoboxMap(subjectData.infobox || []);
    const displayStaff = getDisplayStaff(infoboxMap);
    const score = subjectData.rating?.score ?? 0;
    const ratingLabel = getRatingLabel(score);
    const ratingEmojiIdx = score >= 8.5 ? 4 : score >= 6.5 ? 3 : score >= 5.5 ? 2 : score >= 3.5 ? 1 : 0;
    const generatedAt = new Date().toISOString().slice(0, 10).replace(/-/g, "/");

    // ── 2. 章节数据 ──
    let mainEpisodes: BangumiEpisode[] = [];
    let otherEpisodes: { type: number; label: string; episodes: BangumiEpisode[] }[] = [];
    let airedCount = 0;

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
    }

    // ── 3. 柱状图百分比 ──
    const ratingCounts = subjectData.rating?.count || {};
    const maxCount = Math.max(1, ...Object.values(ratingCounts));
    const barPcts: number[] = [];
    for (let i = 10; i >= 1; i--) {
        barPcts.push(Math.max(3, Math.round(((ratingCounts[i as keyof typeof ratingCounts] || 0) / maxCount) * 100)));
    }

    // ── 4. 收藏统计 ──
    const col = subjectData.collection || { on_hold: 0, dropped: 0, wish: 0, collect: 0, doing: 0 };
    const fmtNum = (n: number) => (n >= 1000 ? (n / 1000).toFixed(1) + "K" : String(n));

    // ── 5. 标签 ──
    const tags = (subjectData.tags || []).slice(0, 12);

    // ── 6. 分页计算 ──
    const episodesPerPage = 30;
    const activePageIdx = userWatchingEp > 0
        ? Math.min(
            Math.floor((userWatchingEp - 1) / episodesPerPage),
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

    // ── 8. 观影状态 ──
    const statusInfo = WATCH_STATUS_MAP[watchStatus] || WATCH_STATUS_MAP.doing;

    // ── 9. 完成度百分比 ──
    const completionPercent = mainEpisodes.length > 0
        ? Math.round((userWatchingEp / mainEpisodes.length) * 100)
        : 0;

    // ── 10. 集数样式预计算 ──
    const episodeStyles = buildEpisodeStyles(episodeStatusMap, userWatchingEp);

    // ── 11. 构建 props ──
    const props = {
        // 番剧基础信息
        id: subjectData.id,
        name: subjectData.name || "",
        name_cn: subjectData.name_cn || "",
        images: subjectData.images || {},
        date: subjectData.date || "",
        platform: subjectData.platform || "",
        total_episodes: subjectData.total_episodes || 0,
        rating: subjectData.rating || { rank: 0, total: 0, count: {}, score: 0 },
        tags,
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
        // 以下字段在 user_collection.vue 中不再使用，保留为空默认值
        currentEpNum: 0,
        currentEpName: "",
        currentEpAirDate: "",
        nextEpNum: 0,
        nextEpName: "",
        nextEpAirDate: "",
        fansubs: [],

        // ── 用户信息 ──
        userAvatar: userAvatar || "",
        userName,
        userUsername: userUsername || "",
        watchStatusLabel: statusInfo.label,
        watchStatusStyle: statusInfo.style,
        userWatchingEp,
        completionPercent,
        userRating,
        userTags,
        episodeStyles,
    };

    // ── 12. 计算内容哈希（用于去重） ──
    const hashInput = JSON.stringify({
        subjectId: subjectData.id,
        userId: userName,
        watchStatus,
        userWatchingEp,
        userRating,
        userTags,
        episodeStatusMap,
        score,
        episodeSorts: mainEpisodes.map((ep) => ep.sort),
    });
    const hash = crypto.createHash("sha256").update(hashInput, "utf8").digest("hex");

    // ── 13. 检查缓存 ──
    const cachedFileId = await getImgCache(hash);
    if (cachedFileId) {
        logger.debug(`[generateUserCollectionImage] 命中图片缓存: user=${userName} subject=${subjectData.id} hash=${hash.slice(0, 8)}`);
        return { hash, file_id: cachedFileId };
    }

    // ── 14. 读取 Vue 模板并生成图片 ──
    const templateStr = await fs.readFile(
        new URL("user_collection.vue", import.meta.url),
        "utf-8",
    );

    const imageResult = await generateImage(
        { width: 1000, height: 550, quality: 2 },
        templateStr,
        props,
    );

    if (!imageResult.path) {
        throw new Error(`生成用户收藏图片失败: user=${userName} subject=${subjectData.id}`);
    }

    // ── 15. 返回结果（file_id 由调用方在上传后缓存） ──
    return {
        path: imageResult.path,
        hash,
    };
}
