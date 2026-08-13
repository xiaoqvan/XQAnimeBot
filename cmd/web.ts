import type { message as messageType } from "tdlib-types";
import type { Client } from "tdl";
import { isUserAdmin } from "@TDLib/function/index.ts";
import { sendMessage } from "@TDLib/function/message.ts";
import { getConfig } from "@db/config.ts";
import { env } from "../database/initDb.ts";
import { issueKey } from "../web/key.ts";
import { getApiBaseUrl } from "../web/config.ts";

/**
 * /web 命令：主人/管理员获取 Web 管理连接信息（可复制）。
 *
 * 生成一个 24 小时有效的动态密钥，并发送可复制的后端 API 地址与密钥文本；
 * 前端为独立工程（本地 Vite 运行/部署），用户在管理界面填入地址+密钥即可连接。
 */
export default async function webCmd(
    client: Client,
    message: messageType
): Promise<void> {
    const config = await getConfig("admin");
    const isAdmin = await isUserAdmin(
        client,
        Number(env.data.ADMIN_GROUP_ID),
        message.sender_id
    );
    const isBotAdmin =
        message.sender_id._ === "messageSenderUser" &&
        message.sender_id.user_id === config?.super_admin;

    if (!isAdmin && !isBotAdmin) {
        await sendMessage(client, message.chat_id, {
            reply_to_message_id: message.id,
            text: "❌ 你没有权限获取 Web 连接密钥（仅主人/管理员可用）",
        });
        return;
    }

    // 生成 24h 有效密钥
    const { key, expiresAt } = issueKey("web");
    const api = getApiBaseUrl();

    // 发送可复制的连接信息（不托管前端，前端独立运行）
    await sendMessage(client, message.chat_id, {
        reply_to_message_id: message.id,
        text:
            `🔑 *Web 管理连接*\n\n` +
            `**后端 API 地址**（可复制）：\n` +
            `\`${api}\`\n\n` +
            `**访问密钥**（可复制，24 小时有效）：\n` +
            `\`${key}\`\n\n` +
            `在打开的管理界面中填入以上后端地址与密钥即可连接。\n` +
            `密钥有效期至：${new Date(expiresAt).toLocaleString("zh-CN", { hour12: false })}\n\n` +
            `⚠️ 密钥 24 小时有效，过期后请重新执行 /web 获取。`,
        link_preview: true,
    });
}
