import type { message as messageType } from "tdlib-types";
import type { Client } from "tdl";

import { isUserAdmin } from "@TDLib/function/index.ts";
import { sendMessage } from "@TDLib/function/message.ts";
import { sendMegToNavAnime } from "../anime/sendAnime.ts";
import { getAnimeById } from "../database/query.ts";
import { env } from "../database/initDb.ts";
import { getConfig } from "@db/config.ts";

export default async function updateAnime(
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
  if (message.content._ !== "messageText") {
    return;
  }
  if (!commandParts || commandParts.length !== 1) {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: "❌ 用法错误！\n\n**正确用法**:\n`/updateanime <ID>` - 更新指定ID动漫的信息\n\n**示例**:\n`/updateanime 12345`",
      link_preview: true,
    });
    return;
  }

  const animeId = Number(commandParts[0]);
  if (!animeId) {
    sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: "❌ 用法错误！\n\n**正确用法**:\n`/updateanime <ID>` - 更新指定ID动漫的信息\n\n**示例**:\n`/updateanime 12345`",
      link_preview: true,
    });
    return;
  }
  const anime = await getAnimeById(animeId);
  await sendMessage(client, message.chat_id, {
    reply_to_message_id: message.id,
    text: `✅ 已触发更新动漫${anime?.name_cn || anime?.name}(${
      anime?.id
    })信息的操作\nlink:${
      anime?.navMessage?.link
    }\n\n更新后的信息会更新到频道。\n\n如果长时间未更新信息，请检查日志或联系管理员。`,
    link_preview: true,
  });
  await sendMegToNavAnime(client, animeId);
  return;
}
