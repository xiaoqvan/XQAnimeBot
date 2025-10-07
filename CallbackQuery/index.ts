import { ErrorHandler } from "../function/index.ts";
import logger from "@log/index.ts";
import { falseAnime, nullAnime, trueAnime } from "./hasAnime.ts";
import type { updateNewCallbackQuery as Td$updateNewCallbackQuery } from "tdlib-types";
import type { Client } from "tdl";

/**
 * 处理新的回调查询
 * @param client - TDLib客户端
 * @param update - TDLib更新对象
 * @description 处理新的回调查询
 */
export default async function updateNewCallbackQuery(
  client: Client,
  update: Td$updateNewCallbackQuery
) {
  // 确保更新类型正确
  if (update.payload._ !== "callbackQueryPayloadData") {
    return;
  }
  // console.log("收到新的回调查询", JSON.stringify(update, null, 2));
  const chat_id = update.chat_id;
  const sender_user_id = update.sender_user_id;
  const message_id = update.message_id;
  const data = update.payload.data;
  const queryId = update.id;
  const raw = Buffer.from(data, "base64").toString("utf-8");
  logger.debug(
    "处理回调查询:",
    raw,
    "来自用户ID:",
    sender_user_id,
    "消息ID:",
    message_id
  );

  // 检查 raw 的格式并根据 ? 前的命令进行 switch 处理
  const [command, params] = raw.split("?");
  try {
    switch (command) {
      case "Y_anime": {
        await trueAnime(
          client,
          chat_id,
          sender_user_id,
          message_id,
          queryId,
          raw
        );
        break;
      }
      case "F_anime": {
        await falseAnime(
          client,
          chat_id,
          sender_user_id,
          message_id,
          queryId,
          raw
        );
        break;
      }
      case "N_anime": {
        await nullAnime(
          client,
          chat_id,
          sender_user_id,
          message_id,
          queryId,
          raw
        );
        break;
      }
      default: {
        logger.info("收到未知回调按钮指令:", command, params);
        break;
      }
    }
  } catch (err) {
    logger.error("处理回调查询时发生错误:", err);
    ErrorHandler(client, err);
  }
}
