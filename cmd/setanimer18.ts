import type { message as messageType } from "tdlib-types";
import type { Client } from "tdl";

import logger from "@log/index.ts";
import {
  editMessageMedia,
  editMessageText,
  sendMessage,
} from "@TDLib/function/message.ts";
import { isUserAdmin, parseTextEntities } from "@TDLib/function/index.ts";
import { env } from "../database/initDb.ts";

import { getAnimeById } from "../database/query.ts";
import { updateAnimeR18 } from "../database/update.ts";
import type { animeItem, anime as animeType } from "../types/anime.d.ts";
import { getMessageLinkInfo } from "@TDLib/function/get.ts";
import { AnimeText } from "../anime/text.ts";
import { getConfig } from "@db/config.ts";
/**
 * 处理 /addanime 命令
 * @param {object} message - 消息对象
 * @param {string[]} commandParts - 命令参数数组
 */
export default async function handleSetAnimeR18(
  client: Client,
  message: messageType,
  commandParts: string[] | undefined
) {
  // 检查是否为管理员
  const isAdmin = await isUserAdmin(
    client,
    Number(env.data.ADMIN_GROUP_ID),
    message.sender_id
  );
  const config = await getConfig("admin");
  const isBotAdmin =
    message.sender_id._ === "messageSenderUser" &&
    message.sender_id.user_id === config?.super_admin;

  if (!isAdmin && !isBotAdmin) {
    return;
  }
  // 参数校验
  if (!commandParts || commandParts.length !== 2) {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: "❌ 用法错误！\n\n**正确用法**:\n`/setanimer18 <ID> <true|false>` - 设置指定ID动漫的 r18 字段\n\n**示例**:\n`/setanimer18 12345 true`",
      link_preview: true,
    });
    return;
  }

  const animeId = Number(commandParts[0]);
  if (Number.isNaN(animeId)) {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: "❌ 无效的动漫ID，请传入数字ID，例如：`/setanimer18 12345 true`",
      link_preview: true,
    });
    return;
  }

  const rawR18 = (commandParts[1] ?? "").trim().toLowerCase();
  if (rawR18 !== "true" && rawR18 !== "false") {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: "❌ r18 参数无效，只接受 `true` 或 `false`（不区分大小写）",
      link_preview: true,
    });
    return;
  }
  const r18Value = rawR18 === "true";

  // 查询动漫
  const anime = await getAnimeById(animeId);
  if (!anime) {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: `❌ 未找到ID为 ${animeId} 的动漫信息`,
      link_preview: true,
    });
    return;
  }

  const tipsmeg = await sendMessage(client, message.chat_id, {
    reply_to_message_id: message.id,
    text: `🔄 正在更新ID为 ${animeId} 的动漫 r18 字段为 ${
      r18Value ? "true" : "false"
    }...`,
    link_preview: true,
  });
  if (!tipsmeg) {
    logger.error("发送提示消息失败");
    throw new Error("发送提示消息失败");
  }
  // 更新 r18 字段
  const result = await updateAnimeR18(animeId, r18Value);
  if (!result) {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: `❌ 更新ID为 ${animeId} 的动漫 r18 字段失败，可能值未改变`,
      link_preview: true,
    });
    return;
  }

  // 取回更新后的 anime 并更新 btdata 相关消息内容
  const updatedAnime = await getAnimeById(animeId);
  if (!updatedAnime) {
    logger.error("获取更新后的动漫信息失败");
    throw new Error("获取更新后的动漫信息失败");
  }
  await updateAssociatedVideoMessagesR18(
    client,
    updatedAnime,
    tipsmeg,
    r18Value
  );

  await editMessageText(client, tipsmeg.chat_id, tipsmeg.id, {
    text: `✅ 已成功设置 ID 为 ${animeId} 的动漫 r18 字段为 ${
      r18Value ? "true" : "false"
    }`,
  });
}

/**
 * 更新所有关联的视频消息的 r18 字段
 *
 * @param client - TDLib 客户端实例
 * @param anime - 更新后的动漫数据
 * @param message - 消息对象
 * @param r18Value - r18 新值
 */
async function updateAssociatedVideoMessagesR18(
  client: Client,
  anime: animeType,
  message: messageType,
  r18Value: boolean
) {
  if (!anime.btdata || typeof anime.btdata !== "object") return;
  // 统计总的视频消息数量
  let totalMessages = 0;
  for (const episodes of Object.values(anime.btdata)) {
    if (!Array.isArray(episodes)) continue;
    for (const ep of episodes) {
      if (ep.TGMegLink) totalMessages++;
    }
  }
  let processed = 0,
    success = 0,
    fail = 0,
    failDetails = [];
  // 进度提示消息
  await editMessageText(client, message.chat_id, message.id, {
    text: `🔄 正在更新视频消息 r18 字段...\n进度: ${processed}/${totalMessages}\n成功: ${success}\n失败: ${fail}`,
    link_preview: true,
  });
  for (const [fansub, episodes] of Object.entries(anime.btdata)) {
    if (!Array.isArray(episodes)) continue;
    for (const ep of episodes) {
      if (!ep.TGMegLink) continue;
      processed++;
      // 获取消息信息
      let messageLinkInfo;
      try {
        messageLinkInfo = await getMessageLinkInfo(client, ep.TGMegLink);
      } catch {
        fail++;
        failDetails.push(`[${fansub}] 第${ep.episode}集: 链接无效`);
        continue;
      }

      if (
        !messageLinkInfo ||
        !messageLinkInfo.message ||
        messageLinkInfo.message?.content._ !== "messageVideo"
      ) {
        fail++;
        failDetails.push(`[${fansub}] 第${ep.episode}集: 获取消息信息失败`);
        continue;
      }
      // 从标题中提取字幕组名称
      let fansubs: string[] = [];
      const match = ep.title.match(/^(?:\[([^\]]+)\]|【([^】]+)】)/);
      if (!match) {
        continue; // 跳过无法解析的条目
      }
      if (match) {
        const raw = match[1] || match[2];
        fansubs = raw
          .split(/\s*[&/|｜、]\s*/)
          .map((s) => s.trim())
          .filter(Boolean);
      }

      // 构建 item 对象，模拟 RSS 项格式
      const item: animeItem = {
        title: ep.title,
        fansub: fansubs,
        names: anime.names || [],
        pubDate: "",
        magnet: "",
        team: "",
      };
      const animeText = AnimeText(anime, item);
      // 构造新文本（AnimeText 需包含 r18 字段变化）
      const animeTdText = await parseTextEntities(client, animeText);
      // 检查是否需要更新
      if (
        messageLinkInfo.message?.content._ === "messageVideo" &&
        animeTdText.text &&
        messageLinkInfo.message?.content?.caption?.text.trim() ===
          animeTdText.text.trim()
      ) {
        success++;
        continue;
      }
      try {
        await editMessageMedia(
          client,
          messageLinkInfo.message.chat_id,
          messageLinkInfo.message.id,
          {
            text: animeText,
            media: {
              video: {
                id: ep.videoid,
              },
              cover: {
                id: messageLinkInfo.message.content.cover?.sizes[0].photo.remote
                  .unique_id,
              },
              width: messageLinkInfo.message.content.video.width,
              height: messageLinkInfo.message.content.video.height,
              duration: messageLinkInfo.message.content.video.duration,
              has_spoiler: r18Value === true || anime?.r18 === true || false,
              supports_streaming: true,
            },
          }
        );
        success++;
      } catch (error) {
        fail++;
        failDetails.push(
          `[${fansub}] 第${ep.episode}集: 编辑消息失败 ${
            (error as Error).message
          }`
        );
      }
      // 更新进度消息
      await editMessageText(client, message.chat_id, message.id, {
        text: `🔄 正在更新视频消息 r18 字段...\n进度: ${processed}/${totalMessages}\n成功: ${success}\n失败: ${fail}`,
      });
      await new Promise((res) => setTimeout(res, 5000));
    }
  }
  await editMessageText(client, message.chat_id, message.id, {
    text: `✅ NSFW 字段更新完成！\n总数: ${totalMessages}\n成功: ${success}\n失败: ${fail}${
      failDetails.length ? "\n失败详情: " + failDetails.join("; ") : ""
    }`,
  });
}
