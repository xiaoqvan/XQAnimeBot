import { sendMessage } from "@TDLib/function/message.ts";
import type { Client } from "tdl";
import { env } from "../database/initDb.ts";
/**
 * 错误处理函数
 * @param error
 */
export async function ErrorHandler(client: Client, error: unknown) {
  let errorText;
  if (error instanceof Error) {
    errorText = `name: ${error.name}\nmessage: ${error.message}\nstack: ${error.stack}`;
  } else {
    errorText = JSON.stringify(error, null, 2);
  }

  await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
    topic_id: {
      forum_topic_id: Number(env.data.ERROR_GROUP_THREAD_ID),
    },
    text: `错误信息:\n${errorText}`,
    link_preview: true,
  });
}
