/**
 * @file Bangumi 工具函数 —— 精简搜索与关联条目查询
 *
 * #sym:AnimeTools 供 AI Agent 使用
 * 提供：
 * - searchAnimeCandidates —— 精简搜索，返回 AnimeCandidate[]
 * - getRelatedSubjects   —— 获取关联条目（如番剧相关的其他动画）
 */

import axios from "axios";
import { env } from "../database/initDb.ts";
import { getEpisodeInfo } from "./get.ts";

// ─── Types ───────────────────────────────────────────────────────────────────

/** 精简的番剧候选项 */
export type AnimeCandidate = {
    id: number;

    // 名称
    name: string;
    name_cn?: string;

    // 时间
    date?: string;

    // 集数 "26-38"
    episode_range?: string;

    // 简介
    summary?: string;
};

/** 关联条目精简结果（去掉 images 和 type） */
export type RelatedSubject = {
    id: number;
    name: string;
    name_cn: string;
    relation: string;
};

/** getEpisodeInfo 返回的章节条目 */
interface EpisodeItem {
    id: number;
    name: string;
    name_cn: string;
    /** 章节编号（如第 1 集、第 2 集），用于推断总集数 */
    ep: number;
    /** 实际集数编号（与 ep 相同或更精准），取最大值作为总集数 */
    sort: number;
    airdate: string;
    duration: string;
    desc: string;
    subject_id: number;
    comment: number;
    type: number;
}

/** getEpisodeInfo 的完整响应结构 */
interface EpisodeInfoResponse {
    data: EpisodeItem[];
    total: number;
    limit: number;
    offset: number;
}

/** 搜索 API 返回的原始条目（bangumiAnime 的子集，由后端实际返回的字段） */
interface SearchResultItem {
    id: number;
    name: string;
    name_cn?: string;
    date?: string;
    summary?: string;
}

/** 关联条目 API 返回的原始条目 */
interface RawRelatedSubject {
    id: number;
    name: string;
    name_cn: string;
    relation: string;
    type: number;
    images: Record<string, string>;
}

// ─── API 通用配置 ────────────────────────────────────────────────────────────

const BASE_URL = "https://api.bgm.tv";
const HEADERS = {
    "User-Agent": "xiaoqvan/my-private-project",
    Authorization: `Bearer ${env.data.BG_ACCESS_TOKEN}`,
};

// ─── 工具函数 ────────────────────────────────────────────────────────────────

/**
 * 根据 getEpisodeInfo 的返回数据推断集数范围字符串。
 * Bangumi API 中每集有 sort 字段表示实际集数编号，
 * 取 data 中最大 sort 值作为总集数。
 *
 * 例如：sort 值有 1, 2, 3, ..., 26，则返回 "26"。
 */
async function inferEpisodeRange(subjectId: number): Promise<string> {
    const res = await getEpisodeInfo(subjectId) as EpisodeInfoResponse;
    const episodes = res.data ?? [];

    if (episodes.length === 0) {
        return "0";
    }

    // 取最大 sort 值作为总集数
    const maxSort = Math.max(...episodes.map((ep) => ep.sort));
    return String(maxSort);
}

/**
 * #sym:searchAnimeCandidates
 *
 * 精简搜索番剧，返回 AnimeCandidate[]。
 *
 * 与 `animeinfo()` 不同，此函数只返回最必要的字段，
 * 并通过 getEpisodeInfo 补充集数信息。
 * 适合在简单列表展示、候选推荐等场景使用。
 *
 * @param keyword  搜索关键词
 * @param limit    返回数量上限（默认 10）
 */
export async function searchAnimeCandidates(
    keyword: string,
    limit = 10,
): Promise<AnimeCandidate[]> {
    if (!keyword) {
        throw new Error("searchAnimeCandidates: 搜索关键词为空");
    }

    const schema = {
        keyword,
        sort: "rank",
        filter: { type: [2], nsfw: true },
    };

    const res = await axios.post(
        `${BASE_URL}/v0/search/subjects?limit=${limit}`,
        schema,
        { headers: HEADERS },
    );

    const list: SearchResultItem[] = res.data.data ?? [];

    // 并行获取集数信息
    const candidates: AnimeCandidate[] = await Promise.all(
        list.map(async (item) => {
            let episode_range: string | undefined;
            try {
                episode_range = await inferEpisodeRange(item.id);
            } catch {
                // 获取集数失败时跳过此字段
            }

            return {
                id: item.id,
                name: item.name,
                name_cn: item.name_cn,
                date: item.date,
                episode_range,
                summary: item.summary,
            };
        }),
    );

    return candidates;
}

/**
 * #sym:getRelatedSubjects
 *
 * 获取指定条目的关联作品列表，仅保留 type === 2（动画）的作品，
 * 并对结果进行精简：去掉 images、type 等冗余字段。
 *
 * API: GET /v0/subjects/{subject_id}/subjects
 *
 * @param subjectId 条目 ID
 * @returns 精简后的关联动画列表
 */
export async function getRelatedSubjects(subjectId: number): Promise<RelatedSubject[]> {
    if (!subjectId) {
        throw new Error("getRelatedSubjects: 无效的 subjectId");
    }

    const res = await axios.get(
        `${BASE_URL}/v0/subjects/${subjectId}/subjects`,
        { headers: HEADERS },
    );

    const list: RawRelatedSubject[] = res.data ?? [];

    // 筛选 type === 2 并精简字段
    const related: RelatedSubject[] = list
        .filter((item) => item.type === 2)
        .map((item) => ({
            id: item.id,
            name: item.name,
            name_cn: item.name_cn,
            relation: item.relation,
        }));

    return related;
}