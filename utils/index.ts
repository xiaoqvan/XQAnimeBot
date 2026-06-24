import logger from "@log/index.ts";
import { getTagExcludeList } from "../database/query.ts";

/** smartDelay 计算结果 */
export interface SmartDelayInfo {
  /** 当前时间段的基础间隔（毫秒） */
  intervalMs: number;
  /** 距离下一个时间段切换点的时长（毫秒） */
  timeToNextChangeMs: number;
  /** 本轮实际等待时长（毫秒） */
  waitMs: number;
  /** 本轮等待结束时间 */
  waitEnd: Date;
  /** 北京时间小时（0-23） */
  currentHourInBeijing: number;
  /** 下一个切换点小时（北京时间） */
  nextChangeHourInBeijing: number;
}

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

/**
 * 计算 smartDelay 的下一次刷新信息（不执行 sleep）
 * @param now - 可选基准时间，默认当前时间
 * @returns smartDelay 的等待信息快照
 */
export function getSmartDelayInfo(now: Date = new Date()): SmartDelayInfo {
  // 获取北京时间
  const beijingTime = new Date(now.getTime() + 8 * 60 * 60 * 1000);
  const currentHour = beijingTime.getUTCHours();

  // 时间段切换点：2, 11, 14, 18, 21
  const changeHours = [2, 11, 14, 18, 21];

  // 获取请求间隔
  let intervalMs;
  if (currentHour >= 21 || currentHour < 2) {
    intervalMs = 60 * 1000;
  } else if (currentHour >= 18 && currentHour < 21) {
    intervalMs = 3 * 60 * 1000;
  } else if (currentHour >= 11 && currentHour < 14) {
    intervalMs = 5 * 60 * 1000;
  } else {
    intervalMs = 15 * 60 * 1000;
  }

  // 找到下一个切换点
  let nextChangeHour = changeHours.find((hour) => hour > currentHour);
  let timeToNextChangeMs;

  if (!nextChangeHour) {
    nextChangeHour = 2;
    const tomorrow = new Date(beijingTime);
    tomorrow.setUTCDate(tomorrow.getUTCDate() + 1);
    tomorrow.setUTCHours(2, 0, 0, 0);
    const tomorrowLocal = new Date(tomorrow.getTime() - 8 * 60 * 60 * 1000);
    timeToNextChangeMs = tomorrowLocal.getTime() - now.getTime();
  } else {
    const nextChange = new Date(beijingTime);
    nextChange.setUTCHours(nextChangeHour, 0, 0, 0);
    const nextChangeLocal = new Date(nextChange.getTime() - 8 * 60 * 60 * 1000);
    timeToNextChangeMs = nextChangeLocal.getTime() - now.getTime();
  }

  const waitMs = timeToNextChangeMs < intervalMs ? timeToNextChangeMs + 1000 : intervalMs;
  const waitEnd = new Date(now.getTime() + waitMs);

  return {
    intervalMs,
    timeToNextChangeMs,
    waitMs,
    waitEnd,
    currentHourInBeijing: currentHour,
    nextChangeHourInBeijing: nextChangeHour,
  };
}

/**
 * 智能延迟与时间段计算方法
 * 根据当前时间动态调整请求间隔
 */
export async function smartDelayWithInterval() {
  const now = new Date();
  const info = getSmartDelayInfo(now);

  if (info.timeToNextChangeMs < info.intervalMs) {
    logger.debug(
      `距离下一个时间段切换还有 ${Math.round(
        info.timeToNextChangeMs / 60000
      )} 分钟，将在切换点立即检查，等待 ${info.waitMs} ms，结束时间: ${info.waitEnd.toLocaleString()}`
    );
    await delay(info.waitMs);
  } else {
    logger.debug(
      `本次等待 ${info.waitMs} ms，结束时间: ${info.waitEnd.toLocaleString()}`
    );
    await delay(info.waitMs);
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

// 重新导出精简搜索工具
export {
  formatSearchSimple,
} from "./animeSearchSimple.ts";
export type {
  SearchSimpleResult,
  SearchSimpleOptions,
} from "./animeSearchSimple.ts";
