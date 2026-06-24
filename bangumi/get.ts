import axios from "axios";
import logger from "@log/index.ts";
import type { bangumiAnime, bangumiSearchResult } from "../types/bangumi.d.ts";
import { env } from "../database/initDb.ts";

/**
 * 重试请求函数
 * @param requestFn - 请求函数
 * @param maxRetries - 最大重试次数
 * @param delay - 重试间隔（毫秒）
 * @returns 请求结果
 */
export async function retryRequest(
    requestFn: () => Promise<any>,
    maxRetries = 3,
    delay = 10000
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

/** 使用 bangumi.tv API 获取动漫信息
 * @param keyword - 搜索关键词
 * @returns API响应数据
 * @throws 当关键词为空或请求失败时抛出异常
 */
export async function animeinfo(keyword: string) {
    if (!keyword) {
        throw new Error(`bgm.tv 动漫搜索关键词为空 , keyword:${keyword}`);
    }

    const Schema = {
        keyword: `${keyword}`,
        sort: "rank",
        filter: {
            type: [2],
            nsfw: true,
        },
    };

    const data = await retryRequest(async () => {
        return await axios.post(
            "https://api.bgm.tv/v0/search/subjects?limit=10",
            Schema,
            {
                headers: {
                    "User-Agent": "xiaoqvan/my-private-project",
                    Authorization: `Bearer ${env.data.BG_ACCESS_TOKEN}`,
                },
            }
        );
    });
    return data.data as bangumiSearchResult;
}

/** 获取一个番剧的信息
 * @param id
 * @returns
 */
export async function getSubjectById(
    id: number | string
): Promise<bangumiAnime> {
    if (!id || (typeof id !== "number" && typeof id !== "string")) {
        throw new Error("无效的ID");
    }

    const response = await retryRequest(async () => {
        return await axios.get(`https://api.bgm.tv/v0/subjects/${id}`, {
            headers: {
                "User-Agent": "xiaoqvan/my-private-project",
                Authorization: `Bearer ${env.data.BG_ACCESS_TOKEN}`,
            },
        });
    });

    if (response.status !== 200) {
        throw new Error(`获取数据失败，状态码：${response.status}`);
    }

    return response.data as bangumiAnime;
}

/**
 * 获取章节信息
 * @param episodesid 章节ID
 * @returns 章节信息
 */
export async function getEpisodeById(episodesid: number) {
    if (!episodesid) {
        throw new Error("无效的章节ID");
    }
    const response = await axios.get(`https://api.bgm.tv/v0/episodes/${episodesid}`, {
        headers: {
            "User-Agent": "XQAnimeBot/1.0 (https://github.com/xiaoqvan/XQAnimeBot)",
            Authorization: `Bearer ${env.data.BG_ACCESS_TOKEN}`,
        },
    });
    return response.data;
}

/**
 * 获取一个番剧的剧集信息
 * @param id
 * @returns
 */
export async function getEpisodeInfo(id: number | string) {
    if (!id || (typeof id !== "number" && typeof id !== "string")) {
        throw new Error("无效的ID");
    }

    const data = await retryRequest(async () => {
        return await axios.get(
            `https://api.bgm.tv/v0/episodes?subject_id=${id}&limit=100&offset=0`,
            {
                headers: {
                    "User-Agent": "xiaoqvan/my-private-project",
                    Authorization: `Bearer ${env.data.BG_ACCESS_TOKEN}`,
                },
            }
        );
    });

    if (data.status !== 200) {
        throw new Error(`获取数据失败，状态码：${data.status}`);
    }
    return data.data;
}