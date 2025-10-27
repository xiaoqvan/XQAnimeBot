import logger from "@log/index.ts";
import { getTagExcludeList } from "../database/query.ts";

/**
 * 延迟函数
 * @param ms
 * @returns
 */
export function delay(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * 多个字幕组使用_链接
 * @param fansub
 * @returns
 */
export function combineFansub(fansub: string[] | null) {
  if (!Array.isArray(fansub) || fansub.length === 0) return "";
  return fansub.join("_");
}

export /**
 * 智能延迟与时间段计算方法
 * 根据当前时间动态调整请求间隔
 */
async function smartDelayWithInterval() {
  const now = new Date();
  // 获取北京时间
  const beijingTime = new Date(now.getTime() + 8 * 60 * 60 * 1000);
  const currentHour = beijingTime.getUTCHours();

  // 时间段切换点：2, 11, 14, 18, 21
  const changeHours = [2, 11, 14, 18, 21];

  // 获取请求间隔
  let interval;
  if (currentHour >= 21 || currentHour < 2) {
    interval = 60 * 1000;
  } else if (currentHour >= 18 && currentHour < 21) {
    interval = 3 * 60 * 1000;
  } else if (currentHour >= 11 && currentHour < 14) {
    interval = 5 * 60 * 1000;
  } else {
    interval = 15 * 60 * 1000;
  }

  // 找到下一个切换点
  let nextChangeHour = changeHours.find((hour) => hour > currentHour);
  let timeToNextChange;
  let waitMs;
  let waitEnd;
  if (!nextChangeHour) {
    nextChangeHour = 2;
    const tomorrow = new Date(beijingTime);
    tomorrow.setUTCDate(tomorrow.getUTCDate() + 1);
    tomorrow.setUTCHours(2, 0, 0, 0);
    const tomorrowLocal = new Date(tomorrow.getTime() - 8 * 60 * 60 * 1000);
    timeToNextChange = tomorrowLocal.getTime() - now.getTime();
  } else {
    const nextChange = new Date(beijingTime);
    nextChange.setUTCHours(nextChangeHour, 0, 0, 0);
    const nextChangeLocal = new Date(nextChange.getTime() - 8 * 60 * 60 * 1000);
    timeToNextChange = nextChangeLocal.getTime() - now.getTime();
  }

  if (timeToNextChange < interval) {
    waitMs = timeToNextChange + 1000;
    waitEnd = new Date(now.getTime() + waitMs);
    logger.debug(
      `距离下一个时间段切换还有 ${Math.round(
        timeToNextChange / 60000
      )} 分钟，将在切换点立即检查，等待 ${waitMs} ms，结束时间: ${waitEnd.toLocaleString()}`
    );
    await delay(waitMs);
  } else {
    waitMs = interval;
    waitEnd = new Date(now.getTime() + waitMs);
    logger.debug(
      `本次等待 ${waitMs} ms，结束时间: ${waitEnd.toLocaleString()}`
    );
    await delay(waitMs);
  }
}

/**
 * 提取过滤后的标签名称
 * @param tags - 标签数组
 * @returns 过滤后的标签名称数组
 */
export async function extractFilteredTagNames(
  tags: {
    name: string;
    count?: number;
    total_cont?: number;
  }[]
) {
  const excludeList = await getTagExcludeList();

  return tags
    .map((tag) => tag.name)
    .filter(
      (name) =>
        !/^\d{4}年/.test(name) && // 排除 "2024年" 这类标签
        !/^\d+$/.test(name) && // 排除纯数字标签
        !excludeList.includes(name) // 排除自定义黑名单
    );
}

/** 格式化标签为字符串
 * @param tags - 标签数组
 * @returns 格式化后的标签字符串
 */
export function formatTags(tags: string[]) {
  if (!Array.isArray(tags)) return "";

  return tags
    .map((t) => safeTag(t)) // 对每个标签进行格式化
    .filter((t) => t && !/^\d+$/.test(t)) // 过滤掉空值和纯数字标签
    .map((t) => `#${t}`)
    .join(" ");
}

/** 安全格式化标签
 * @param text - 原始标签文本
 * @returns 格式化后的标签文本
 */
export function safeTag(text: string) {
  text = String(text ?? "");
  return text
    .trim()
    .replace(/\s+/g, "")
    .replace(
      /[^\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Latin}0-9_]/gu,
      ""
    )
    .replace(/[-❀]/g, "");
}
