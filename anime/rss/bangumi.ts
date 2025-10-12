import axios from "axios";
import * as cheerio from "cheerio";
import logger from "@log/index.ts";
import type { RssAnimeItem } from "../../types/anime.ts";

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

      // 过滤标题中含有特定内容的条目
      if (
        title.includes("内封") ||
        title.includes("繁") ||
        title.includes("合集") ||
        title.includes("無字幕") ||
        title.includes("粵語") ||
        title.includes("整理搬运") ||
        title.includes("無對白字幕") ||
        /\bFin\b/i.test(title) || // 精确匹配 Fin
        title.includes("BIG5") || // 匹配所有包含 BIG5 的情况
        /\bMKV\b/i.test(title) || // 精确匹配 MKV（不是 xxxMKVxxx）
        title.includes("[720p]")
      )
        continue;

      // 幻樱字幕组特殊过滤：只要中文版本和1080P
      if (title.includes("【幻樱字幕组】")) {
        // 过滤掉BIG5繁体版本
        if (title.includes("【BIG5_MP4】")) {
          continue;
        }
        // 过滤掉720P版本
        if (title.includes("【1280X720】")) {
          continue;
        }
        // 只保留GB简体和1080P版本
        if (!title.includes("【GB_MP4】") || !title.includes("【1920X1080】")) {
          continue;
        }
      }

      // 悠哈璃羽字幕社特殊过滤：只要CHS简体版本，排除CHT繁体版本
      if (title.includes("【悠哈璃羽字幕社】")) {
        // 过滤掉CHT繁体版本
        if (title.includes("[CHT]")) {
          continue;
        }
      }

      // 过滤集数区间
      if (/\[\d{1,3}-\d{1,3}\]|\(\d{1,3}-\d{1,3}\)/.test(title)) continue;

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
