import { sendMessage } from "@TDLib/function/message.ts";
import type { Client } from "tdl";
import type { message } from "tdlib-types"
import { base62ToHex } from "../utils/base62ToHex.ts";
import { getAccessToken } from "../bangumi/callback.ts";
import { getMe, updateEpisodeCollectionInfo, updateSubjectCollectionInfo } from "../bangumi/set.ts";
import { getAnimeById, getBangumiUserById, getAnimeByEpisodeId } from "../database/query.ts";
import { getBgmToken } from "../utils/getBgmToken.ts";

export default async function start(client: Client, message: message) {
    if (message.content._ !== "messageText") return;
    const text = (message.content.text?.text || "").trim();
    // 获取发送者用户 ID
    const user_id = message.sender_id._ === "messageSenderUser" ? message.sender_id.user_id : null;
    if (!user_id) return;
    const m = text.match(/^\/start(?:@[\S]+)?\s*(.*)$/i);
    if (!m) return;
    const payload = (m[1] || "").trim();
    // 纯 /start 或无 payload 时不回复
    if (!payload) return;

    const index = payload.indexOf("-");
    let action = payload;
    let values: string[] = [];

    if (index !== -1) {
        action = payload.substring(0, index);
        const params = payload.substring(index + 1);
        if (params) values = params.split("_");
    }
    switch (action) {
        case "bgm_cb":
            try {
                const short = values[0] ?? "";
                const state = values[1] ?? "";
                const code = base62ToHex(short);
                const bgm = await getBangumiUserById(Number(state));
                if (!bgm) {
                    sendMessage(client, message.chat_id, {
                        reply_to_message_id: message.id,
                        text: `授权失败，找不到对应的用户记录！请重新使用 /bindbangumi 进行绑定。`,
                    });
                    break;
                }
                if (user_id !== bgm.tgUserId) {
                    sendMessage(client, message.chat_id, {
                        reply_to_message_id: message.id,
                        text: `授权失败，当前 Telegram 账户与绑定的账户不匹配！请使用正确的账户进行授权。`,
                    });
                    break;
                }
                const access = await getAccessToken(code, state);
                const me = await getMe(access.access_token);
                sendMessage(client, message.chat_id, {
                    reply_to_message_id: message.id,
                    text: `账户授权成功！\n账户名：${me.nickname}\n用户ID：${me.id}\n你可以在 https://bgm.tv/ 中管理你的进度\n\n使用 /exitbangumi 可以退出登录。\n\n如果需要更改绑定的账户,使用 /exitbangumi 退出后重新授权流程即可`,
                });
            } catch (error) {
                sendMessage(client, message.chat_id, {
                    reply_to_message_id: message.id,
                    text: `授权失败，发生错误：${(error as Error).message}`,
                });
            }
            break;
        case "collection":
            const animeId = Number(values[0] ?? "");
            const tokenResult = await getBgmToken(user_id);
            if (!tokenResult.success || !tokenResult.access_token) {
                sendMessage(client, message.chat_id, {
                    text: tokenResult.message || "获取Bangumi访问令牌失败，请先绑定Bangumi账户。",
                });
                break;
            }
            const anime = await getAnimeById(animeId);

            if (!anime || !tokenResult.access_token) {
                sendMessage(client, message.chat_id, {
                    reply_to_message_id: message.id,
                    text: `收藏失败，ID：${animeId}\n当前频道未添加该动漫信息，请确认后再试！`,
                });
                break;
            }
            await updateSubjectCollectionInfo(tokenResult.access_token, animeId, 3);
            sendMessage(client, message.chat_id, {
                reply_to_message_id: message.id,
                text: `收藏动漫: [${anime?.name_cn || anime?.name}](https://bgm.tv/subject/${animeId}) 成功！\n当前状态: **在看**\n\n点击下方按钮更改收藏状态：`,
                invoke: {
                    reply_markup: {
                        _: "replyMarkupInlineKeyboard",
                        rows: [
                            [
                                {
                                    _: "inlineKeyboardButton",
                                    text: "想看",
                                    type: {
                                        _: "inlineKeyboardButtonTypeCallback",
                                        data: Buffer.from(
                                            `chgcol?id=${animeId}&status=1`
                                        ).toString("base64"),
                                    },
                                },
                                {
                                    _: "inlineKeyboardButton",
                                    text: "看过",
                                    type: {
                                        _: "inlineKeyboardButtonTypeCallback",
                                        data: Buffer.from(
                                            `chgcol?id=${animeId}&status=2`
                                        ).toString("base64"),
                                    },
                                },
                            ],
                            [
                                {
                                    _: "inlineKeyboardButton",
                                    text: "搁置",
                                    type: {
                                        _: "inlineKeyboardButtonTypeCallback",
                                        data: Buffer.from(
                                            `chgcol?id=${animeId}&status=4`
                                        ).toString("base64"),
                                    },
                                },
                                {
                                    _: "inlineKeyboardButton",
                                    text: "抛弃",
                                    type: {
                                        _: "inlineKeyboardButtonTypeCallback",
                                        data: Buffer.from(
                                            `chgcol?id=${animeId}&status=5`
                                        ).toString("base64"),
                                    },
                                },
                            ],
                        ],
                    },
                },
            });
            break;
        case "eplook":
            const episodeId = values[0] ?? "";
            const epResult = await getBgmToken(user_id);
            if (!epResult.success || !epResult.access_token) {
                sendMessage(client, message.chat_id, {
                    text: epResult.message || "获取Bangumi访问令牌失败，请先绑定Bangumi账户。",
                });
                break;
            }
            const animeFromEpisode = await getAnimeByEpisodeId(Number(episodeId));
            if (!animeFromEpisode) {
                sendMessage(client, message.chat_id, {
                    reply_to_message_id: message.id,
                    text: `标记失败，找不到对应的动漫信息，剧集ID：${episodeId}`,
                });
                break;
            }

            const epUpdated = await updateEpisodeCollectionInfo(epResult.access_token, Number(episodeId), 2);
            if (!epUpdated) {
                sendMessage(client, message.chat_id, {
                    reply_to_message_id: message.id,
                    text: `标记失败：当前用户未收藏该条目 [${animeFromEpisode.name_cn || animeFromEpisode.name}](https://bgm.tv/subject/${animeFromEpisode.id})\n请先收藏该条目后再标记看过！\n\n下方按钮可以快捷操作：\n1-收藏该动漫为 *\_在看\_* ,且将本集标记为 *\_看过\_* \n2- 收藏该动漫和本集为 *\_看过\_* `,
                    invoke: {
                        reply_markup: {
                            _: "replyMarkupInlineKeyboard",
                            rows: [
                                [
                                    {
                                        _: "inlineKeyboardButton",
                                        text: "1",
                                        type: {
                                            _: "inlineKeyboardButtonTypeCallback",
                                            data: Buffer.from(
                                                `colorep?id=${animeFromEpisode.id}&at=3&ep=${episodeId}&et=2`
                                            ).toString("base64"),
                                        },
                                    },
                                    {
                                        _: "inlineKeyboardButton",
                                        text: "2",
                                        type: {
                                            _: "inlineKeyboardButtonTypeCallback",
                                            data: Buffer.from(
                                                `colorep?id=${animeFromEpisode.id}&at=2&ep=${episodeId}&et=2`
                                            ).toString("base64"),
                                        },
                                    },
                                ],
                            ],
                        },
                    },
                });
                break;
            }
            // 查找该章节的 sort（作为集数显示）
            const epEntry = animeFromEpisode.eps?.list?.find(e => e.id === Number(episodeId));
            const epSort = epEntry?.sort ?? episodeId;

            sendMessage(client, message.chat_id, {
                reply_to_message_id: message.id,
                text: `动漫: [${animeFromEpisode.name_cn || animeFromEpisode.name}](https://bgm.tv/subject/${animeFromEpisode.id})\n集数: [${epSort}](https://bgm.tv/ep/${episodeId}) 标记为 看过 成功！`,
            });
            break;
        default:
            break;
    }

} 
