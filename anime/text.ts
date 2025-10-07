import { parseTextEntities } from "@TDLib/function/index.ts";
import type {
  animeItem,
  anime as animeType,
  BtData as BtDataType,
  BtEntry,
} from "../types/anime.ts";

import type { Client } from "tdl";

/**
 * 生成导航消息文本（首条带图，1024 文本长度限制；资源分条纯文本，每条 4096 文本长度限制）
 * - 超限时优先压缩 summary（最低压到 100 字符）
 * - 仍超限则移除首条中的资源区，并把资源分页输出到后续消息
 * - 无资源时不显示资源
 */
export async function navmegtext(
  client: Client,
  anime: animeType
): Promise<string[]> {
  const messages: string[] = [];
  const sections = formatBtData(anime.btdata || {}); // 资源段

  // 构造首条消息（可选择是否包含资源区、summary 最大长度、资源分页导航）
  const buildMain = (
    summaryMaxLen: number,
    includeResources: boolean,
    resourceNavigation = ""
  ) => {
    const title = `${animeDate(anime.airingStart)} ${NSFW(anime.r18)} ${
      anime.name || anime.name_cn
    }`;
    const baseInfo =
      `\n> 中文名称: ${anime.name_cn}\n` +
      `> 本季话数: ${anime.episode || "未知"}\n` +
      `> 放送开始: ${anime.airingStart || "未知"}\n` +
      `> 放送星期: ${anime.airingDay || "未知"}\n` +
      `> 动漫评分: [${anime.score || "未知"}](https://bgm.tv/subject/${
        anime.id
      }/stats)` +
      `${getBeijingDate()}`;

    const summaryPart = summaryTrim(anime.summary, anime.id, summaryMaxLen);

    // 资源区：只有在 includeResources 且确有资源时才显示
    const hasResources = includeResources && sections.length > 0;
    const resourcesPart = hasResources
      ? `\n\n资源:\n${sections.join("\n")}`
      : resourceNavigation
      ? `\n\n资源:\n> ${resourceNavigation}`
      : "";

    const tagsPart = `\n\n标签: \n> ${formatTags(anime.tags || [])}||`;

    return `${title}${baseInfo}${summaryPart}${resourcesPart}${tagsPart}`;
  };

  // 1) 先构建资源分页并获取导航链接
  let resourcePages: string[] = [];
  let resourceNavigation = "";

  if (sections.length > 0) {
    resourcePages = await buildResourcePages(client, anime, sections);

    // 构建页码导航
    if (resourcePages.length > 0) {
      const pageLinks: string[] = [];
      if (anime.navVideoMessage && anime.navVideoMessage.length > 0) {
        for (let i = 0; i < resourcePages.length; i++) {
          const videoMsg = anime.navVideoMessage.find(
            (msg) => msg.page === i + 1
          );
          if (videoMsg && videoMsg.link) {
            pageLinks.push(`[第${i + 1}页](${videoMsg.link})`);
          } else {
            pageLinks.push(`第${i + 1}页`);
          }
        }
      } else {
        for (let i = 0; i < resourcePages.length; i++) {
          pageLinks.push(`第${i + 1}页`);
        }
      }
      resourceNavigation = pageLinks.join(" ");
    }
  }

  // 2) 基于无资源版本确定合适的 summary 长度
  let summaryMax = 300;
  let mainWithoutResources = buildMain(summaryMax, false, resourceNavigation);
  let { textLength: baseTextLen, entitiesLength: baseEntitiesLen } =
    await measureTextLen(client, mainWithoutResources);

  if (baseTextLen > 1024 || baseEntitiesLen > 100) {
    // 3) 逐步压缩 summary 至少到 100
    const steps = [250, 200, 150, 120, 100];
    for (const len of steps) {
      summaryMax = len;
      const candidate = buildMain(summaryMax, false, resourceNavigation);
      const { textLength, entitiesLength } = await measureTextLen(
        client,
        candidate
      );
      if (textLength <= 1024 && entitiesLength <= 100) {
        mainWithoutResources = candidate;
        baseTextLen = textLength;
        baseEntitiesLen = entitiesLength;
        break;
      } else {
        mainWithoutResources = candidate;
        baseTextLen = textLength;
        baseEntitiesLen = entitiesLength;
      }
    }
  }

  // 4) 尝试在无资源版本基础上添加完整资源区
  let includeResources = false;
  let main = mainWithoutResources;

  if (sections.length > 0) {
    const mainWithResources = buildMain(summaryMax, true, "");
    const { textLength, entitiesLength } = await measureTextLen(
      client,
      mainWithResources
    );
    if (textLength <= 1024 && entitiesLength <= 100) {
      // 完整资源区可以放入首条消息
      includeResources = true;
      main = mainWithResources;
    }
  }

  messages.push(main);

  // 5) 添加资源分页（仅当资源未完整包含在首条时）
  if (!includeResources && resourcePages.length > 0) {
    messages.push(...resourcePages);
  }

  return messages;
}

/** 获取纯文本和实体数量（Markdown 转换后） */
async function measureTextLen(
  client: Client,
  md: string
): Promise<{ textLength: number; entitiesLength: number }> {
  const { text, entities } = await parseTextEntities(client, md);
  if (!text) {
    throw new Error("解析 Markdown 失败，无法获取纯文本长度");
  }
  return { textLength: text.length, entitiesLength: entities?.length || 0 };
}

/** 构造资源分页（每页 4096 文本长度限制） */
async function buildResourcePages(
  client: Client,
  anime: animeType,
  sections: string[]
): Promise<string[]> {
  const title = `动漫: ${anime.name_cn || anime.name}\n>\n资源:\n`;
  const pages: string[] = [];

  let currentBody = "";
  const pushPage = () => {
    if (currentBody.trim().length === 0) return;
    pages.push(`${title}${currentBody.trim()}`);
    currentBody = "";
  };

  // 逐段装入，确保每个字幕组（标题+集数）是一个整体
  for (const sec of sections) {
    const candidateBody = currentBody ? `${currentBody}\n${sec}` : sec;
    const candidatePage = `${title}${candidateBody}`;
    const { textLength, entitiesLength } = await measureTextLen(
      client,
      candidatePage
    );

    if (textLength > 4096 || entitiesLength > 100) {
      // 当前页已满，先提交当前页，再将本段作为新页的起始
      pushPage();

      // 若单段本身已超限，仍需强制放入（极端情况），避免死循环
      const singleSecPage = `${title}${sec}`;
      const { textLength: singleTextLen, entitiesLength: singleEntitiesLen } =
        await measureTextLen(client, singleSecPage);
      if (singleTextLen > 4096 || singleEntitiesLen > 100) {
        pages.push(singleSecPage);
        currentBody = "";
      } else {
        currentBody = sec;
      }
    } else {
      currentBody = candidateBody;
    }
  }
  pushPage();

  return pages;
}

/** 格式化 时间 #<时间> */
function animeDate(time: string | undefined) {
  const titleTag =
    (time ? "#" : "") +
    (time ? time.replace(/(\d{4})年(\d{1,2})月.*/, "$1年$2月") : "");
  return titleTag;
}

/** 格式化 NSFW #NSFW */
function NSFW(nsfw: boolean | null | undefined) {
  return nsfw ? "#NSFW" : "";
}

/** 格式化 动漫介绍 */
function summaryTrim(summary: string | undefined, id: number, maxLen = 300) {
  if (!summary || !summary.trim()) return "";
  const cleanSummary = summary.replace(/\\n/g, "\n").replace(/\n{2,}/g, "\n");
  const truncatedSummary =
    cleanSummary.length > maxLen
      ? cleanSummary.substring(0, maxLen) +
        `[...详细](https://bgm.tv/subject/${id})`
      : cleanSummary;
  return `\n\n介绍:\n> ${truncatedSummary.replace(/\n/g, "\n> ")}||`;
}

/** 获取当前北京时间，格式为 yyyy-MM-dd */
export function getBeijingDate() {
  const now = new Date();
  // 东八区偏移（分钟）
  const beijingOffset = 8 * 60;
  const localOffset = now.getTimezoneOffset();
  const beijingTime = new Date(
    now.getTime() + (beijingOffset + localOffset) * 60000
  );

  const y = beijingTime.getFullYear();
  const m = String(beijingTime.getMonth() + 1).padStart(2, "0");
  const d = String(beijingTime.getDate()).padStart(2, "0");

  return `(${y}-${m}-${d})`;
}

/**
 * 将 BtDataType 格式化为字符串数组
 */
export function formatBtData(btdata: BtDataType): string[] {
  if (!btdata || typeof btdata !== "object") return [];

  return Object.entries(btdata).map(([key, entries]) => {
    const line = entries
      // 过滤掉没有链接的
      .filter((entry) => entry.Message?.link || entry.TGMegLink)
      // 排序
      .sort(compareEpisode)
      // 格式化输出
      .map((entry) => {
        const link = entry.Message?.link ?? entry.TGMegLink!;
        return `[${entry.episode}](${link})`;
      })
      .join(" | ");

    const section = `[#${safeTag(key)}]\n${line}`;

    // 给资源段的每一行都加引用前缀（"> "）
    return section
      .split("\n")
      .map((l) => (l.trim() ? `> ${l}` : l))
      .join("\n");
  });
}

/**
 * 解析集数字符串，提取数字、版本、特殊剧集类型
 */
function parseEpisode(
  ep: string
):
  | { type: "num"; num: number; version: string }
  | { type: "sp"; num: number }
  | { type: "other"; raw: string } {
  const match = ep.match(/^(\d+)(v\d+)?$/i);
  if (match)
    return {
      type: "num",
      num: parseInt(match[1], 10),
      version: match[2] ?? "",
    };

  const spMatch = ep.match(/^SP(\d+)$/i);
  if (spMatch) return { type: "sp", num: parseInt(spMatch[1], 10) };

  const specialMatch = ep.match(/^特别篇(\d+)$/);
  if (specialMatch) return { type: "sp", num: parseInt(specialMatch[1], 10) };

  return { type: "other", raw: ep };
}

/**
 * 集数比较函数
 * 支持数字集数、版本号、SP、特别篇及非数字集
 * 排序规则：
 * 1. 数字集数优先，按数字升序
 * 2. 数字相同，按版本号排序 (如 03 < 03v2)
 * 3. 特殊集 (SP, 特别篇) 紧跟在数字集数后面
 * 4. 其他（电影、剧场版等）排在最后
 */
function compareEpisode(a: BtEntry, b: BtEntry): number {
  const ea = parseEpisode(a.episode);
  const eb = parseEpisode(b.episode);

  if (ea.type === "num" && eb.type === "num") {
    if (ea.num !== eb.num) return ea.num - eb.num;
    return ea.version.localeCompare(eb.version); // version 已经保证是 string
  }

  if (ea.type === "num" && eb.type === "sp") {
    if (ea.num !== eb.num) return ea.num - eb.num;
    return -1;
  }
  if (ea.type === "sp" && eb.type === "num") {
    if (ea.num !== eb.num) return ea.num - eb.num;
    return 1;
  }

  if (ea.type === "sp" && eb.type === "sp") {
    return ea.num - eb.num;
  }

  if (ea.type !== "other" && eb.type === "other") return -1;
  if (ea.type === "other" && eb.type !== "other") return 1;

  return 0;
}

function formatTags(tags: string[]) {
  if (!Array.isArray(tags)) return "";

  return tags
    .map((t) => safeTag(t)) // 对每个标签进行格式化
    .filter((t) => t && !/^\d+$/.test(t)) // 过滤掉空值和纯数字标签
    .map((t) => `#${t}`)
    .join(" ");
}
function safeTag(text: string) {
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

// ----------------------------------------------------------------

/**
 * 生成动漫信息文本
 * @param anime - 数据库中动漫详细信息
 * @param item - 动漫在BT站中的信息
 * @returns - 格式化后的动漫信息文本
 */
export function AnimeText(anime: animeType, item: animeItem) {
  const nsfwTag = anime.r18 === true ? "#NSFW " : "";
  const text = `#${
    anime.airingStart
      ? anime.airingStart.replace(/(\d{4})年(\d{1,2})月.*/, "$1年$2月")
      : ""
  } ${nsfwTag} ${item.title}\n>原名称: ${anime.name}\n>中文名: ${
    anime.name_cn
  }\n>发布组: ${formatTags(item.fansub?.map((f) => safeTag(f)) || [])}${
    item.pubDate ? `\n>发布时间: ${item.pubDate}` : ""
  }\n\n追踪标签：\n>名称: #${safeTag(
    anime.name_cn || anime.name
  )}\n>番剧组: ${item.fansub
    ?.map(
      (f) =>
        `#${safeTag(f.replace(/\s+/g, "_"))}_${safeTag(
          anime.name_cn || anime.name
        )}`
    )
    .join(" ")}${
    anime.navMessage?.link || anime.navMessageLink
      ? ` **\n[番剧信息](${anime.navMessage?.link || anime.navMessageLink})`
      : ""
  }`;

  return text;
}
