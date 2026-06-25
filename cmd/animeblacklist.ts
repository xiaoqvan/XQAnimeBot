import logger from "@log/index.ts";
import { sendMessage } from "@TDLib/function/message.ts";
import { getDatabase } from "@db/index.ts";
import type { Client } from "tdl";
import type { updateNewMessage } from "tdlib-types";

const db = await getDatabase();

/**
 * 黑名单关键词管理命令
 *
 * 用法：
 *   /animeblacklist list              - 列出所有黑名单关键词
 *   /animeblacklist add <关键词>       - 添加黑名单关键词
 *   /animeblacklist remove <关键词>    - 移除黑名单关键词
 *   /animeblacklist clear             - 清空所有黑名单关键词
 *
 * 当 RSS 标题中包含黑名单关键词时，该条目会被跳过不处理。
 */
export default async function animeBlacklistCmd(
    client: Client,
    message: updateNewMessage["message"],
    args: string[] = []
) {
    if (!args || args.length === 0) {
        await sendMessage(client, message.chat_id, {
            text:
                `📋 **黑名单关键词管理**\n\n` +
                `用法：\n` +
                `  /animeblacklist list              - 列出所有黑名单关键词\n` +
                `  /animeblacklist add <关键词>       - 添加黑名单关键词\n` +
                `  /animeblacklist remove <关键词>    - 移除黑名单关键词\n` +
                `  /animeblacklist clear             - 清空所有黑名单关键词\n\n` +
                `当 RSS 标题中包含黑名单关键词时，该条目会被跳过不处理。`,
        });
        return;
    }

    const cmd = args[0]!.toLowerCase();

    switch (cmd) {
        case "list": {
            const config = await db
                .collection<{ key: string; list: string[] }>("config")
                .findOne({ key: "animeBlacklist" });

            const list = config?.list ?? [];

            if (list.length === 0) {
                await sendMessage(client, message.chat_id, {
                    text: "📭 当前没有任何黑名单关键词。",
                });
                return;
            }

            const text = list
                .map((keyword, index) => `${index + 1}. \`${keyword}\``)
                .join("\n");

            await sendMessage(client, message.chat_id, {
                text: `📋 **黑名单关键词列表（共 ${list.length} 个）**\n\n${text}`,
            });
            break;
        }

        case "add": {
            if (!args[1]) {
                await sendMessage(client, message.chat_id, {
                    text: "❌ 请提供要添加的关键词：/animeblacklist add <关键词>",
                });
                return;
            }

            const keyword = args.slice(1).join(" ").trim();
            if (!keyword) {
                await sendMessage(client, message.chat_id, {
                    text: "❌ 关键词不能为空",
                });
                return;
            }

            const result = await db
                .collection("config")
                .findOneAndUpdate(
                    { key: "animeBlacklist" },
                    {
                        $addToSet: { list: keyword },
                        $setOnInsert: { key: "animeBlacklist" },
                    },
                    { upsert: true, returnDocument: "after" }
                );

            const currentList = result?.list ?? [];
            await sendMessage(client, message.chat_id, {
                text:
                    `✅ 已添加黑名单关键词：\`${keyword}\`\n` +
                    `当前共 ${currentList.length} 个关键词`,
            });

            logger.info(`[AnimeBlacklist] 已添加关键词: ${keyword}`);
            break;
        }

        case "remove":
        case "delete": {
            if (!args[1]) {
                await sendMessage(client, message.chat_id, {
                    text: "❌ 请提供要移除的关键词：/animeblacklist remove <关键词>",
                });
                return;
            }

            const keyword = args.slice(1).join(" ").trim();
            if (!keyword) {
                await sendMessage(client, message.chat_id, {
                    text: "❌ 关键词不能为空",
                });
                return;
            }

            const result = await db
                .collection<{ key: string; list: string[] }>("config")
                .findOneAndUpdate(
                    { key: "animeBlacklist" },
                    { $pull: { list: { $eq: keyword } } },
                    { returnDocument: "after" }
                );

            if (!result) {
                await sendMessage(client, message.chat_id, {
                    text: "⚠️ 黑名单配置不存在，无需移除。",
                });
                return;
            }

            const currentList = result.list ?? [];
            await sendMessage(client, message.chat_id, {
                text:
                    `✅ 已移除黑名单关键词：\`${keyword}\`\n` +
                    `当前共 ${currentList.length} 个关键词`,
            });

            logger.info(`[AnimeBlacklist] 已移除关键词: ${keyword}`);
            break;
        }

        case "clear": {
            await db
                .collection("config")
                .findOneAndUpdate(
                    { key: "animeBlacklist" },
                    { $set: { list: [] } }
                );

            await sendMessage(client, message.chat_id, {
                text: "✅ 已清空所有黑名单关键词。",
            });

            logger.info("[AnimeBlacklist] 已清空所有关键词");
            break;
        }

        default: {
            await sendMessage(client, message.chat_id, {
                text: `❌ 未知子命令：\`${cmd}\`\n可用命令：list, add, remove, clear`,
            });
            break;
        }
    }
}
