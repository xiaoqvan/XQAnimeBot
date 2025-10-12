import { fetchBangumiRss } from "./bangumi.ts";
import { fetchDmhyRss } from "./dmhy.ts";
import { fetchAcgnxRss } from "./acgnx.ts";
import logger from "@log/index.ts";
import { getAnimeBlacklist } from "../../database/query.ts";
import type { RssAnimeItem } from "../../types/anime.ts";

/**
 * 合并多个RSS源并进行去重处理
 * 优先级顺序：bangumi > dmhy > acgnx
 * 相同标题的条目只保留优先级最高的源
 * 如果某个RSS源失效，会自动跳过该源继续处理其他源
 */
export async function fetchMergedRss() {
  try {
    // 并行获取所有RSS源的数据，失效的源会被自动跳过
    const [bangumiData, dmhyData, acgnxData] = await Promise.allSettled([
      fetchBangumiRss().catch((err) => {
        logger.warn("Bangumi RSS获取失败:", err.message);
        return [];
      }),
      fetchDmhyRss().catch((err) => {
        logger.warn("DMHY RSS获取失败:", err.message);
        return [];
      }),
      fetchAcgnxRss().catch((err) => {
        logger.warn("ACGNX RSS获取失败:", err.message);
        return [];
      }),
    ]);

    // 提取成功的数据
    const bangumi = bangumiData.status === "fulfilled" ? bangumiData.value : [];
    const dmhy = dmhyData.status === "fulfilled" ? dmhyData.value : [];
    const acgnx = acgnxData.status === "fulfilled" ? acgnxData.value : [];

    logger.debug(
      `RSS数据获取完成 - Bangumi: ${bangumi.length}, DMHY: ${dmhy.length}, ACGNX: ${acgnx.length}`
    );

    // 获取拉黑列表（若未配置则视为空数组）
    const blacklistAnimes = (await getAnimeBlacklist()) ?? [];

    // 使用Map进行去重，按优先级保留
    const mergedMap = new Map();

    // 按优先级顺序处理：bangumi > dmhy > acgnx
    // 首先处理优先级最低的acgnx
    acgnx.forEach((item) => {
      const title = item.title;
      // 检查是否在拉黑列表中
      const isBlacklisted = blacklistAnimes.some((blacklistedName) =>
        title.includes(blacklistedName)
      );
      if (isBlacklisted) {
        logger.debug(`跳过拉黑动漫: ${title}`);
        return;
      }
      const normalizedTitle = normalizeTitle(title);
      const cleanedTitle = cleanTitle(title);
      if (!mergedMap.has(normalizedTitle)) {
        mergedMap.set(normalizedTitle, {
          ...item,
          title: cleanedTitle,
          source: "acgnx",
        });
      }
    });

    // 然后处理dmhy，会覆盖相同标题的acgnx条目
    dmhy.forEach((item) => {
      const title = item.title;
      const isBlacklisted = blacklistAnimes.some((blacklistedName) =>
        title.includes(blacklistedName)
      );
      if (isBlacklisted) {
        logger.debug(`跳过拉黑动漫: ${title}`);
        return;
      }
      const normalizedTitle = normalizeTitle(title);
      const cleanedTitle = cleanTitle(title);
      mergedMap.set(normalizedTitle, {
        ...item,
        title: cleanedTitle,
        source: "dmhy",
      });
    });

    // 最后处理bangumi，会覆盖相同标题的dmhy和acgnx条目
    bangumi.forEach((item) => {
      const title = item.title;
      const isBlacklisted = blacklistAnimes.some((blacklistedName) =>
        title.includes(blacklistedName)
      );
      if (isBlacklisted) {
        logger.debug(`跳过拉黑动漫: ${title}`);
        return;
      }
      const normalizedTitle = normalizeTitle(title);
      const cleanedTitle = cleanTitle(title);
      mergedMap.set(normalizedTitle, {
        ...item,
        title: cleanedTitle,
        source: "bangumi",
      });
    });

    const mergedList: RssAnimeItem[] = Array.from(mergedMap.values());

    // 按发布时间排序（最新的在前）
    mergedList.sort((a, b) => {
      if (a.pubDate && b.pubDate) {
        return parsePubDate(b.pubDate) - parsePubDate(a.pubDate);
      }
      return 0;
    });

    return mergedList;
  } catch (error) {
    logger.error("合并RSS数据时发生错误:", error);
    return [];
  }
}

/**
 * 解析中文格式的发布日期并返回时间戳（毫秒）
 * 支持格式："2025年07月20日 01:56AM" 或 "2025年07月19日 10:01PM"
 */
function parsePubDate(pubDateString: string): number {
  try {
    const match = pubDateString.match(
      /(\d{4})年(\d{2})月(\d{2})日\s+(\d{1,2}):(\d{2})(AM|PM)/
    );

    if (!match) {
      const d = new Date(pubDateString);
      return Number.isNaN(d.getTime()) ? Date.now() : d.getTime();
    }

    const [, year, month, day, hour, minute, ampm] = match;

    let hour24 = parseInt(hour);
    if (ampm === "PM" && hour24 !== 12) {
      hour24 += 12;
    } else if (ampm === "AM" && hour24 === 12) {
      hour24 = 0;
    }

    const d = new Date(
      parseInt(year),
      parseInt(month) - 1,
      parseInt(day),
      hour24,
      parseInt(minute)
    );
    return d.getTime();
  } catch {
    return Date.now();
  }
}

/**
 * 清理标题中的多余空格
 * 保留原始标题的大小写和格式，只压缩多个空格为单个空格
 */
function cleanTitle(title: string): string {
  return (
    title
      .trim()
      // 移除零宽字符
      .replace(/[\u200B-\u200D\uFEFF]/g, "")
      // 压缩多个空格为单个空格
      .replace(/\s+/g, " ")
      .trim()
  );
}

/**
 * 标准化标题用于去重比较
 * 移除一些可能影响比较的字符和格式差异
 */
function normalizeTitle(title: string): string {
  return (
    title
      .trim()
      .toLowerCase()
      // 替换全角符号为半角
      .replace(/／/g, "/")
      .replace(/【/g, "[")
      .replace(/】/g, "]")
      .replace(/（/g, "(")
      .replace(/）/g, ")")
      // 移除零宽字符
      .replace(/[\u200B-\u200D\uFEFF]/g, "")
      // 先压缩所有多余空格为单个空格（必须在其他空格处理之前）
      .replace(/\s+/g, " ")
      // 去除斜杠两侧空格
      .replace(/\s*\/\s*/g, "/")
      // 统一连接符（去除连接符两侧空格）
      .replace(/\s*-\s*/g, "-")
      .replace(/\s*_\s*/g, "_")
      // 去除方括号内侧的空格
      .replace(/\[\s+/g, "[")
      .replace(/\s+\]/g, "]")
      // 再次压缩空格确保彻底清理
      .replace(/\s+/g, " ")
      .trim()
  );
}
