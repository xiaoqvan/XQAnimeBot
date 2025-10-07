import { getAnimeById, getCacheItemById } from "../database/query.ts";
import { sendMessage } from "@TDLib/function/message.ts";
import { isUserAdmin, parseTextEntities } from "@TDLib/function/index.ts";
import { env } from "../database/initDb.ts";

import type { message as messageType } from "tdlib-types";
import type { Client } from "tdl";
import { getConfig } from "@db/config.ts";

export default async function ConAnimeInformation(
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
  if (
    message.content._ !== "messageText" ||
    message.sender_id._ !== "messageSenderUser"
  )
    return;
  if (!commandParts || commandParts.length !== 2) {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: "❌ 用法错误！\n\n**正确用法**:\n`/jz <动漫ID> <缓存ID>` - 连接番剧与缓存\n\n**示例**:\n`/jz 12345 67890`",
      link_preview: true,
    });
    return;
  }
  const id = Number(commandParts[0]);
  const Cache_id = Number(commandParts[1]);

  const anime = await getAnimeById(id, true);
  const cache = await getCacheItemById(Cache_id);

  await sendMessage(client, message.chat_id, {
    invoke: {
      reply_markup: {
        _: "replyMarkupInlineKeyboard",
        rows: [
          [
            {
              _: "inlineKeyboardButton",
              text: "确定",
              type: {
                _: "inlineKeyboardButtonTypeCallback",
                data: Buffer.from(`Y_anime?id=${id}&c=${Cache_id}`).toString(
                  "base64"
                ),
              },
            },
          ],
        ],
      },
      input_message_content: {
        _: "inputMessageText",
        text: await parseTextEntities(
          client,
          `番剧缓存信息为：${anime?.name_cn || anime?.name || "未知"}(id:${
            anime?.id
          })\n缓存: ${cache?.title}(id:${Cache_id}) `
        ),
      },
    },
  });
}
