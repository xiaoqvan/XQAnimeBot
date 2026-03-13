import type { message as messageType } from "tdlib-types";
import type { Client } from "tdl";

import { isUserAdmin } from "@TDLib/function/index.ts";
import { sendMessage } from "@TDLib/function/message.ts";
import { animeProcessor } from "../anime/AnimeProcessorManager.ts";
import { env } from "../database/initDb.ts";
import { getConfig } from "@db/config.ts";

/**
 * 处理 `/canceltask` 命令：取消当前堵塞任务并释放并发槽位
 *
 * 支持参数：
 * - 无参数：取消第 1 个活跃任务（通常是最早卡住的任务）
 * - 数字：按 `/progress` 显示序号取消，例如 `/canceltask 2`
 * - `all`：取消全部活跃任务
 *
 * 权限：仅管理员或 super_admin 可使用
 *
 * @param client - TDLib 客户端实例
 * @param message - 触发命令的 Telegram 消息对象
 * @param commandParts - 命令参数数组
 */
export default async function cancelTask(
    client: Client,
    message: messageType,
    commandParts: string[] | undefined
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

    if (!isAdmin && !isBotAdmin) return;

    const arg = commandParts?.[0]?.trim().toLowerCase();

    if (!arg) {
        const result = animeProcessor.cancelActiveByIndex(1);
        await sendMessage(client, message.chat_id, {
            reply_to_message_id: message.id,
            text: result.ok
                ? `✅ ${result.message}`
                : `❌ ${result.message}\n\n用法: /canceltask [序号|all]`,
            link_preview: false,
        });
        return;
    }

    if (arg === "all") {
        const results = animeProcessor.cancelAllActive();
        const success = results.filter((r) => r.ok);
        const failed = results.filter((r) => !r.ok);

        let text = `**取消活跃任务结果**\n\n`;
        text += `✅ 成功: ${success.length}\n`;
        text += `❌ 失败: ${failed.length}\n\n`;

        if (success.length > 0) {
            text += `成功项:\n${success
                .map((r, i) => `${i + 1}. ${r.title ?? "(未知)"}`)
                .join("\n")}\n\n`;
        }

        if (failed.length > 0) {
            text += `失败原因:\n${failed
                .map((r, i) => `${i + 1}. ${r.message}`)
                .join("\n")}`;
        }

        await sendMessage(client, message.chat_id, {
            reply_to_message_id: message.id,
            text,
            link_preview: false,
        });
        return;
    }

    const index = Number(arg);
    if (!Number.isInteger(index) || index <= 0) {
        await sendMessage(client, message.chat_id, {
            reply_to_message_id: message.id,
            text: "❌ 参数错误，用法: /canceltask [序号|all]\n示例: /canceltask 1",
            link_preview: false,
        });
        return;
    }

    const result = animeProcessor.cancelActiveByIndex(index);
    await sendMessage(client, message.chat_id, {
        reply_to_message_id: message.id,
        text: result.ok ? `✅ ${result.message}` : `❌ ${result.message}`,
        link_preview: false,
    });
}
