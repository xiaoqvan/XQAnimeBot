import logger from "@log/index.ts";

import { hasAnimeSend, hasTorrentTitle } from "../database/query.ts";

import {
  animeinfo,
  fetchBangumiTags,
  fetchBangumiTeam,
  fetchBangumiTorrent,
} from "./get.ts";

import { updateAnimeBtdata } from "../database/update.ts";
import { addCacheItem, addTorrent, saveAnime } from "../database/create.ts";
import { getMessageLink } from "@TDLib/function/get.ts";
import { sendMessage } from "@TDLib/function/message.ts";
import { fetchMergedRss } from "./rss/index.ts";
import { sendMegToAnime, sendMegToCache, sendMegToNavAnime } from "./sendAnime.ts";
import { env } from "../database/initDb.ts";

import type {
  RssAnimeItem,
  anime as animeType,
  animeItem,
} from "../types/anime.d.ts";
import type { Client } from "tdl";
import { parseInfo } from "../utils/animeParser.ts";
import { combineFansub, smartDelayWithInterval } from "../utils/index.ts";
import { buildAndSaveAnimeFromInfo } from "../utils/buildAnimeinfo.ts";
import { ErrorHandler } from "../utils/ErrorHandler.ts";
import { downloadAndValidateTorrent, removeTorrentAndData } from "../qBittorrent/download.ts";
import { type EpisodeMatchResult, matchBangumiEpisode } from "../utils/matcher.ts";

/**
 * 启动处理动漫RSS源并发送消息
 * @param client TDLib 客户端实例
 */
export async function anime(client: Client) {
  while (true) {
    try {
      const rss = await fetchMergedRss();
      if (rss && Array.isArray(rss)) {
        const validItems = rss.filter(
          (item) => item && item.title && item.pubDate && item.type
        );
        await processItemsWithConcurrency(client, validItems, 3)
      }
      await smartDelayWithInterval();
    } catch (error) {
      logger.error("动漫处理主线程报错", error);
      ErrorHandler(client, error).catch();
      await smartDelayWithInterval();
    }
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
  const queue = [...items]; // 复制一份作为任务队列

  logger.debug(
    `开始处理 ${queue.length} 个RSS动漫项，最大并发数: ${maxConcurrency}`
  );

  // 创建 worker 函数，每个 worker 都是一个 Promise
  const worker = async () => {
    while (queue.length > 0) {
      const item = queue.shift();
      if (!item) continue;

      try {
        await handleRssAnimeItem(client, item);
      } catch (error) {
        logger.error(`处理动漫项出错: ${item.title}`, error);
        ErrorHandler(client, new Error(`处理动漫项出错: ${item.title}`)).catch();
      }
    }
  };

  // 创建一个 Promise 池
  const workers = Array(maxConcurrency)
    .fill(null)
    .map(() => worker());

  // 等待所有 worker 完成它们的工作
  await Promise.all(workers);

  logger.debug(`处理完成，共处理 ${items.length} 个RSS动漫项`);
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
    return;
  }

  let newitem: animeItem;

  // 从标题中提取字幕组信息
  let fansub = null;
  const match = item.title.match(/^(?:\[([^\]]+)]|【([^】]+)】)/);
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

/** 4.区分新番与更新番
 * @param client - TDLib 客户端实例
 * @param item - 动漫项
 */
async function animeDownload(client: Client, item: animeItem) {
  // 1. 查找番剧 (Subject Level)
  const anime = await hasAnimeSend(item.names);

  // ==========================================
  // 分支 A: 这是一个全新的番剧 (数据库没存过)
  // ==========================================
  if (!anime) {
    logger.info(`✨ 发现潜在新番: ${item.title}`);
    await handleNewAnime(client, item);
    return;
  }
  logger.info(`找到匹配的番剧，准备下载: ${item.title}`);
  await handleExistingAnime(client, item, anime);

}

/**
 * 5. 如果是新番剧
 * @param client - TDLib 客户端实例
 * @param item - 动漫项
 * @returns - 是否已保存
 */
async function handleNewAnime(client: Client, item: animeItem) {
  // 搜索番剧信息
  const searchAnime = await animeinfo(item.names[0]);

  // 添加缓存条目
  const Cache_id = await addCacheItem(item);

  // 添加下载任务避免重复检查
  await addTorrent(item.magnet, "等待下载", item.title);

  if (!searchAnime.data || searchAnime.data.length === 0) {
    // 未搜索到番剧信息，发送提示消息给管理员
    promptAdminProvideAnimeInfo(client, Cache_id, item);
    return;
  }
  const anime = await buildAndSaveAnimeFromInfo(searchAnime.data[0], true);

  // 下载种子文件并获取下载路径
  const torrent = await downloadAndValidateTorrent(item);
  if (!torrent) return;

  const animeMeg = await sendMegToCache(
    client,
    anime,
    item,
    torrent.raw.content_path,
  );
  removeTorrentAndData(torrent.id).catch();
  if (!animeMeg) {
    logger.error("发送动漫消息失败");
    throw new Error("发送动漫消息失败");
  }


  const animeLink = await getMessageLink(client, animeMeg.chat_id, animeMeg.id);




  await updateAnimeBtdata(
    anime.id,
    undefined,
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
  const matchResult = matchBangumiEpisode(anime, item.episode);
  if (matchResult.status !== "MATCHED") {
    await promptAdminConfirmAnimeEpisodes(client, anime, Cache_id, item, matchResult);
    return;
  }
  // 提示管理员确认动漫信息
  await promptAdminConfirmAnime(client, anime, matchResult.episodeId, Cache_id, item);
  return;
}

/** 6. 如果是已有的番剧
 * @param client - TDLib 客户端实例
 * @param item - 动漫项 
 */
export async function handleExistingAnime(client: Client, item: animeItem, anime: animeType) {
  const matchResult = matchBangumiEpisode(anime, item.episode);
  await addTorrent(item.magnet, "等待下载", item.title);

  const torrent = await downloadAndValidateTorrent(item);

  if (matchResult.status !== "MATCHED") {
    const animeMeg = await sendMegToCache(
      client,
      anime,
      item,
      torrent.raw.content_path,
    );
    if (!animeMeg) {
      logger.error("发送动漫消息失败");
      throw new Error("发送动漫消息失败");
    }
    const canimeid = await saveAnime(anime, true);
    const animeLink = await getMessageLink(client, animeMeg.chat_id, animeMeg.id);
    const Cache_id = await addCacheItem(item);
    await updateAnimeBtdata(
      canimeid,
      undefined,
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
    await removeTorrentAndData(torrent.id);
    await promptAdminConfirmAnimeEpisodes(client, anime, Cache_id, item, matchResult);
    return;
  }
  const animeMeg = await sendMegToAnime(
    client,
    anime,
    item,
    torrent.raw.content_path, matchResult.episodeId
  );

  if (!animeMeg) {
    await removeTorrentAndData(torrent.id);
    throw new Error(`发送动漫消息失败${item.title}`);
  }
  await removeTorrentAndData(torrent.id);

  const animeLink = await getMessageLink(client, animeMeg.chat_id, animeMeg.id);

  // 更新动漫的数据库信息
  await updateAnimeBtdata(
    anime.id,
    matchResult.episodeId,
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

/** 提示管理员审核动漫信息
 * @param client - TDLib 客户端实例
 * @param cacheId - 缓存ID
 * @param itemTitle - 动漫标题
 */
async function promptAdminProvideAnimeInfo(client: Client, cacheId: number, item: animeItem) {
  await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
    topic_id: {
      _: "messageTopicForum",
      forum_topic_id: Number(env.data.NAV_GROUP_THREAD_ID),
    },
    text: `番剧: ${item.title}\n集数: ${item.episode || "未获取到"}\n\n未搜索到的动漫信息\n请手动提供动漫信息`,
    link_preview: true,
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
                data: Buffer.from(`N_anime?c=${cacheId}`).toString("base64"),
              },
            },
          ],
        ],
      },
    },
  });
}

/** 提示管理员审核动漫信息
 * @param client - TDLib 客户端实例
 * @param anime - 动漫信息
 * @param episodeId - 集数ID
 * @param bgmInfo - 搜索到的动漫信息
 * @param cacheId - 缓存ID
 * @param item - 动漫项
 */
export async function promptAdminConfirmAnime(
  client: Client,
  anime: animeType,
  episodeId: number,
  cacheId: number,
  item: animeItem
) {
  // 查找该章节的 sort（作为集数显示）
  const epEntry = anime.eps?.list?.find(e => e.id === Number(episodeId));
  const epSort = epEntry?.sort ?? episodeId;
  await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
    topic_id: {
      _: "messageTopicForum",
      forum_topic_id: Number(env.data.NAV_GROUP_THREAD_ID),
    },
    text: `当前番剧为${item.title}\n\n搜索到的动漫信息：\n\n**名称：** [${anime.name_cn || anime.name
      }](https://bgm.tv/subject/${anime.id})\n**ID：** ${anime.id
      }\n剧集: [${epSort}](https://bgm.tv/ep/${episodeId})\n\n请确认是否正确`,
    link_preview: true,
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
                  `Y_anime?id=${episodeId}&c=${cacheId}`
                ).toString("base64"),
              },
            },
            {
              _: "inlineKeyboardButton",
              text: "错误",
              type: {
                _: "inlineKeyboardButtonTypeCallback",
                data: Buffer.from(
                  `F_anime?id=${episodeId}&c=${cacheId}`
                ).toString("base64"),
              },
            },
          ],
        ],
      },
    },
  });
}

/** 提示管理员审核动漫集数匹配情况
 * @param client - TDLib 客户端实例
 * @param anime - 动漫信息
 * @param cacheId - 缓存ID
 * @param item - 动漫项
 * @param matchResult - 集数匹配结果
 */
export async function promptAdminConfirmAnimeEpisodes(client: Client, anime: animeType, cacheId: number, item: animeItem, matchResult: EpisodeMatchResult) {
  if (matchResult.status === "MATCHED") return
  if (matchResult.status === "DATE_MISMATCH") {
    await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
      topic_id: {
        _: "messageTopicForum",
        forum_topic_id: Number(env.data.NAV_GROUP_THREAD_ID),
      },
      text: `当前番剧为${item.title}\n\n动漫信息：\n\n**名称：** [${anime.name_cn || anime.name
        }](https://bgm.tv/subject/${anime.id})\n**ID：** ${anime.id
        }\n匹配集数: ${item.episode}\n出现问题：${matchResult.msg}\n\n请确认是否正确`,
      link_preview: true,
      invoke: {
        reply_markup: {
          _: "replyMarkupInlineKeyboard",
          rows: [
            [
              {
                _: "inlineKeyboardButton",
                text: "提供正确",
                type: {
                  _: "inlineKeyboardButtonTypeCallback",
                  data: Buffer.from(
                    `Y_anime?id=${matchResult.episodeId}&c=${cacheId}`
                  ).toString("base64"),
                },
              },
            ],
          ],
        },
      },
    });
    return;
  }
  await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
    topic_id: {
      _: "messageTopicForum",
      forum_topic_id: Number(env.data.NAV_GROUP_THREAD_ID),
    },
    text: `当前番剧为${item.title}\n\n动漫信息：\n\n**名称：** [${anime.name_cn || anime.name
      }](https://bgm.tv/subject/${anime.id})\n**ID：** ${anime.id
      }\n匹配集数: ${item.episode}\n出现问题：${matchResult.msg}\n\n请提供正确的集数id`,
    link_preview: true,
    invoke: {
      reply_markup: {
        _: "replyMarkupInlineKeyboard",
        rows: [
          [
            {
              _: "inlineKeyboardButton",
              text: "提供正确",
              type: {
                _: "inlineKeyboardButtonTypeCallback",
                data: Buffer.from(
                  `N_ep?c=${cacheId}=id${anime.id}`
                ).toString("base64"),
              },
            },
          ],
        ],
      },
    },
  });
}