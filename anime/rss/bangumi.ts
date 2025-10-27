import axios from "axios";
import * as cheerio from "cheerio";
import logger from "@log/index.ts";
import type { RssAnimeItem } from "../../types/anime.ts";
import { isTitleAllowed } from "./common.ts";

/**
 * 格式化发布时间 UTC+8
 * @param pubDateString 发布时间字符串
 * @returns
 */
export function formatPubDate(pubDateString: string): string {
  const timestamp = Date.parse(pubDateString);
  if (isNaN(timestamp)) {
    logger.warn(`formatPubDate: 无法解析的日期字符串: ${pubDateString}`);
    return pubDateString;
  }

  const beijingTs = timestamp + 8 * 60 * 60 * 1000; // +8 小时
  const d = new Date(beijingTs);

  const year = d.getUTCFullYear();
  const month = String(d.getUTCMonth() + 1).padStart(2, "0");
  const day = String(d.getUTCDate()).padStart(2, "0");

  let hours = d.getUTCHours();
  const minutes = String(d.getUTCMinutes()).padStart(2, "0");
  const ampm = hours >= 12 ? "PM" : "AM";

  hours = hours % 12;
  hours = hours ? hours : 12; // 0点显示为12点
  const formattedHours = String(hours).padStart(2, "0");

  return `${year}年${month}月${day}日 ${formattedHours}:${minutes}${ampm}`;
}

export async function fetchBangumiRss() {
  try {
    const response = await axios.get("https://bangumi.moe/rss/latest");
    const xml = response.data;
    const $ = cheerio.load(xml, { xmlMode: true });
    const items = $("channel > item");
    const bangumiList: RssAnimeItem[] = [];

    for (const item of items) {
      const title = $(item).find("title").text().trim().replace(/\s+/g, " ");
      const link = $(item).find("link").text();
      const pubDateRaw = $(item).find("pubDate").text();
      const pubDate = formatPubDate(pubDateRaw);
      const id = link.replace(/.*\/(\w+)$/, "$1");
      const torrent = $(item).find("enclosure").attr("url");

      // 统一的标题内容过滤（迁移到 common.ts）
      if (!isTitleAllowed(title)) continue;

      bangumiList.push({
        type: "bangumi",
        id,
        title,
        link,
        pubDate,
        torrent,
      });
    }

    return bangumiList;
  } catch (error) {
    logger.error("Error fetching Bangumi data:", error);
    throw error;
  }
}
