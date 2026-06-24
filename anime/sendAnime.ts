import fs from "fs/promises";

import logger from "@log/index.ts";

import {
  updateAnimeEpisodes,
  updateAnimeInfo,
  updateAnimeNavMessage,
  // updateAnimeNavMessageLink, // 不再使用链接单独更新
  updateAnimeNavVideoMessage, // 新增
  updateTorrentStatus,
} from "../database/update.ts";

import {
  editMessageCaption,
  editMessageText,
  sendMessage,
  sendMessageAlbum,
} from "@TDLib/function/message.ts";
import { getAnimeById } from "../database/query.ts";
import { AnimeText, navmegtext } from "./text.ts";
import { getEpisodeInfo, getSubjectById } from "./get.ts";
import { getMessageLink, getMessageLinkInfo } from "@TDLib/function/get.ts";
import { downloadFile, extractVideoMetadata } from "../function/index.ts";
import { env } from "../database/initDb.ts";

import type { anime as animeType } from "../types/anime.ts";
import type { messageType } from "../types/message.d.ts";
import type { animeItem } from "../types/rss.d.ts";

import type { Client } from "tdl";
import type { MessageContent } from "tdlib-types";

import { parseTextEntities } from "@TDLib/function/index.ts";

/**
 * 延迟函数，用于避免频繁编辑触发风控
 * @param ms - 延迟毫秒数
 */
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * 发送/更新 导航频道的消息
 *
 * @param client - TDLib 客户端实例
 * @param id - 数据库中动漫的id字段值
 * @returns 导航消息链接
 */
export async function sendMegToNavAnime(client: Client, id: number) {
  const Anime = await getAnimeById(id);

  if (!Anime) return;

  // 导航频道中有该番剧，编辑现有消息
  if (Anime.navMessage?.link) {
    // 更新评分
    const animeInfo = await getSubjectById(Anime.id);
    // 更新集数信息
    const episodeInfo = await getEpisodeInfo(Anime.id);

    // 并行持久化评分、简介和集数信息，并捕获错误避免影响后续导航消息更新
    await Promise.allSettled([
      updateAnimeInfo(Anime.id, animeInfo),
      updateAnimeEpisodes(Anime.id, episodeInfo),
    ]);

    // 同步内存中的 Anime 对象，确保导航消息文本使用最新数据
    Anime.score = animeInfo?.rating?.score ?? Anime.score;
    if (episodeInfo?.total) {
      Anime.episode = String(episodeInfo.total);
    }
    if (Array.isArray(episodeInfo?.data) && episodeInfo.data.length > 0) {
      Anime.eps = {
        total: episodeInfo.total,
        list: episodeInfo.data,
      };
    }
    const megtexts = await navmegtext(client, Anime); // megtexts[0] 为主导航，1.. 为资源

    if (!megtexts || !megtexts[0]) {
      logger.error(`Failed to generate navigation text for anime: ${Anime.name}`);
      throw new Error(`Failed to generate navigation text for anime: ${Anime.name}`);
    }

    // 主导航消息（应为 messagePhoto）：仅在文本变化时才编辑
    try {
      const navInfo = await getMessageLinkInfo(client, Anime.navMessage.link);
      const newCaptionText = await parseTextEntities(client, megtexts[0]);
      const oldCaptionText =
        navInfo?.message?.content?._ === "messagePhoto"
          ? navInfo.message.content.caption ?? ""
          : navInfo?.message?.content?._ === "messageText"
            ? // 兼容极端情况：历史主消息是文本
            navInfo.message.content.text ?? ""
            : "";

      if (oldCaptionText !== newCaptionText) {
        await editMessageCaption(
          client,
          Anime.navMessage.chat_id,
          Anime.navMessage.message_id,
          {
            text: megtexts[0],
          }
        );
        // 添加延迟避免频繁编辑触发风控
        await sleep(5000);
      }
    } catch {
      // 获取旧消息失败则按原逻辑尝试编辑
      await editMessageCaption(
        client,
        Anime.navMessage.chat_id,
        Anime.navMessage.message_id,
        {
          text: megtexts[0],
        }
      );
      // 添加延迟避免频繁编辑触发风控
      await sleep(5000);
    }

    // 没有就发送新的，有就修改（并补足多出来的）
    const existingVideoMsgs = Anime.navVideoMessage ?? [];
    let idx = 1;

    if (existingVideoMsgs.length > 0) {
      // 先修改已有的
      for (const videoMeg of existingVideoMsgs) {
        if (idx >= megtexts.length) break;
        // 文本消息（通常为 messageText），仅在变化时才编辑
        try {
          const info = await getMessageLinkInfo(client, videoMeg.link);
          const newText = await parseTextEntities(client, megtexts[idx]!);
          const content: MessageContent | undefined = info?.message?.content;
          if (content?._ === "messageText") {
            const oldText = content?.text ?? "";
            if (oldText !== newText) {
              await editMessageText(
                client,
                videoMeg.chat_id,
                videoMeg.message_id,
                {
                  text: megtexts[idx],
                  link_preview: true,
                }
              );
              // 添加延迟避免频繁编辑触发风控
              await sleep(5000);
            }
          } else if (content?._ === "messagePhoto") {
            const oldCaption = content?.caption ?? "";
            if (oldCaption !== newText) {
              await editMessageCaption(
                client,
                videoMeg.chat_id,
                videoMeg.message_id,
                {
                  text: megtexts[idx],
                }
              );
              // 添加延迟避免频繁编辑触发风控
              await sleep(5000);
            }
          } else {
            // 未知类型，保持兼容使用编辑文本
            await editMessageText(
              client,
              videoMeg.chat_id,
              videoMeg.message_id,
              {
                text: megtexts[idx],
                link_preview: true,
              }
            );
            // 添加延迟避免频繁编辑触发风控
            await sleep(5000);
          }
        } catch {
          // 获取旧消息失败则按原逻辑尝试编辑为文本
          await editMessageText(client, videoMeg.chat_id, videoMeg.message_id, {
            text: megtexts[idx],
            link_preview: true,
          });
          // 添加延迟避免频繁编辑触发风控
          await sleep(5000);
        }
        idx++;
      }
      // 如果 megtexts 有新增条目，则补发并写入数据库
      for (; idx < megtexts.length; idx++) {
        const videoMeg = await sendMessage(client, Anime.navMessage.chat_id, {
          topic_id: Anime.navMessage.topic_id,
          reply_to_message_id: Anime.navMessage.message_id,
          text: megtexts[idx],
          link_preview: true,
        });

        if (!videoMeg) {
          logger.error(
            `sendMegToNavAnime 补发导航频道消息失败: ${Anime.navMessage.chat_id}, ${Anime.id}`
          );
          continue;
        }

        const navLink = await getMessageLink(
          client,
          videoMeg.chat_id,
          videoMeg.id
        );
        await updateAnimeNavVideoMessage(Anime.id, [
          {
            page: idx, // 与 megtexts 的索引对应：1.. 为资源页
            chat_id: videoMeg.chat_id,
            message_id: videoMeg.id,
            topic_id: videoMeg.topic_id,
            link: navLink.link,
          },
        ]);
        const newAnimeinfo = await getAnimeById(Anime.id);

        if (newAnimeinfo) {
          const megtexts = await navmegtext(client, newAnimeinfo);

          await editMessageCaption(
            client,
            newAnimeinfo.navMessage!.chat_id,
            newAnimeinfo.navMessage!.message_id,
            {
              text: megtexts[0],
            }
          );
          // 添加延迟避免频繁编辑触发风控
          await sleep(5000);
        }
      }
    } else {
      // 没有历史视频消息，全部按顺序发送，并写入数据库
      for (idx = 1; idx < megtexts.length; idx++) {
        const videoMeg = await sendMessage(client, Anime.navMessage.chat_id, {
          text: megtexts[idx],
          reply_to_message_id: Anime.navMessage.message_id,
          topic_id: Anime.navMessage.topic_id,
          invoke: {
            reply_to: {
              _: "inputMessageReplyToMessage",
              message_id: Anime.navMessage.message_id,
            },
          },
        });

        if (!videoMeg) {
          logger.error(
            `sendMegToNavAnime 补发导航频道消息失败: ${Anime.navMessage.chat_id}, ${Anime.id}`
          );
          continue;
        }

        const navLink = await getMessageLink(
          client,
          videoMeg.chat_id,
          videoMeg.id
        );
        await updateAnimeNavVideoMessage(Anime.id, [
          {
            page: idx,
            chat_id: videoMeg.chat_id,
            message_id: videoMeg.id,
            topic_id: videoMeg.topic_id,
            link: navLink.link,
          },
        ]);

        const newAnimeinfo = await getAnimeById(Anime.id);

        if (newAnimeinfo) {
          const megtexts = await navmegtext(client, newAnimeinfo);

          await editMessageCaption(
            client,
            newAnimeinfo.navMessage!.chat_id,
            newAnimeinfo.navMessage!.message_id,
            {
              text: megtexts[0],
            }
          );
          // 添加延迟避免频繁编辑触发风控
          await sleep(5000);
        }
      }
    }

    return Anime.navMessage?.link;
  }

  // 导航频道中没有的番剧，新动漫发送逻辑
  let navmeg = null;
  let localImagePath: string | null = null;
  const megtexts = await navmegtext(client, Anime);

  // 首先尝试使用远程图片（caption 只使用首条 megtexts[0]）
  navmeg = await sendMessage(client, Number(env.data.NAV_CHANNEL), {
    text: megtexts[0],
    media: {
      photo: {
        id: Anime.image,
      },
    },
  });

  // 如果远程图片发送失败，尝试下载到本地
  if (!navmeg) {
    try {
      localImagePath = await downloadFile(Anime.image);

      // 使用本地图片发送
      navmeg = await sendMessage(client, Number(env.data.NAV_CHANNEL), {
        text: megtexts[0],
        media: {
          photo: {
            path: localImagePath,
          },
        },
      });
    } catch (localError) {
      logger.error(localError, `本地图片上传也失败: ${Anime.image}`);
      throw localError;
    } finally {
      // 清理本地图片文件
      if (localImagePath) {
        await fs.unlink(localImagePath).catch(() => { });
      }
    }
  }

  if (!navmeg) {
    throw new Error("发送导航消息失败");
  }

  // 获取首条（图片）消息链接并写入 navMessage
  const navLink = await getMessageLink(client, navmeg.chat_id, navmeg.id);
  const navMessage: messageType = {
    chat_id: navmeg.chat_id,
    message_id: navmeg.id,
    topic_id: navmeg.topic_id,
    link: navLink.link,
  };
  await updateAnimeNavMessage(Anime.id, navMessage);

  // 继续发送后续文本消息，并写入 navVideoMessage
  for (let i = 1; i < megtexts.length; i++) {
    const videoMeg = await sendMessage(client, navmeg.chat_id, {
      text: megtexts[i],
      reply_to_message_id: navmeg.id,
      topic_id: navmeg.topic_id,
    });

    if (!videoMeg) {
      logger.error(
        new Error(`补发导航频道消息失败: ${navmeg.chat_id}, ${Anime.id}, index=${i}`)
      );
      continue;
    }

    const link = await getMessageLink(client, videoMeg.chat_id, videoMeg.id);
    await updateAnimeNavVideoMessage(Anime.id, [
      {
        page: i,
        chat_id: videoMeg.chat_id,
        message_id: videoMeg.id,
        topic_id: videoMeg.topic_id,
        link: link.link,
      },
    ]);
  }

  return navLink.link;
}

/**
 * 发送动漫视频到动漫频道
 * 超时时间为 30 分钟，失败或超时后自动重试一次
 * @param client - TDLib 客户端实例
 * @param anime - 数据库中动漫详细信息
 * @param item - 动漫在BT站中的信息
 * @param videoPath - 种子完整信息
 * @param episodeId - 集数ID
 * @param segments - 可选的分段视频路径数组，如果提供则发送分段视频而非单一视频
 */
export async function sendMegToAnime(
  client: Client,
  anime: animeType,
  item: animeItem,
  videoPath: string,
  episodeId: number,
  segments?: string[]
) {
  await updateTorrentStatus(item.title, "上传中");

  const coverPaths: string[] = [];
  let sendOnce: () => ReturnType<typeof sendMessage | typeof sendMessageAlbum>;

  if (segments) {
    const text = AnimeText(anime, item, episodeId);
    const videoInfos: Awaited<ReturnType<typeof extractVideoMetadata>>[] = [];
    for (const path of segments) {
      const videoInfo = await extractVideoMetadata(path);
      videoInfos.push(videoInfo);
      coverPaths.push(videoInfo.coverPath);
    }
    sendOnce = () =>
      sendMessageAlbum(client, Number(env.data.ANIME_CHANNEL), {
        timeout: 1800,
        medias: videoInfos.map((videoInfo, index) => ({
          video: { path: segments[index] },
          cover: { path: videoInfo.coverPath },
          width: videoInfo.width,
          height: videoInfo.height,
          duration: Math.floor(videoInfo.duration),
          supports_streaming: true,
          has_spoiler: anime?.r18 === true || false,
          caption: index === 0 ? text : undefined,
        })),
      });
  } else {
    const videoInfo = await extractVideoMetadata(videoPath);
    coverPaths.push(videoInfo.coverPath);
    const text = AnimeText(anime, item, episodeId);
    sendOnce = () =>
      sendMessage(client, Number(env.data.ANIME_CHANNEL), {
        text,
        timeout: 1800,
        media: {
          video: { path: videoPath },
          cover: { path: videoInfo.coverPath },
          width: videoInfo.width,
          height: videoInfo.height,
          duration: Math.floor(videoInfo.duration),
          supports_streaming: true,
          has_spoiler: anime?.r18 === true || false,
        },
      });
  }

  let result;
  try {
    result = await sendOnce();
  } catch (firstError) {
    logger.warn(firstError, `sendMegToAnime 首次发送失败，准备重试: ${item.title}`);
    result = await sendOnce();
  }

  await updateTorrentStatus(item.title, "完成");

  // 统一清理视频文件和封面
  const videoPaths = segments ?? [videoPath];
  for (const p of videoPaths) fs.unlink(p).catch(() => { });
  for (const p of coverPaths) fs.unlink(p).catch(() => { });

  return result;
}

/** 发送动漫视频到缓存频道
 * @param client - TDLib 客户端实例
 * @param anime - 数据库中动漫详细信息
 * @param item - 动漫在BT站中的信息
 * @param videoPath - 种子完整信息
 * @param segments - 可选的分段视频路径数组，如果提供则发送分段视频而非单一视频
 */
export async function sendMegToCache(
  client: Client,
  anime: animeType,
  item: animeItem,
  videoPath: string,
  segments?: string[]
) {
  await updateTorrentStatus(item.title, "上传中");
  const cacheTopicId = Number(env.data.ANIME_GROUP_THREAD_ID);
  const validCacheTopic =
    Number.isFinite(cacheTopicId) && cacheTopicId > 0
      ? { _: "messageTopicForum" as const, forum_topic_id: cacheTopicId }
      : undefined;
  if (segments) {
    const videoInfos = [];
    for (const path of segments) {
      const videoInfo = await extractVideoMetadata(path);
      videoInfos.push(videoInfo);
    }
    const animeMessages = await sendMessageAlbum(
      client,
      Number(env.data.ADMIN_GROUP_ID),
      {
        topic_id: validCacheTopic,
        timeout: 3600,
        medias: videoInfos.map((videoInfo, index) => ({
          video: {
            path: segments[index],
          },
          cover: {
            path: videoInfo.coverPath,
          },
          width: videoInfo.width,
          height: videoInfo.height,
          duration: Math.floor(videoInfo.duration),
          supports_streaming: true,
          has_spoiler: anime?.r18 === true || false,
          caption: index === 0 ? item.title : undefined,
        }))
      }
    )

    // 清理 segments 文件和封面
    for (const path of segments) {
      fs.unlink(path).catch(() => { });
    }
    for (const videoInfo of videoInfos) {
      fs.unlink(videoInfo.coverPath).catch(() => { });
    }

    return animeMessages;
  }
  const videoInfo = await extractVideoMetadata(videoPath);
  const animeMessage = await sendMessage(
    client,
    Number(env.data.ADMIN_GROUP_ID),
    {
      text: item.title,
      topic_id: validCacheTopic,
      timeout: 3600,
      media: {
        video: {
          path: videoPath,
        },
        cover: {
          path: videoInfo.coverPath,
        },
        width: videoInfo.width,
        height: videoInfo.height,
        duration: Math.floor(videoInfo.duration),
        supports_streaming: true,
        has_spoiler: anime?.r18 === true || false,
      },
    }
  );
  await updateTorrentStatus(item.title, "等待纠正");
  fs.unlink(videoPath).catch(() => { });
  fs.unlink(videoInfo.coverPath).catch(() => { });
  return animeMessage;
}