import axios from "axios";
import * as cheerio from "cheerio";
import logger from "@log/index.ts";
import { retryRequest } from "../bangumi/get.ts";

export { animeinfo, getEpisodeInfo, getSubjectById } from "../bangumi/get.ts";

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
    logger.error(error, "请求失败:");
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
    logger.error(error, "请求失败:");
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
    logger.error(error, "请求失败:");
    throw error;
  }
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
    logger.error(error, "请求失败:");
    throw error;
  }
}
