import logger from "@log/index.ts";

import {
  getTagExcludeList,
  hasAnimeSend,
  hasTorrentTitle,
} from "../database/query.ts";

import {
  animeinfo,
  fetchBangumiTags,
  fetchBangumiTeam,
  fetchBangumiTorrent,
  getEpisodeInfo,
} from "./info.ts";

import { groupRules } from "./groupRules.ts";
import { updateAnimeBtdata } from "../database/update.ts";
import { addCacheItem, addTorrent, saveAnime } from "../database/create.ts";
import { getQBClient } from "../qBittorrent/index.ts";
import { getMessageLink } from "@TDLib/function/get.ts";
import { sendMessage } from "@TDLib/function/message.ts";
import { fetchMergedRss } from "./rss/index.ts";
import { sendMegToAnime, sendMegToNavAnime } from "./sendAnime.ts";
import { downloadTorrentFromUrl } from "./torrent.ts";

import type {
  RssAnimeItem,
  anime as animeType,
  animeItem,
  bangumiAnime,
  infobox,
} from "../types/anime.ts";
import type { Client } from "tdl";
import { parseTextEntities } from "@TDLib/function/index.ts";
import { ErrorHandler } from "../function/index.ts";
import { env } from "../database/initDb.ts";

export async function anime(client: Client) {
  while (true) {
    const rss = await fetchMergedRss();
    if (rss && Array.isArray(rss)) {
      const validItems = rss.filter(
        (item) => item && item.title && item.pubDate && item.type
      );
      await processItemsWithConcurrency(client, validItems, 3).catch(() => {});
    }

    await smartDelayWithInterval();
  }
}

/**
 * 2.控制并发数量的并循环处理动漫
 * @param client - TDLib 客户端实例
 * @param {Array} items - 待处理的动漫项数组
 * @param {number} maxConcurrency - 最大并发数
 */
async function processItemsWithConcurrency(
  client: Client,
  items: RssAnimeItem[],
  maxConcurrency: number
) {
  const executing = new Set();
  logger.debug(
    `开始处理 ${items.length} 个RSS动漫项，最大并发数: ${maxConcurrency}`
  );

  for (const item of items) {
    // 创建处理 Promise
    const promise = handleRssAnimeItem(client, item)
      .catch((err: unknown) => {
        logger.error(
          `处理RSS动漫项失败: ${item.title},json:${JSON.stringify(
            item,
            null,
            2
          )}`,
          err
        );
        ErrorHandler(client, err);
      })
      .finally(() => {
        // 任务完成后从执行集合中移除
        executing.delete(promise);
      });

    // 添加到执行集合
    executing.add(promise);

    // 如果达到最大并发数，等待其中一个完成
    if (executing.size >= maxConcurrency) {
      await Promise.race(executing);
    }
  }

  // 等待所有剩余任务完成
  await Promise.all(executing);
  return;
}

/**
 * 3.处理单个RSS动漫项
 * @param client - TDLib 客户端实例
 * @param {Object} item - 待处理的动漫项
 */
async function handleRssAnimeItem(client: Client, item: RssAnimeItem) {
  // 检查种子是否已存在
  const torrentExists = await hasTorrentTitle(item.title);

  if (torrentExists) {
    // 种子已存在，跳过处理
    return;
  }

  let newitem: animeItem;

  // 从标题中提取字幕组信息
  let fansub = null;
  const match = item.title.match(/^(?:\[([^\]]+)\]|【([^】]+)】)/);
  if (!match) {
    return; // 跳过无法解析的条目
  }
  if (match) {
    const raw = match[1] || match[2];
    fansub = raw
      .split(/\s*[&/|｜、]\s*/)
      .map((s) => s.trim())
      .filter(Boolean);
  }

  if (fansub === null || fansub.length === 0) {
    return;
  }

  // 处理 bangumi 类型的 RSS 动漫项
  if (item.type === "bangumi") {
    const torrentInfo = await fetchBangumiTorrent(item.id);
    // 获取作者信息

    // 提取发布组信息
    let team = [];
    if (torrentInfo.team_id) {
      team = await fetchBangumiTeam(torrentInfo.team_id);
    } else {
      team = [{ name: fansub[0] }];
    }
    const tags =
      torrentInfo.tag_ids && torrentInfo.tag_ids.length > 0
        ? await fetchBangumiTags(torrentInfo.tag_ids)
        : [];

    // 从tags中找到type为"bangumi"的项目，提取locale信息
    const bangumiTag = tags.find(
      (tag: {
        type?: string;
        locale?: { zh_cn?: string; ja?: string; en?: string };
      }) => tag.type === "bangumi"
    );
    const nameLocales = bangumiTag
      ? {
          cn: bangumiTag.locale.zh_cn || "",
          jp: bangumiTag.locale.ja || "",
          en: bangumiTag.locale.en || "",
        }
      : {
          cn: "",
          jp: "",
          en: "",
        };

    const infoq = parseInfo(item.title, team[0]?.name);
    if (!infoq) {
      return;
    }
    // 将 nameLocales 中每个语言的文本追加到 infoq.names 中，去重并去空
    const localeNames = [nameLocales.cn, nameLocales.jp, nameLocales.en]
      .filter((s) => typeof s === "string" && s.trim() !== "")
      .map((s) => s.trim());

    infoq.names = Array.isArray(infoq.names)
      ? infoq.names
          .map((s) => (typeof s === "string" ? s.trim() : ""))
          .filter(Boolean)
      : [];

    infoq.names = Array.from(new Set([...infoq.names, ...localeNames])).filter(
      Boolean
    );

    // 白名单机制：如果最终 names 为空，跳过该条目
    if (!infoq.names || infoq.names.length === 0) {
      return;
    }

    newitem = {
      title: item.title,
      pubDate: item.pubDate,
      magnet: torrentInfo.magnet,
      team: team[0]?.name,
      fansub,
      ...infoq,
    };
  } else if (item.type === "dmhy" || item.type === "acgnx") {
    // 处理 动漫花园 与 末日动漫 的 RSS 动漫项
    const infoq = parseInfo(item.title, item.author);
    if (!infoq) {
      return;
    }
    newitem = {
      title: item.title,
      pubDate: item.pubDate,
      magnet: item.magnet,
      team: item.author,
      fansub,
      ...infoq,
    };
  } else {
    return;
  }
  // 判断是否为新番
  await animeDownload(client, newitem);
  return;
}

/**
 * 4.下载动漫并判断是否为新番
 * @param client - TDLib 客户端实例
 * @param {Object} item - 动漫项
 */
async function animeDownload(client: Client, item: animeItem) {
  // 检查动漫是否存在
  const existingAnime = await hasAnimeSend(item.names);

  if (!existingAnime) {
    await newAnimeHasBeenSaved(client, item);
    return;
  } else {
    await updateAnime(client, existingAnime, item);
    return;
  }
}

/**
 * 5. 如果是新番剧
 * @param client - TDLib 客户端实例
 * @param item - 动漫项
 * @returns - 是否已保存
 */
async function newAnimeHasBeenSaved(client: Client, item: animeItem) {
  const searchAnime = await animeinfo(item.names[0]);

  const Cache_id = await addCacheItem(item);

  await addTorrent(item.magnet, "等待下载", item.title);

  if (!searchAnime.data || searchAnime.data.length === 0) {
    await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
      invoke: {
        reply_markup: {
          _: "replyMarkupInlineKeyboard",
          rows: [
            [
              {
                _: "inlineKeyboardButton",
                text: "点击提供",
                type: {
                  _: "inlineKeyboardButtonTypeCallback",
                  data: Buffer.from(`N_anime?c=${Cache_id}`).toString("base64"),
                },
              },
            ],
          ],
        },
        message_thread_id: Number(env.data.NAV_GROUP_THREAD_ID),
        input_message_content: {
          _: "inputMessageText",
          text: await parseTextEntities(
            client,
            `当前番剧为${item.title}\n\n未搜索到的动漫信息\n请手动提供一个`
          ),
          link_preview_options: {
            _: "linkPreviewOptions",
            is_disabled: true,
          },
        },
      },
    });
    return;
  }

  const anime = await buildAndSaveAnimeFromInfo(searchAnime.data[0], true);

  // 下载种子文件并获取下载路径
  const torrent = await downloadTorrentFromUrl(item.magnet, item.title);
  if (!torrent) {
    logger.error(`种子下载失败: ${item.title}, magnet: ${item.magnet}`);
    return;
  }

  const animeMeg = await sendMegToAnime(
    client,
    anime,
    item,
    torrent.raw.content_path,
    true
  );

  const QBclient = await getQBClient();
  QBclient.removeTorrent(torrent.id, true);

  if (!animeMeg) {
    logger.error("发送动漫消息失败");
    return;
  }

  const animeLink = await getMessageLink(client, animeMeg.chat_id, animeMeg.id);

  await updateAnimeBtdata(
    anime.id,
    combineFansub(item.fansub),
    item.episode || "未知",
    {
      chat_id: animeMeg.chat_id,
      message_id: animeMeg.id,
      thread_id: animeMeg.topic_id
        ? animeMeg.topic_id._ === "messageTopicForum"
          ? animeMeg.topic_id.forum_topic_id
          : 0
        : 0,
      link: animeLink.link,
    },
    item.title,
    item.source,
    item.names,
    animeMeg.content._ === "messageVideo"
      ? animeMeg.content.video.video.remote.id
      : undefined,
    animeMeg.content._ === "messageVideo"
      ? animeMeg.content.video.video.remote.unique_id
      : undefined,
    Cache_id,
    true
  );

  await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
    invoke: {
      reply_markup: {
        _: "replyMarkupInlineKeyboard",
        rows: [
          [
            {
              _: "inlineKeyboardButton",
              text: "正确",
              type: {
                _: "inlineKeyboardButtonTypeCallback",
                data: Buffer.from(
                  `Y_anime?id=${anime.id}&c=${Cache_id}`
                ).toString("base64"),
              },
            },
            {
              _: "inlineKeyboardButton",
              text: "错误",
              type: {
                _: "inlineKeyboardButtonTypeCallback",
                data: Buffer.from(
                  `F_anime?id=${anime.id}&c=${Cache_id}`
                ).toString("base64"),
              },
            },
          ],
        ],
      },
      message_thread_id: Number(env.data.NAV_GROUP_THREAD_ID),
      input_message_content: {
        _: "inputMessageText",
        text: await parseTextEntities(
          client,
          `当前番剧为${item.title}\n\n搜索到的动漫信息：\n\n**名称：** [${
            searchAnime.data[0].name_cn || searchAnime.data[0].name
          }](https://bgm.tv/subject/${searchAnime.data[0].id})\n**ID：** ${
            searchAnime.data[0].id
          }\n\n请确认是否正确`
        ),
        link_preview_options: {
          _: "linkPreviewOptions",
          is_disabled: true,
        },
      },
    },
  });
  return;
}

/**
 * 5.1 新番剧更新动漫信息中构建并保存动漫数据
 * @param info - 动漫信息
 * @param newanime - 是否为新番剧如果你需要直接保存到anime集合中而不是缓存传递 false
 */
export async function buildAndSaveAnimeFromInfo(
  info: bangumiAnime,
  newanime: boolean
) {
  const EpisodeInfo = await getEpisodeInfo(info.id);
  const infobox = extractInfoFromInfobox(info?.infobox || []);
  const anime: animeType = {
    id: info.id,
    name_cn: info.name_cn || infobox.name,
    name: info.name,
    names: [
      ...new Set(
        [info.name_cn, info.name, ...infobox.names].filter((x): x is string =>
          Boolean(x)
        )
      ),
    ],
    image:
      info.images?.large ||
      info.images?.medium ||
      info.images?.common ||
      "https://dummyimage.com/350x600/cccccc/ffffff&text=%E6%97%A0%E5%B0%81%E9%9D%A2",
    summary: info.summary
      ? info.summary
          .replace(/\r\n/g, "\\n")
          .replace(/\n/g, "\\n")
          .replace(/\r/g, "\\n")
      : undefined,

    tags: info.tags ? (await extractFilteredTagNames(info.tags)) || [] : [],
    episode: infobox.episodeCount || undefined,
    eps: {
      total: EpisodeInfo.total || 0,
      list: EpisodeInfo.data.map((ep: any) => ({
        airdate: ep.airdate,
        name: ep.name,
        name_cn: ep.name_cn,
        duration: ep.duration,
        desc: ep.desc,
        ep: ep.ep,
        sort: ep.sort,
        id: ep.id,
        subject_id: ep.subject_id,
        comment: ep.comment,
      })),
    },
    score: info.rating?.score,
    navMessageLink: undefined,
    airingDay: infobox.broadcastDay || undefined,
    airingStart: infobox.broadcastStart || undefined,
  };
  if (newanime) {
    await saveAnime(anime, true);
    return anime;
  }
  await saveAnime(anime);
  return anime;
}

/**
 * 6.1 对于不是新番剧，更新动漫信息
 * @param client - TDLib 客户端实例
 * @param anime - 动漫信息
 * @param item - 动漫项
 */
export async function updateAnime(
  client: Client,
  anime: animeType,
  item: animeItem
) {
  // 下载种子文件并获取下载路径
  const Torrent = await downloadTorrentFromUrl(item.magnet, item.title);

  if (!Torrent) {
    logger.error(`种子下载失败: ${item.title}, magnet: ${item.magnet}`);
    throw new Error(` 种子下载失败: ${item.title}`);
  }

  const animeMeg = await sendMegToAnime(
    client,
    anime,
    item,
    Torrent.raw.content_path
  );

  if (!animeMeg) {
    throw new Error(`发送动漫消息失败${item.title}`);
  }

  // remove data on disk
  const QBclient = await getQBClient();
  await QBclient.removeTorrent(Torrent.id, true);

  const animeLink = await getMessageLink(client, animeMeg.chat_id, animeMeg.id);

  await updateAnimeBtdata(
    anime.id,
    combineFansub(item.fansub),
    item.episode || "未知",
    {
      chat_id: animeMeg.chat_id,
      message_id: animeMeg.id,
      thread_id: animeMeg.topic_id
        ? animeMeg.topic_id._ === "messageTopicForum"
          ? animeMeg.topic_id.forum_topic_id
          : 0
        : 0,
      link: animeLink.link,
    },
    item.title,
    item.source,
    item.names,
    animeMeg.content._ === "messageVideo"
      ? animeMeg.content.video.video.remote.id
      : undefined,
    animeMeg.content._ === "messageVideo"
      ? animeMeg.content.video.video.remote.unique_id
      : undefined
  );
  await sendMegToNavAnime(client, anime.id);
  return;
}

/**
 *
 * 下方为辅助方法
 *
 */

/**
 * 提取 bgm 番剧相信信息中的动漫信息
 * @param infobox - 信息盒数组
 * @returns - 提取的动漫信息
 */
export function extractInfoFromInfobox(
  infoboxList: infobox[] | Array<{ key: string; value: any }>
) {
  type InfoboxItem = { key: string; value: any };

  const result: {
    name: string;
    names: string[];
    episodeCount: string;
    broadcastDay: string;
    broadcastStart: string;
  } = {
    name: "",
    names: [],
    episodeCount: "",
    broadcastDay: "",
    broadcastStart: "",
  };

  for (const item of infoboxList as InfoboxItem[]) {
    const key = String(item.key || "");
    const value = item.value;

    switch (key) {
      case "中文名":
        if (typeof value === "string") {
          result.name = value;
          result.names.push(value);
        }
        break;
      case "别名":
        if (Array.isArray(value)) {
          for (const alias of value) {
            // alias 可能是对象 { v: string } 或直接字符串
            if (alias && typeof alias === "object" && "v" in alias) {
              const v = (alias as any).v;
              if (typeof v === "string" && v.trim()) {
                result.names.push(v);
              }
            } else if (typeof alias === "string" && alias.trim()) {
              result.names.push(alias);
            }
          }
        } else if (typeof value === "string" && value.trim()) {
          // 有时别名可能是单个字符串
          result.names.push(value);
        }
        break;
      case "话数":
        if (typeof value === "string") {
          result.episodeCount = value;
        } else if (typeof value === "number") {
          result.episodeCount = String(value);
        }
        break;
      case "放送星期":
        if (typeof value === "string") {
          result.broadcastDay = value;
        }
        break;
      case "放送开始":
        if (typeof value === "string") {
          result.broadcastStart = value;
        }
        break;
      default:
        break;
    }
  }

  // 去重，去除空值
  result.names = [...new Set(result.names.filter(Boolean))];

  return result;
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

/**
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
 * 延迟函数
 * @param ms
 * @returns
 */
function delay(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * 格式化番剧信息
 * @param title - 动漫标题
 * @param teamName - 动漫发布组名称
 * @returns
 */
export function parseInfo(title: string, teamName: string | null) {
  let names = [];
  let source = "";
  let episode;

  // 查找对应的字幕组规则
  const teamKeys = Object.keys(groupRules);
  const teamKey = teamKeys.find(
    (key) => teamName && teamName.toLowerCase().includes(key.toLowerCase())
  );

  if (!teamKey || !groupRules[teamKey]) {
    return; // 明确返回空数组
  }

  // 使用字幕组规则解析标题
  try {
    names = groupRules[teamKey](title);

    // 集数提取（支持区间和版本号）
    // 先检查是否是多集区间（支持版本号）
    const multiEpMatch = title.match(
      /\[(\d{1,3}(?:v\d+)?)-(\d{1,3}(?:v\d+)?)\]|\((\d{1,3}(?:v\d+)?)-(\d{1,3}(?:v\d+)?)\)/
    );
    let epMatch = null;

    if (multiEpMatch) {
      // 多集区间，返回集数数组
      const startStr = multiEpMatch[1] || multiEpMatch[3];
      const endStr = multiEpMatch[2] || multiEpMatch[4];

      // 提取数字部分和版本号部分
      const startMatch = startStr.match(/^(\d{1,3})(v\d+)?$/);
      const endMatch = endStr.match(/^(\d{1,3})(v\d+)?$/);

      if (startMatch && endMatch) {
        const start = parseInt(startMatch[1]);
        const end = parseInt(endMatch[1]);
        const startVersion = startMatch[2] || "";
        const endVersion = endMatch[2] || "";

        episode = [];
        for (let i = start; i <= end; i++) {
          // 保持原有的格式（如果原来是01，保持01的格式）
          const padLength = startMatch[1].length;
          const paddedNum = i.toString().padStart(padLength, "0");

          // 如果是起始集数且有版本号，或者是结束集数且有版本号，保留版本号
          if (i === start && startVersion) {
            episode.push(paddedNum + startVersion);
          } else if (i === end && endVersion) {
            episode.push(paddedNum + endVersion);
          } else {
            episode.push(paddedNum);
          }
        }
      }
    } else {
      // 单集处理
      // 优先匹配 [数字+版本号] 格式的集数（如 [02v2]）
      epMatch = title.match(/\[(\d{1,3}(?:v+)?)\](?![^[]*\[)/);
      if (!epMatch) {
        // 适配 [03_卢恩城] 这种格式
        epMatch = title.match(/\[(\d{1,3})_/);
      }
      if (!epMatch) {
        // 适配 [03 - 总第13] 这种格式
        epMatch = title.match(/\[(\d{1,3})\s*-\s*总第\d+/);
      }
      if (!epMatch) {
        // 尝试匹配中文方括号 【数字+版本号】 格式（如 【14】）
        epMatch = title.match(/【(\d{1,3}(?:v\d+)?)】/);
      }
      if (!epMatch) {
        // 尝试匹配 - 数字+版本号 格式（如 - 02v2）
        epMatch = title.match(/\s-\s(\d{1,3}(?:v\d+)?)(?:\s|$|\(|\[|【)/);
      }
      if (!epMatch) {
        // 尝试匹配 [数字] 格式的集数（纯数字）
        epMatch = title.match(/\[(\d{1,3})\](?![^[]*\[)/);
      }
      if (!epMatch) {
        // 尝试匹配中文方括号 【数字】 格式（纯数字）
        epMatch = title.match(/【(\d{1,3})】/);
      }
      if (!epMatch) {
        // 尝试匹配 - 数字 格式（如黒ネズミたち）
        epMatch = title.match(/\s-\s(\d{1,3})(?:\s|$|\()/);
      }
      if (!epMatch) {
        // 如果没有以上格式，再尝试其他格式
        epMatch = title.match(/(?:第|EP|ep)(\d{1,3}(?:v\d+)?)(?:话|集|話|集)/i);
      }
      if (!epMatch) {
        // 最后尝试匹配独立的数字（带版本号），但排除在番剧名称中的数字
        const brackets = [...title.matchAll(/\[([^\]]+)\]/g)];
        for (let i = brackets.length - 1; i >= 0; i--) {
          const content = brackets[i][1];
          if (/^\d{1,3}(?:v\d+)?$/.test(content)) {
            epMatch = [null, content];
            break;
          }
        }
      }
      if (!epMatch) {
        // 尝试匹配中文方括号内的独立数字
        const chineseBrackets = [...title.matchAll(/【([^】]+)】/g)];
        for (let i = chineseBrackets.length - 1; i >= 0; i--) {
          const content = chineseBrackets[i][1];
          if (/^\d{1,3}(?:v\d+)?$/.test(content)) {
            epMatch = [null, content];
            break;
          }
        }
      }
      if (!epMatch) {
        // 最后尝试匹配独立的纯数字，但排除在番剧名称中的数字
        const brackets = [...title.matchAll(/\[([^\]]+)\]/g)];
        for (let i = brackets.length - 1; i >= 0; i--) {
          const content = brackets[i][1];
          if (/^\d{1,3}$/.test(content)) {
            epMatch = [null, content];
            break;
          }
        }
      }
    }
    episode = epMatch ? epMatch[1] : null;

    // 特殊集数提取（包含繁体变体）
    if (!episode) {
      // 剧场版、剧场总集篇等（支持简体/繁体与外文关键字）
      if (
        /(?:剧场版|劇場版|剧场总集篇|劇場總集篇|Gekijouban|Eiga|Movie|MOVIE)/i.test(
          title
        )
      ) {
        if (/(?:剧场总集篇|劇場總集篇)/i.test(title)) {
          episode = "剧场总集篇";
        } else if (/(?:剧场版|劇場版)/i.test(title)) {
          episode = "剧场版";
        } else if (/(?:Gekijouban|Eiga|Movie|MOVIE)/i.test(title)) {
          episode = "剧场版";
        }
      }
      // 电影（简体/繁体）
      else if (/(?:电影|電影)/i.test(title)) {
        episode = "电影";
      }
      // 特别篇 / 特別篇 / Special / SP
      else if (/(?:特别篇|特別篇|Special|SP)/i.test(title)) {
        episode = "SP";
      }
      // 番外、Extra、OVA、OAD、特典（带数字或不带数字）
      else {
        // OVA/OAD/SP/Extra/番外/特典 + 数字（如 OVA2、OAD03、SP1、Extra1、番外2），包含繁体/英文学写法
        const specialEp = title.match(
          /(?:OVA|OAD|SP|Extra|番外|特典)[\s\-:]?(\d{1,3})/i
        );
        if (specialEp) {
          episode = RegExp.$1 ? RegExp.lastMatch : specialEp[0];
        } else {
          // [OVA03]、[OAD01]、[SP1]、[Extra2]、[番外2] 这种括号内
          const bracketSpecial = title.match(
            /\[(OVA|OAD|SP|Extra|番外|特典)[\s\-:]?(\d{1,3})\]/i
          );
          if (bracketSpecial) {
            episode = bracketSpecial[1] + bracketSpecial[2];
          } else {
            // 小数集数，如 48.5
            const decimalEp = title.match(/[\s\-\[]?(\d{1,3}\.\d)[\s\)\]]/);
            if (decimalEp) {
              episode = decimalEp[1];
            }
          }
        }
      }
    }

    // 来源提取 - 改进的规则
    const sourcePatterns = [
      /\[([A-Za-z]+)\s+WEB-DL[^\]]*\]/i, // [Bilibili WEB-DL 1080P AVC 8bit AAC MKV] -> Bilibili
      /\[([A-Za-z]+)\]\[WEB-DL\]/i, // [Baha][WEB-DL] -> Baha
      /\(([A-Za-z-]+)\s+\d+x\d+/i, // (CR 1920x1080) -> CR, (B-Global 1920x1080) -> B-Global
      /\(([A-Za-z]+)\s+/i, // (ABEMA 1920x1080) -> ABEMA
    ];

    for (const pattern of sourcePatterns) {
      const match = title.match(pattern);
      if (match && match[1]) {
        const potentialSource = match[1].trim();
        // 排除技术标记，但允许平台名称
        if (
          !/^(WebRip|WEB-DL|MP4|MKV|AVC|HEVC|AAC|1080P|720P|CHT|CHS|GB|BIG5|x264|x265|10bit|8bit)$/i.test(
            potentialSource
          )
        ) {
          source = potentialSource;
          break;
        }
      }
    }

    // 如果上面的模式没有匹配到，尝试单独的[]标记
    if (!source) {
      const allBrackets = [...title.matchAll(/\[([^\]]+)\]/g)];
      for (const bracket of allBrackets) {
        const content = bracket[1].trim();
        // 检查是否是常见的来源平台
        if (
          /^(Baha|CR|Bilibili|Netflix|Amazon|Hulu|Funimation|iQIYI|Youku|ABEMA|B-Global)$/i.test(
            content
          )
        ) {
          source = content;
          break;
        }
      }
    }
  } catch (error) {
    logger.error(
      `❌ 解析出错: ${error instanceof Error ? error.message : String(error)}`
    );
    return; // 出错时也返回空
  }

  // 确保 episode 不为空，若为空则填充 "未知"
  if (Array.isArray(episode) && episode.length === 0) {
    episode = "未知";
  }
  if (
    episode === null ||
    episode === undefined ||
    (typeof episode === "string" && episode.trim() === "")
  ) {
    episode = "未知";
  }

  return { names, source, episode };
}
/**
 * 多个字幕组使用_链接
 * @param fansub
 * @returns
 */
function combineFansub(fansub: string[] | null) {
  if (!Array.isArray(fansub) || fansub.length === 0) return "";
  return fansub.join("_");
}
