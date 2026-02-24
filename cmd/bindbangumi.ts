import { sendMessage } from "@TDLib/function/message.ts";
import type { Client } from "tdl";
import type { message } from "tdlib-types"
import { getAuthUrl } from "../bangumi/callback.ts";


export default async function bindbangumi(client: Client, message: message) {
    if (message.content._ !== "messageText") return;
    // 获取发送者用户 ID
    const user_id = message.sender_id._ === "messageSenderUser" ? message.sender_id.user_id : null;
    if (!user_id) return;
    const authUrl = await getAuthUrl(user_id);
    sendMessage(client, message.chat_id, {
        reply_to_message_id: message.id,
        text: `请点击以下方按钮进行![☺️](tg://emoji?id=6057366942199586325)Bangumi账户授权\n\n注:\n如果你是新用户第一次使用Bangumi,你需要知道\n> Bangumi 用于管理 ACG 收藏与收视进度\n>Bangumi 不提供 资源下载与观看\n我们使用Bangumi的API接口进行 ACG 收藏与收视进度与管理`,
        invoke: {
            reply_markup: {
                _: "replyMarkupInlineKeyboard",
                rows: [
                    [
                        {
                            _: "inlineKeyboardButton",
                            text: "前往 Bangumi 授权页面",
                            icon_custom_emoji_id: "6057366942199586325",
                            type: {
                                _: "inlineKeyboardButtonTypeUrl",
                                url: authUrl,
                            },
                        },
                    ],
                ],
            },
        },
    });
} 