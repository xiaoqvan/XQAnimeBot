import { sendMessage } from "@TDLib/function/message.ts";
import type { Client } from "tdl";
import type { message } from "tdlib-types"
import { getAuthUrl } from "../bangumi/callback.ts";


export default async function exitbangumi(client: Client, message: message) {
    if (message.content._ !== "messageText") return;
    // 获取发送者用户 ID
    const user_id = message.sender_id._ === "messageSenderUser" ? message.sender_id.user_id : null;
    if (!user_id) return;
    const authUrl = await getAuthUrl(user_id);
    sendMessage(client, message.chat_id, {
        reply_to_message_id: message.id,
        text: `请点击以下链接进行Bangumi账户绑定：\n${authUrl}\n如果没有跳转授权页面，请退出Bangumi账户后再次点击链接登录账户。`,
    });
} 