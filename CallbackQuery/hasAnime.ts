import { getAnimeById, getCacheItemById } from "../database/query.ts";
import logger from "@log/index.ts";
import { answerCallbackQuery, chatoruserMdown } from "@TDLib/function/index.ts";
import {
  deleteMessage,
  editMessageText,
  sendMessage,
} from "@TDLib/function/message.ts";

import type { messageSenderUser } from "tdlib-types";
import type { anime as animeType, messageType } from "../types/anime.d.ts";
import type { Client } from "tdl";
import { saveAnime } from "../database/create.ts";
import { sendMegToAnime, sendMegToNavAnime } from "../anime/sendAnime.ts";
import { AnimeText } from "../anime/text.ts";
import { getMessageLink } from "@TDLib/function/get.ts";
import {
  addAnimeNameAlias,
  updateAnimeBtdata,
  updateTorrentStatus,
} from "../database/update.ts";
import { findEpisodeByCacheId, omit } from "../function/index.ts";
import { deleteCacheAnime } from "../database/delete.ts";
import { getSubjectById } from "../anime/info.ts";
import { buildAndSaveAnimeFromInfo } from "../anime/index.ts";
import { downloadTorrentFromUrl } from "../anime/torrent.ts";
import { getQBClient } from "../qBittorrent/index.ts";

import { env } from "../database/initDb.ts";

const QBclient = await getQBClient();

/**
 * 当前匹配正确
 */
export async function trueAnime(
  client: Client,
  chat_id: number,
  sender_user_id: number,
  message_id: number,
  queryId: string,
  raw: string
) {
  const query = raw.includes("?") ? raw.split("?")[1] : raw;
  const params = new URLSearchParams(query);
  const id = Number(params.get("id"));
  const Cache_id = Number(params.get("c"));

  const sender_id: messageSenderUser = {
    _: "messageSenderUser",
    user_id: sender_user_id,
  };

  const anime = await getAnimeById(id, true);

  if (!anime) {
    await answerCallbackQuery(client, queryId, {
      text: `失败出现错误`,
      show_alert: false,
    });
    throw new Error("anime不存在");
  }

  await editMessageText(client, chat_id, message_id, {
    text: `${anime.id} - ${
      anime.name
    } 正在更新\n触发用户：${await chatoruserMdown(client, sender_id, true)}`,
  });

  const result = await updateAnimeLinks(
    client,
    chat_id,
    message_id,
    anime,
    Cache_id
  );

  if (!result) {
    await answerCallbackQuery(client, queryId, {
      text: `失败出现错误`,
      show_alert: false,
    });
    throw new Error("更新动漫链接失败");
  }
  await answerCallbackQuery(client, queryId, {
    text: `确认成功`,
    show_alert: false,
  });
  await editMessageText(client, chat_id, message_id, {
    text: `${anime.id} - ${
      anime.name
    } 更新完成\n触发用户：${await chatoruserMdown(client, sender_id, true)}`,
  });
}
/**
 * 当前匹配错误进行纠正
 */
export async function falseAnime(
  client: Client,
  chat_id: number,
  sender_user_id: number,
  message_id: number,
  queryId: string,
  raw: string
) {
  const query = raw.includes("?") ? raw.split("?")[1] : raw;
  const params = new URLSearchParams(query);
  const id = Number(params.get("id"));
  const Cache_id = Number(params.get("c"));

  const sender_id: messageSenderUser = {
    _: "messageSenderUser",
    user_id: sender_user_id,
  };

  const anime = await getAnimeById(id, true);

  if (!anime) {
    await answerCallbackQuery(client, queryId, {
      text: `失败出现错误`,
      show_alert: false,
    });
    return;
  }

  await editMessageText(client, chat_id, message_id, {
    text: `${await chatoruserMdown(
      client,
      sender_id,
      true
    )} ，请回复这一条消息提供正确的缓存动漫 id 或 bgm.tv 的链接\n\n回复 /cancel 取消`,
  });

  let status = null;
  let newAnime = null;

  // 临时用法后续建议包装该参数
  for await (const update of client.iterUpdates()) {
    if (
      update._ === "updateNewMessage" &&
      update.message.content._ === "messageText" &&
      update.message.chat_id === chat_id &&
      update.message.reply_to?._ === "messageReplyToMessage" &&
      update.message.reply_to?.message_id === message_id
    ) {
      deleteMessage(client, chat_id, update.message.id, true);
      const rawText = update.message.content?.text?.text;
      if (!rawText) continue; // 非文本消息忽略

      const text = rawText.trim();
      const cmd = text.split(/\s+/)[0].toLowerCase();

      if (cmd === "/cancel") {
        await editMessageText(client, chat_id, message_id, {
          text: "已取消",
        });
        status = "canceled";
        break;
      }

      // 支持纯数字 ID 或 bgm 链接
      let parsedId = null;
      if (/^\d+$/.test(text)) {
        parsedId = Number(text);
      } else {
        const m = text.match(
          /https?:\/\/(?:bgm\.tv|bangumi\.tv)\/subject\/(\d+)(?:\/|$)/i
        );
        if (m) parsedId = Number(m[1]);
      }

      if (!parsedId) {
        await editMessageText(client, chat_id, message_id, {
          text: "未识别的格式，请提供 ID（例如：502272）或 bgm 链接，或者使用 /cancel 取消",
        });
        continue;
      }
      const Subject = await getSubjectById(parsedId);
      if (!Subject) {
        await editMessageText(client, chat_id, message_id, {
          text: `未找到相关的动漫信息，请确认 ID: ${parsedId} 是否正确。\n请重新提供一个 ID 或 bgm 链接，或者使用 /cancel 取消`,
        });
        continue;
      }
      newAnime = await buildAndSaveAnimeFromInfo(Subject, false);
      break;
    }
  }
  if (status === "canceled") {
    await answerCallbackQuery(client, queryId, {
      text: `已取消`,
      show_alert: false,
    });
    return;
  }
  if (!newAnime) {
    await answerCallbackQuery(client, queryId, {
      text: `失败出现错误newAnime不存在`,
      show_alert: false,
    });
    return;
  }
  newAnime.btdata = anime.btdata;

  const result = await updateAnimeLinks(
    client,
    chat_id,
    message_id,
    newAnime,
    Cache_id
  );

  if (!result) {
    await answerCallbackQuery(client, queryId, {
      text: `失败出现错误`,
      show_alert: false,
    });
    return;
  }

  await answerCallbackQuery(client, queryId, {
    text: `成功更新动漫信息`,
    show_alert: false,
  });

  await deleteCacheAnime(newAnime.id, Cache_id);
}

/**
 * 处理未找到动漫的情况
 */
export async function nullAnime(
  client: Client,
  chat_id: number,
  sender_user_id: number,
  message_id: number,
  queryId: string,
  raw: string
) {
  const query = raw.includes("?") ? raw.split("?")[1] : raw;
  const params = new URLSearchParams(query);
  const Cache_id = Number(params.get("c"));

  const sender_id: messageSenderUser = {
    _: "messageSenderUser",
    user_id: sender_user_id,
  };

  const item = await getCacheItemById(Cache_id);

  if (!item) {
    await answerCallbackQuery(client, queryId, {
      text: `失败出现错误`,
      show_alert: false,
    });
    return;
  }

  await editMessageText(client, chat_id, message_id, {
    text: `${await chatoruserMdown(
      client,
      sender_id,
      true
    )} ，请回复这一条消息提供正确的缓存动漫 id 或 bgm.tv 的链接\n\n回复 /cancel 取消`,
  });

  let status = undefined;
  let newAnime = undefined;
  for await (const update of client.iterUpdates()) {
    if (
      update._ === "updateNewMessage" &&
      update.message.content._ === "messageText" &&
      update.message.chat_id === chat_id &&
      update.message.reply_to?._ === "messageReplyToMessage" &&
      update.message.reply_to?.message_id === message_id
    ) {
      const rawText = update.message.content?.text?.text;
      if (!rawText) continue; // 非文本消息忽略

      const text = rawText.trim();
      const cmd = text.split(/\s+/)[0].toLowerCase();

      if (cmd === "/cancel") {
        await editMessageText(client, chat_id, message_id, {
          text: "已取消",
        });
        status = "canceled";
        break;
      }

      // 支持纯数字 ID 或 bgm 链接
      let parsedId = undefined;
      if (/^\d+$/.test(text)) {
        parsedId = Number(text);
      } else {
        const m = text.match(
          /https?:\/\/(?:bgm\.tv|bangumi\.tv)\/subject\/(\d+)(?:\/|$)/i
        );
        if (m) parsedId = Number(m[1]);
      }

      if (!parsedId) {
        await editMessageText(client, chat_id, message_id, {
          text: "未识别的格式，请提供 ID（例如：502272）或 bgm 链接，或者使用 /cancel 取消",
        });
        continue;
      }
      const Subject = await getSubjectById(parsedId);
      if (!Subject) {
        await editMessageText(client, chat_id, message_id, {
          text: `未找到相关的动漫信息，请确认 ID: ${parsedId} 是否正确。\n请重新提供一个 ID 或 bgm 链接，或者使用 /cancel 取消`,
        });
        continue;
      }
      newAnime = await buildAndSaveAnimeFromInfo(Subject, false);
      break;
    }
  }
  if (status === "canceled") {
    await answerCallbackQuery(client, queryId, {
      text: `已取消`,
      show_alert: false,
    });
    return;
  }
  if (!newAnime) {
    await answerCallbackQuery(client, queryId, {
      text: `失败出现错误newAnime不存在`,
      show_alert: false,
    });
    return;
  }

  // 下载种子文件并获取下载路径
  const Torrent = await downloadTorrentFromUrl(item.magnet, item.title);

  if (!Torrent || !Torrent.raw.content_path) {
    return;
  }

  const cacheAnimeMeg = await sendMegToAnime(
    client,
    newAnime,
    item,
    Torrent?.raw.content_path,
    true
  );

  await QBclient.removeTorrent(Torrent.id, true);

  if (!cacheAnimeMeg) {
    throw new Error("发送动漫消息失败");
  }

  const animeLink = await getMessageLink(
    client,
    cacheAnimeMeg.chat_id,
    cacheAnimeMeg.id
  );

  newAnime.btdata = {
    [combineFansub(item.fansub)]: [
      {
        episode: item.episode || "未知",
        Message: {
          chat_id: cacheAnimeMeg.chat_id,
          message_id: cacheAnimeMeg.id,
          thread_id: cacheAnimeMeg.topic_id
            ? cacheAnimeMeg.topic_id._ === "messageTopicForum"
              ? cacheAnimeMeg.topic_id.forum_topic_id
              : 0
            : 0,
          link: animeLink.link,
        } as messageType,
        title: item.title,
        videoid:
          cacheAnimeMeg.content._ === "messageVideo"
            ? cacheAnimeMeg.content.video.video.remote.id
            : undefined,
        unique_id:
          cacheAnimeMeg.content._ === "messageVideo"
            ? cacheAnimeMeg.content.video.video.remote.unique_id
            : undefined,
      },
    ],
  };

  const result = await updateAnimeLinks(
    client,
    chat_id,
    message_id,
    newAnime,
    Cache_id
  );

  if (!result) {
    await answerCallbackQuery(client, queryId, {
      text: `失败出现错误`,
      show_alert: false,
    });
    return;
  }

  await answerCallbackQuery(client, queryId, {
    text: `成功更新动漫信息`,
    show_alert: false,
  });
}

/**
 * 更新动漫链接
 * @param anime - 动漫对象
 * @param chat_id - 聊天ID
 * @param message_id - 消息ID
 */
async function updateAnimeLinks(
  client: Client,
  chat_id: number,
  message_id: number,
  anime: animeType,
  cache_id: number
) {
  const cacheItem = await getCacheItemById(cache_id);
  if (!cacheItem) {
    logger.error(`缓存信息不存在，ID: ${cache_id}`);
    throw new Error("缓存信息不存在");
  }
  const newAnime = omit(anime, [
    "navMessageLink",
    "createdAt",
    "updatedAt",
    "btdata",
  ]);
  // 刷新动漫信息
  const animeId = await saveAnime(newAnime);

  await sendMegToNavAnime(client, animeId);
  if (!anime.btdata) {
    throw new Error("btdata不存在");
  }

  const episodeData = findEpisodeByCacheId(
    anime.btdata,
    cache_id,
    cacheItem.title
  );

  if (
    !episodeData ||
    !episodeData.episode ||
    !episodeData.episode.unique_id ||
    !episodeData.episode.title
  ) {
    logger.error(`未找到对应的集数信息, ID: ${cache_id}\nanime.btdata: ${JSON.stringify(
      anime.btdata,
      null,
      2
    )}\n
      episodeData: ${JSON.stringify(episodeData, null, 2)}`);
    throw new Error(`未找到对应的集数信息`);
  }

  const new_Anime = await getAnimeById(animeId);
  if (!new_Anime) {
    logger.error(`更新后动漫信息不存在，ID: ${animeId}`);
    throw new Error("更新后动漫信息不存在");
  }
  const animetext = AnimeText(new_Anime, cacheItem);

  const animeMeg = await sendMessage(client, Number(env.data.ANIME_CHANNEL), {
    media: {
      video: {
        id: episodeData.episode.videoid,
      },
    },
    text: animetext,
  });

  if (!animeMeg) {
    logger.error(`发送动漫消息失败: ${JSON.stringify(cacheItem, null, 2)}`);
    throw new Error("发送动漫消息失败");
  }

  const newAnimeLink = await getMessageLink(
    client,
    animeMeg.chat_id,
    animeMeg.id
  );

  // 添加别名
  await addAnimeNameAlias(new_Anime.id, cacheItem.names);

  // 更新当前处理的消息
  try {
    await editMessageText(client, chat_id, message_id, {
      text: `番剧: ${cacheItem.title}\n\n更新完成 ✅\n\n动漫id: ${animeId}\n消息为: ${newAnimeLink.link}`,
    });
  } catch (error) {
    logger.error("更新进度消息失败:", error);
    throw new Error("更新进度消息失败");
  }

  await updateAnimeBtdata(
    animeId,
    combineFansub(cacheItem.fansub),
    cacheItem.episode || "未知",
    {
      chat_id: animeMeg.chat_id,
      message_id: animeMeg.id,
      thread_id: animeMeg.topic_id
        ? animeMeg.topic_id._ === "messageTopicForum"
          ? animeMeg.topic_id.forum_topic_id
          : 0
        : 0,
      link: newAnimeLink.link,
    },
    cacheItem.title,
    cacheItem.source,
    cacheItem.names,
    animeMeg.content._ === "messageVideo"
      ? animeMeg.content.video.video.remote.id
      : undefined,
    animeMeg.content._ === "messageVideo"
      ? animeMeg.content.video.video.remote.unique_id
      : undefined
  );
  await sendMegToNavAnime(client, animeId);
  await deleteCacheAnime(anime.id, cache_id);
  await updateTorrentStatus(cacheItem.title, "完成");
  return true;
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
