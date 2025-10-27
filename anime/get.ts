import axios from "axios";
import * as cheerio from "cheerio";
import logger from "@log/index.ts";
import type { bangumiAnime, bangumiSearchResult } from "../types/anime.js";

/**
 * 重试请求函数
 * @param requestFn - 请求函数
 * @param maxRetries - 最大重试次数
 * @param delay - 重试间隔（毫秒）
 * @returns 请求结果
 */
async function retryRequest(
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
      },
    });
  });

  if (response.status !== 200) {
    throw new Error(`获取数据失败，状态码：${response.status}`);
  }

  return response.data as bangumiAnime;
}

/**
 * 请求 bangumi.moe API 获取种子信息
 * @param torrentId - 种子ID
 * @returns API响应数据
 */
export async function fetchBangumiTorrent(torrentId: string | number) {
  try {
    const response = await retryRequest(async () => {
      return await axios.post(
        "https://bangumi.moe/api/torrent/fetch",
        {
          _id: torrentId,
        },
        {
          headers: {
            "User-Agent":
              "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36 Edg/138.0.0.0",
          },
        }
      );
    });

    return response.data;
  } catch (error) {
    logger.error("请求失败:", error);
    throw error;
  }
}
/**
 * 请求 bangumi.moe API 获取团队信息
 * @param teamId - 团队ID
 * @return API响应数据
 */
export async function fetchBangumiTeam(teamId: string | number) {
  try {
    const response = await retryRequest(async () => {
      return await axios.post(
        "https://bangumi.moe/api/team/fetch",
        {
          _ids: [teamId],
        },
        {
          headers: {
            "User-Agent":
              "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36 Edg/138.0.0.0",
          },
        }
      );
    });

    return response.data;
  } catch (error) {
    logger.error("请求失败:", error);
    throw error;
  }
}

/**
 * 请求 bangumi.moe API 获取标签信息
 * @param tagsIds - 标签ID
 * @return API响应数据
 * @throws 请求失败时抛出异常
 */
export async function fetchBangumiTags(tagsIds: string[]) {
  try {
    const response = await retryRequest(async () => {
      return await axios.post(
        "https://bangumi.moe/api/tag/fetch",
        {
          _ids: tagsIds,
        },
        {
          headers: {
            "User-Agent":
              "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36 Edg/138.0.0.0",
          },
        }
      );
    });

    return response.data;
  } catch (error) {
    logger.error("请求失败:", error);
    throw error;
  }
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
        },
      }
    );
  });

  if (data.status !== 200) {
    throw new Error(`获取数据失败，状态码：${data.status}`);
  }
  return data.data;
}

/**
 * 获取dmhy种子信息
 * @param url - dmhy链接
 * @returns
 */
export async function fetchDmhyTorrent(url: string) {
  if (!url.includes("dmhy.org/topics/view/")) {
    throw new Error("无效的dmhy链接");
  }
  try {
    const response = await retryRequest(async () => {
      return await axios.get(url, {
        headers: {
          "User-Agent":
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36 Edg/138.0.0.0",
        },
      });
    });

    const $ = cheerio.load(response.data);

    const title =
      $(
        "body > div > div > div.main > div.topics_bk.ui-corner-all > div.topic-main > div.topic-title.box.ui-corner-all > h3"
      )
        .text()
        .trim() || "未知标题";

    const author =
      $(
        "body > div > div > div.main > div.topics_bk.ui-corner-all > div.user-sidebar > div:nth-child(1) > p:nth-child(2) > a"
      )
        .text()
        .trim() || "未知作者";

    const team =
      $(
        "body > div > div > div.main > div.topics_bk.ui-corner-all > div.user-sidebar > div:nth-child(2) > p:nth-child(2) > a"
      )
        .text()
        .trim() || "未知发布组";

    const pubDate =
      $(
        "body > div > div > div.main > div.topics_bk.ui-corner-all > div.topic-main > div.topic-title.box.ui-corner-all > div.info.resource-info.right > ul > li:nth-child(2) > span"
      )
        .text()
        .trim() || "未知时间";

    const magnet = $("#a_magnet").attr("href") || "未知磁力链接";

    return { title, pubDate, magnet, author, team };
  } catch (error) {
    logger.error("请求失败:", error);
    throw error;
  }
}
