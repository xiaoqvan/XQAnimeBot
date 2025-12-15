import logger from "@log/index.ts";
import { promises as fs } from "fs";
import { extname } from "path";

import { hasAnimeSend, hasTorrentTitle } from "../database/query.ts";

import {
  animeinfo,
  fetchBangumiTags,
  fetchBangumiTeam,
  fetchBangumiTorrent,
} from "./get.ts";

import { updateAnimeBtdata } from "../database/update.ts";
import { addCacheItem, addTorrent } from "../database/create.ts";
import { getQBClient } from "../qBittorrent/index.ts";
import { getMessageLink } from "@TDLib/function/get.ts";
import { sendMessage } from "@TDLib/function/message.ts";
import { fetchMergedRss } from "./rss/index.ts";
import { sendMegToAnime, sendMegToNavAnime } from "./sendAnime.ts";
import { downloadTorrentFromUrl } from "./torrent.ts";
import { env } from "../database/initDb.ts";

import type {
  RssAnimeItem,
  anime as animeType,
  animeItem,
} from "../types/anime.ts";
import type { Torrent } from "../types/torrent.ts";
import type { Client } from "tdl";
import { parseInfo } from "../utils/animeParser.ts";
import { combineFansub, smartDelayWithInterval } from "../utils/index.ts";
import { buildAndSaveAnimeFromInfo } from "../utils/buildAnimeinfo.ts";
import { ErrorHandler } from "../utils/ErrorHandler.ts";
import { mkvToMp4 } from "../utils/mkvtomp4.ts";

export async function anime(client: Client) {
  while (true) {
    try {
      const rss = await fetchMergedRss();
      if (rss && Array.isArray(rss)) {
        const validItems = rss.filter(
          (item) => item && item.title && item.pubDate && item.type
        );
        await processItemsWithConcurrency(client, validItems, 1).catch(
          () => {}
        );
      }

      await smartDelayWithInterval();
    } catch (error) {
      logger.error("处理RSS动漫项时出错", error);
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
      const item = queue.shift(); // 从队列头部取出一个任务
      if (!item) continue;

      try {
        // 直接等待任务完成（已移除 30 分钟超时）
        await handleRssAnimeItem(client, item);
      } catch (error) {
        // 如果是超时错误，记录警告并继续队列（避免重复处理）
        if (error instanceof Error && error.message.includes("处理超时")) {
          logger.warn(error.message);
        } else {
          logger.error(`处理动漫项 ${item.title} 时发生错误:`, error);
        }
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
    // 种子已存在，跳过处理
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

  // const searchdata = searchAnime.data?.map((anime) => ({
  //   id: anime.id,
  //   name: anime.name,
  //   name_cn: anime.name_cn,
  //   alias: anime.infobox
  //     ?.filter((info) => info.key === "别名")
  //     .flatMap((info) => {
  //       if (Array.isArray(info.value)) {
  //         return info.value
  //           .map((v) => {
  //             if (typeof v === "object" && v !== null && "v" in v) {
  //               return String(v.v);
  //             }
  //             return null;
  //           })
  //           .filter((v): v is string => v !== null);
  //       }
  //       return [];
  //     }),
  // }));

  const Cache_id = await addCacheItem(item);

  await addTorrent(item.magnet, "等待下载", item.title);

  if (!searchAnime.data || searchAnime.data.length === 0) {
    sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
      topic_id: {
        _: "messageTopicForum",
        forum_topic_id: Number(env.data.NAV_GROUP_THREAD_ID),
      },
      text: `当前番剧为${item.title}\n\n未搜索到的动漫信息\n请手动提供一个`,
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
                  data: Buffer.from(`N_anime?c=${Cache_id}`).toString("base64"),
                },
              },
            ],
          ],
        },
      },
    });
    return;
  }
  const anime = await buildAndSaveAnimeFromInfo(searchAnime.data[0], true);

  // 下载种子文件并获取下载路径
  const torrent = await downloadAndValidateTorrent(item);
  if (!torrent) return;

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
    topic_id: {
      _: "messageTopicForum",
      forum_topic_id: Number(env.data.NAV_GROUP_THREAD_ID),
    },
    text: `当前番剧为${item.title}\n\n搜索到的动漫信息：\n\n**名称：** [${
      searchAnime.data[0].name_cn || searchAnime.data[0].name
    }](https://bgm.tv/subject/${searchAnime.data[0].id})\n**ID：** ${
      searchAnime.data[0].id
    }\n\n请确认是否正确`,
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
    },
  });
  return;
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
  const torrent = await downloadAndValidateTorrent(item, {
    throwOnDownloadFail: true,
  });
  if (!torrent) return;

  const animeMeg = await sendMegToAnime(
    client,
    anime,
    item,
    torrent.raw.content_path
  );

  if (!animeMeg) {
    throw new Error(`发送动漫消息失败${item.title}`);
  }

  // remove data on disk
  const QBclient = await getQBClient();
  await QBclient.removeTorrent(torrent.id, true);

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

const TORRENT_MAX_SIZE = 2 * 1024 * 1024 * 1024;

async function downloadAndValidateTorrent(
  item: animeItem,
  options?: { throwOnDownloadFail?: boolean }
): Promise<Torrent | null> {
  const failMessage = `种子下载失败: ${item.title}, magnet: ${item.magnet}`;
  const torrent = await downloadTorrentFromUrl(item.magnet, item.title);
  if (!torrent) {
    logger.error(failMessage);
    if (options?.throwOnDownloadFail) {
      throw new Error(failMessage);
    }
    return null;
  }

  if (torrent.totalSize > TORRENT_MAX_SIZE) {
    logger.warn(
      `种子文件过大(${(torrent.totalSize / 1024 / 1024 / 1024).toFixed(
        2
      )}GB): ${item.title}, 已跳过`
    );
    const QBclient = await getQBClient();
    await QBclient.removeTorrent(torrent.id, true);
    return null;
  }

  if (torrent.raw.content_path) {
    try {
      const stats = await fs.stat(torrent.raw.content_path);
      if (stats.isDirectory()) {
        logger.warn(
          `下载路径是文件夹，跳过: ${torrent.raw.content_path} (${item.title})`
        );
        const QBclient = await getQBClient();
        await QBclient.removeTorrent(torrent.id, true);
        return null;
      }
      const fileExt = extname(torrent.raw.content_path).toLowerCase();
      if (stats.isFile() && fileExt === ".mkv") {
        logger.warn(
          `下载文件为 MKV，尝试转换为 MP4: ${torrent.raw.content_path} (${item.title})`
        );
        const video = await mkvToMp4(torrent.raw.content_path);
        fs.unlink(torrent.raw.content_path).catch(() => {});
        torrent.raw.content_path = video;
        return torrent;
      }
    } catch (err) {
      logger.error("检查下载路径类型时出错", err);
    }
  }

  return torrent;
}
