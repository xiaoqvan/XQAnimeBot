import { sendMessage } from "@TDLib/function/message.ts";
import type { Client } from "tdl";
import type { message } from "tdlib-types";
import { getDatabase } from "@db/index.ts";

export default async function exitbangumi(client: Client, message: message) {
    if (message.content._ !== "messageText") return;
    // 获取发送者用户 ID
    const user_id = message.sender_id._ === "messageSenderUser" ? message.sender_id.user_id : null;
    if (!user_id) return;

    try {
        const db = await getDatabase();
        const res = await db.collection("bangumi_users").deleteOne({ tgUserId: user_id });

        if (res.deletedCount && res.deletedCount > 0) {
            sendMessage(client, message.chat_id, {
                reply_to_message_id: message.id,
                text: `已取消删除你的 Bangumi 账户信息（tgUserId: ${user_id}）。`,
            });
        } else {
            sendMessage(client, message.chat_id, {
                reply_to_message_id: message.id,
                text: `未发现绑定的 Bangumi 账户（tgUserId: ${user_id}）。`,
            });
        }
    } catch (err) {
        sendMessage(client, message.chat_id, {
            reply_to_message_id: message.id,
            text: `删除 Bangumi 账户时出错：${err instanceof Error ? err.message : String(err)}`,
        });
    }
}