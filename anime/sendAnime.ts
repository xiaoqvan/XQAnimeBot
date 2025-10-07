import fs from "fs/promises";

import logger from "@log/index.ts";

import {
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
} from "@TDLib/function/message.ts";
import { getAnimeById } from "../database/query.ts";
import { AnimeText, navmegtext } from "./text.ts";
import { getSubjectById } from "./info.ts";
import { getMessageLink, getMessageLinkInfo } from "@TDLib/function/get.ts";
import { downloadFile, extractVideoMetadata } from "../function/index.ts";
import { env } from "../database/initDb.ts";

import type { animeItem, anime as animeType } from "../types/anime.ts";
import type { Client } from "tdl";
import type { MessageContent } from "tdlib-types";

import { parseTextEntities } from "@TDLib/function/index.ts";

/**
 * 发送/更新 导航频道的消息
 *
 * @param client - TDLib 客户端实例
 * @param id - 数据库中动漫的id字段值
 * @returns 导航消息链接
 */
export async function sendMegToNavAnime(client: Client, id: number) {
  let Anime = await getAnimeById(id);

  if (!Anime) return;

  // 旧的转换为新的过渡
  if (Anime.navMessageLink) {
    const navmeg = await getMessageLinkInfo(client, Anime.navMessageLink);

    if (!navmeg || !navmeg.message) {
      throw new Error(`旧导航频道消息链接无效，链接：${Anime.navMessageLink}`);
    }

    // 导航频道有旧消息，进行新消息适配
    const newMeg = {
      chat_id: navmeg.chat_id,
      message_id: navmeg.message.id,
      thread_id: navmeg.message_thread_id || undefined,
      link: Anime.navMessageLink,
    };
    await updateAnimeNavMessage(Anime.id, newMeg);
    Anime = await getAnimeById(id);
    if (!Anime) return;
  }
  // 导航频道中有该番剧，编辑现有消息
  if (Anime.navMessage?.link) {
    // 更新评分
    const animeInfo = await getSubjectById(Anime.id);

    updateAnimeInfo(Anime.id, animeInfo);

    Anime.score = animeInfo?.rating?.score || Anime.score;
    const megtexts = await navmegtext(client, Anime); // megtexts[0] 为主导航，1.. 为资源

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
          const newText = await parseTextEntities(client, megtexts[idx]);
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
                }
              );
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
            }
          } else {
            // 未知类型，保持兼容使用编辑文本
            await editMessageText(
              client,
              videoMeg.chat_id,
              videoMeg.message_id,
              {
                text: megtexts[idx],
              }
            );
          }
        } catch {
          // 获取旧消息失败则按原逻辑尝试编辑为文本
          await editMessageText(client, videoMeg.chat_id, videoMeg.message_id, {
            text: megtexts[idx],
          });
        }
        idx++;
      }
      // 如果 megtexts 有新增条目，则补发并写入数据库
      for (; idx < megtexts.length; idx++) {
        const videoMeg = await sendMessage(client, Anime.navMessage.chat_id, {
          invoke: {
            reply_to: {
              _: "inputMessageReplyToMessage",
              message_id: Anime.navMessage.message_id,
            },
            message_thread_id: Anime.navMessage.thread_id,
            input_message_content: {
              _: "inputMessageText",
              text: await parseTextEntities(client, megtexts[idx]),
            },
          },
        });

        if (!videoMeg) {
          logger.error(
            "sendMegToNavAnime",
            `补发导航频道消息失败: ${Anime.navMessage.chat_id}, ${Anime.id}`
          );
          continue;
        }

        const navLink = await getMessageLink(
          client,
          videoMeg.chat_id,
          videoMeg.id
        );
        await updateAnimeNavVideoMessage(Anime.id, {
          page: idx, // 与 megtexts 的索引对应：1.. 为资源页
          chat_id: videoMeg.chat_id,
          message_id: videoMeg.id,
          thread_id: Anime.navMessage.thread_id,
          link: navLink.link,
        });
      }
    } else {
      // 没有历史视频消息，全部按顺序发送，并写入数据库
      for (idx = 1; idx < megtexts.length; idx++) {
        const videoMeg = await sendMessage(client, Anime.navMessage.chat_id, {
          invoke: {
            reply_to: {
              _: "inputMessageReplyToMessage",
              message_id: Anime.navMessage.message_id,
            },
            message_thread_id: Anime.navMessage.thread_id,
            input_message_content: {
              _: "inputMessageText",
              text: await parseTextEntities(client, megtexts[idx]),
            },
          },
        });

        if (!videoMeg) {
          logger.error(
            "sendMegToNavAnime",
            `补发导航频道消息失败: ${Anime.navMessage.chat_id}, ${Anime.id}`
          );
          continue;
        }

        const navLink = await getMessageLink(
          client,
          videoMeg.chat_id,
          videoMeg.id
        );
        await updateAnimeNavVideoMessage(Anime.id, {
          page: idx,
          chat_id: videoMeg.chat_id,
          message_id: videoMeg.id,
          thread_id: Anime.navMessage.thread_id,
          link: navLink.link,
        });

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
      logger.error(`本地图片上传也失败: ${Anime.image}`, localError);
      throw localError;
    } finally {
      // 清理本地图片文件
      if (localImagePath) {
        await fs.unlink(localImagePath).catch(() => {});
      }
    }
  }

  if (!navmeg) {
    throw new Error("发送导航消息失败");
  }

  // 获取首条（图片）消息链接并写入 navMessage
  const navLink = await getMessageLink(client, navmeg.chat_id, navmeg.id);
  const navMessage = {
    chat_id: navmeg.chat_id,
    message_id: navmeg.id,
    thread_id: navmeg.message_thread_id,
    link: navLink.link,
  };
  await updateAnimeNavMessage(Anime.id, navMessage);

  // 继续发送后续文本消息，并写入 navVideoMessage
  for (let i = 1; i < megtexts.length; i++) {
    const videoMeg = await sendMessage(client, navmeg.chat_id, {
      text: megtexts[i],
      reply_to_message_id: navmeg.id,
      thread_id: navmeg.message_thread_id,
    });

    if (!videoMeg) {
      logger.error(
        "sendMegToNavAnime",
        `补发导航频道消息失败: ${navmeg.chat_id}, ${Anime.id}, index=${i}`
      );
      continue;
    }

    const link = await getMessageLink(client, videoMeg.chat_id, videoMeg.id);
    await updateAnimeNavVideoMessage(Anime.id, {
      page: i,
      chat_id: videoMeg.chat_id,
      message_id: videoMeg.id,
      thread_id: navmeg.message_thread_id,
      link: link.link,
    });
  }

  return navLink.link;
}

/**
 * 发送动漫视频到动漫频道
 * @param anime - 数据库中动漫详细信息
 * @param item - 动漫在BT站中的信息
 * @param videoPath - 种子完整信息
 * @param newAnime - 是否为待发送的新动漫
 */
export async function sendMegToAnime(
  client: Client,
  anime: animeType,
  item: animeItem,
  videoPath: string,
  newAnime = false
) {
  const text = AnimeText(anime, item);

  await updateTorrentStatus(item.title, "上传中");
  const videoInfo = await extractVideoMetadata(videoPath);

  if (newAnime) {
    const animeMessage = await sendMessage(
      client,
      Number(env.data.ADMIN_GROUP_ID),
      {
        text: AnimeText(anime, item),
        thread_id: Number(env.data.ANIME_GROUP_THREAD_ID),
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

    fs.unlink(videoPath).catch(() => {});
    fs.unlink(videoInfo.coverPath).catch(() => {});
    return animeMessage;
  }
  const animeMessage = await sendMessage(
    client,
    Number(env.data.ANIME_CHANNEL),
    {
      text: text,
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
  await updateTorrentStatus(item.title, "完成");
  fs.unlink(videoPath).catch(() => {});
  fs.unlink(videoInfo.coverPath).catch(() => {});
  return animeMessage;
}
