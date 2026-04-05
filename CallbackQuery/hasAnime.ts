import {
  getAnimeById,
  getCacheItemById,
  getCacheResourceByCacheId,
} from "../database/query.ts";
import logger from "@log/index.ts";
import { answerCallbackQuery, chatoruserMdown } from "@TDLib/function/index.ts";
import {
  deleteMessage,
  editMessageText,
  sendMessage,
  sendMessageAlbum,
} from "@TDLib/function/message.ts";

import type { message, messages, messageSenderUser } from "tdlib-types";
import type {
  anime as animeType,
} from "../types/anime.d.ts";
import type { albumMessageType, messageType } from "../types/message.d.ts";
import type { Client } from "tdl";
import { saveAnime } from "../database/create.ts";
import { sendMegToAnime, sendMegToNavAnime } from "../anime/sendAnime.ts";
import { AnimeText } from "../anime/text.ts";
import { getMessageLink, getMessage } from "@TDLib/function/get.ts";
import {
  addAnimeNameAlias,
  updateAnimeBtdata,
  updateTorrentStatus,
} from "../database/update.ts";
import { deleteCacheAnime } from "../database/delete.ts";
import { getSubjectById } from "../anime/get.ts";

import { env } from "../database/initDb.ts";
import { buildAndSaveAnimeFromInfo } from "../utils/buildAnimeinfo.ts";
import { downloadAndValidateTorrent, removeTorrentAndData } from "../qBittorrent/download.ts";
import { getEpisodeById } from "../bangumi/get.ts";

function normalizeTdMessages(result: message | messages): message[] {
  if ((result as messages).messages && Array.isArray((result as messages).messages)) {
    return (result as messages).messages.filter((m): m is message => m !== null);
  }
  return [result as message];
}

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

  const episode = await getEpisodeById(id);

  const anime = await getAnimeById(episode.subject_id, true);

  if (!anime) {
    await answerCallbackQuery(client, queryId, {
      text: `失败出现错误`,
      show_alert: false,
    });
    throw new Error("anime不存在");
  }

  await editMessageText(client, chat_id, message_id, {
    text: `${anime.id} - ${anime.name
      } 正在更新\n触发用户：${await chatoruserMdown(client, sender_id, true)}`,
  });

  const result = await updateAnimeLinks(
    client,
    chat_id,
    message_id,
    anime,
    id,
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

  const episode = await getEpisodeById(id);

  const anime = await getAnimeById(episode.subject_id, true);

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
    )} ，请回复这一条消息提供正确的 bgm.tv 章节链接或章节id\n\n回复 /cancel 取消`,
  });

  let status = null;
  let newAnime = null;
  let newepid = null;

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
          /https?:\/\/(?:bgm\.tv|bangumi\.tv)\/ep\/(\d+)(?:\/|$)/i
        );
        if (m) parsedId = Number(m[1]);
      }

      if (!parsedId) {
        await editMessageText(client, chat_id, message_id, {
          text: "未识别的格式，请提供 ID（例如：502272）或 bgm 链接，或者使用 /cancel 取消",
        });
        continue;
      }
      newepid = await getEpisodeById(parsedId).catch(() => null);
      const Subject = await getSubjectById(newepid.subject_id).catch(() => null);
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

  const result = await updateAnimeLinks(
    client,
    chat_id,
    message_id,
    newAnime,
    newepid.id,
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
    )} ，请回复这一条消息提供正确的 bgm.tv 章节链接或章节id\n\n回复 /cancel 取消`,
  });

  let status = undefined;
  let newAnime = undefined;
  let newepid = null;

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
          /https?:\/\/(?:bgm\.tv|bangumi\.tv)\/ep\/(\d+)(?:\/|$)/i
        );
        if (m) parsedId = Number(m[1]);
      }

      if (!parsedId) {
        await editMessageText(client, chat_id, message_id, {
          text: "未识别的格式，请提供 ID（例如：502272）或 bgm 链接，或者使用 /cancel 取消",
        });
        continue;
      }
      newepid = await getEpisodeById(parsedId).catch(() => null);
      const Subject = await getSubjectById(newepid.subject_id).catch(() => null);
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
  const Torrent = await downloadAndValidateTorrent(item);

  if (!Torrent || !Torrent.content_path) {
    return;
  }

  const cacheAnimeMeg = await sendMegToAnime(
    client,
    newAnime,
    item,
    Torrent.content_path,
    newepid.id,
  );

  await removeTorrentAndData(Torrent.hash);

  if (!cacheAnimeMeg) {
    throw new Error("发送动漫消息失败");
  }

  const cacheAnimeMessages = normalizeTdMessages(cacheAnimeMeg);
  const primaryCacheAnimeMessage = cacheAnimeMessages[0];
  if (!primaryCacheAnimeMessage) {
    throw new Error("发送动漫消息失败: 无有效消息");
  }
  const result = await updateAnimeLinks(
    client,
    chat_id,
    message_id,
    newAnime,
    newepid.id,
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
 * 当前匹配错误进行纠正
 */
export async function nullEp(
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
    )} ，请回复这一条消息提供正确的缓存动漫 id 或 bgm.tv 的章节链接\n\n回复 /cancel 取消`,
  });

  let status = null;
  let newAnime = null;
  let parsedId = null;

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

      if (/^\d+$/.test(text)) {
        parsedId = Number(text);
      } else {
        const m = text.match(
          /https?:\/\/(?:bgm\.tv|bangumi\.tv)\/ep\/(\d+)(?:\/|$)/i
        );
        if (m) parsedId = Number(m[1]);
      }

      if (!parsedId) {
        await editMessageText(client, chat_id, message_id, {
          text: "未识别的格式，请提供 ID（例如：502272）或 bgm 链接，或者使用 /cancel 取消",
        });
        continue;
      }
      newAnime = await getEpisodeById(parsedId).catch(() => null);
      const Subject = await getSubjectById(newAnime.subject_id).catch(() => null);

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
  if (!newAnime || !parsedId) {
    await answerCallbackQuery(client, queryId, {
      text: `失败出现错误newAnime不存在`,
      show_alert: false,
    });
    return;
  }

  const result = await updateAnimeLinks(
    client,
    chat_id,
    message_id,
    newAnime,
    parsedId,
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
 * 从缓存频道的原始消息中提取视频文件 ID 和封面 ID
 * 封面优先取 cover.sizes 最后一项（分辨率最高），回退到 thumbnail
 */
async function fetchVideoInfosFromCache(
  client: Client,
  episodeEntry: {
    Message?: messageType;
    Messages?: albumMessageType[];
  }
): Promise<Array<{ videoId: string; coverId?: string; width?: number; height?: number; duration?: number }>> {
  const msgRefs: Array<{ chat_id: number; message_id: number }> = [];

  if (episodeEntry.Messages && episodeEntry.Messages.length > 0) {
    for (const m of episodeEntry.Messages) {
      msgRefs.push({ chat_id: m.chat_id, message_id: m.message_id });
    }
  } else if (episodeEntry.Message) {
    msgRefs.push({
      chat_id: episodeEntry.Message.chat_id,
      message_id: episodeEntry.Message.message_id,
    });
  }

  const results: Array<{ videoId: string; coverId?: string; width?: number; height?: number; duration?: number }> = [];

  for (const ref of msgRefs) {
    const msg = await getMessage(client, ref.chat_id, ref.message_id);
    if (!msg || msg.content._ !== "messageVideo") continue;

    const videoId = msg.content.video.video.remote.id;

    // 封面：优先取 cover.sizes 最后一项（最高分辨率），回退 thumbnail
    let coverId: string | undefined;
    const cover = msg.content.cover
    if (cover?.sizes && cover.sizes.length > 0) {
      coverId = cover.sizes[cover.sizes.length - 1].photo.remote.id;
    }

    results.push({
      videoId,
      coverId,
      width: msg.content.video.width,
      height: msg.content.video.height,
      duration: msg.content.video.duration,
    });
  }

  return results;
}

/**
 * 更新动漫链接
 * @param client - TDLib 客户端
 * @param chat_id - 聊天ID
 * @param message_id - 消息ID
 * @param anime - 动漫信息
 * @param episode_id - 剧集ID
 * @param cache_id - 缓存ID
 */
async function updateAnimeLinks(
  client: Client,
  chat_id: number,
  message_id: number,
  anime: animeType,
  episode_id: number,
  cache_id: number
) {
  const cacheItem = await getCacheItemById(cache_id);
  if (!cacheItem) {
    logger.error(`缓存信息不存在，ID: ${cache_id}`);
    throw new Error("缓存信息不存在");
  }
  const { createdAt: _createdAt, updatedAt: _updatedAt, ...newAnime } = anime;
  // 刷新动漫信息
  const animeId = await saveAnime(newAnime);

  await sendMegToNavAnime(client, animeId);
  const episodeData = await getCacheResourceByCacheId(
    anime.id,
    cache_id,
    cacheItem.title
  );

  if (
    !episodeData ||
    !episodeData.episode ||
    (!episodeData.episode.unique_id && (!episodeData.episode.unique_ids || episodeData.episode.unique_ids.length === 0)) ||
    !episodeData.episode.title
  ) {
    logger.error(`未找到对应的集数信息, ID: ${cache_id}\n
      episodeData: ${JSON.stringify(episodeData, null, 2)}`);
    throw new Error(`未找到对应的集数信息`);
  }

  const new_Anime = await getAnimeById(animeId);
  if (!new_Anime) {
    logger.error(`更新后动漫信息不存在，ID: ${animeId}`);
    throw new Error("更新后动漫信息不存在");
  }
  const animetext = AnimeText(new_Anime, cacheItem, episode_id);

  // 从缓存消息中获取真实视频文件 ID 和封面 ID
  const videoInfos = await fetchVideoInfosFromCache(client, episodeData.episode);
  if (videoInfos.length === 0) {
    logger.error(`无法从缓存消息中获取视频信息, cache_id: ${cache_id}`);
    throw new Error("无法从缓存消息中获取视频信息");
  }

  let allSentMessages: message[];
  let primaryAnimeMeg: message;

  if (videoInfos.length > 1) {

    const albumResult = await sendMessageAlbum(client, Number(env.data.ANIME_CHANNEL), {
      medias: videoInfos.map((info, index) => ({
        video: { id: info.videoId },
        ...(info.coverId ? { cover: { id: info.coverId } } : {}),
        width: info.width,
        height: info.height,
        duration: info.duration,
        supports_streaming: true,
        has_spoiler: new_Anime?.r18 === true || false,
        caption: index === 0 ? animetext : undefined,
      })),
    });
    if (!albumResult || !albumResult.messages || albumResult.messages.length === 0) {
      logger.error(`发送动漫消息失败: ${JSON.stringify(cacheItem, null, 2)}`);
      throw new Error("发送动漫消息失败");
    }
    allSentMessages = albumResult.messages.filter((m): m is message => m !== null);
    primaryAnimeMeg = allSentMessages[0];
  } else {
    // 单视频：sendMessage 携带封面
    const info = videoInfos[0];
    const singleMeg = await sendMessage(client, Number(env.data.ANIME_CHANNEL), {
      media: {
        video: { id: info.videoId },
        ...(info.coverId ? { cover: { id: info.coverId } } : {}),
        width: info.width,
        height: info.height,
        duration: info.duration,
        supports_streaming: true,
        has_spoiler: new_Anime?.r18 === true || false,
      },
      text: animetext,
    });
    if (!singleMeg) {
      logger.error(`发送动漫消息失败: ${JSON.stringify(cacheItem, null, 2)}`);
      throw new Error("发送动漫消息失败");
    }
    primaryAnimeMeg = singleMeg;
    allSentMessages = [singleMeg];
  }

  const sentMsgData: albumMessageType[] = allSentMessages.map((msg) => ({
    chat_id: msg.chat_id,
    message_id: msg.id,
    topic_id: msg.topic_id,
    videoid: msg.content._ === "messageVideo" ? msg.content.video.video.remote.id : undefined,
    unique_id: msg.content._ === "messageVideo" ? msg.content.video.video.remote.unique_id : undefined,
  }));
  const sentVideoids = sentMsgData.map((m) => m.videoid).filter((id): id is string => !!id);
  const sentUniqueIds = sentMsgData.map((m) => m.unique_id).filter((id): id is string => !!id);

  const newAnimeLink = await getMessageLink(
    client,
    primaryAnimeMeg.chat_id,
    primaryAnimeMeg.id
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
    episode_id,
    combineFansub(cacheItem.fansub),
    cacheItem.episode || "未知",
    {
      chat_id: primaryAnimeMeg.chat_id,
      message_id: primaryAnimeMeg.id,
      thread_id: primaryAnimeMeg.topic_id
        ? primaryAnimeMeg.topic_id._ === "messageTopicForum"
          ? primaryAnimeMeg.topic_id.forum_topic_id
          : 0
        : 0,
      link: newAnimeLink.link,
    },
    cacheItem.title,
    cacheItem.source,
    cacheItem.names,
    sentVideoids[0],
    sentUniqueIds[0],
    undefined,
    false,
    sentVideoids.length > 1 ? sentVideoids : undefined,
    sentUniqueIds.length > 1 ? sentUniqueIds : undefined,
    sentMsgData.length > 1 ? sentMsgData : undefined
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
