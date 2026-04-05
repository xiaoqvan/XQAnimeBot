import { sendMessage } from "@TDLib/function/message.ts";
import { env } from "../database/initDb.ts";
import { getEpisodeMetasBySubjectId } from "../database/query.ts";
import type { EpisodeMatchResult } from "../utils/matcher.ts";
import type { anime as animeType } from "../types/anime.d.ts";
import type { animeItem } from "../types/rss.d.ts";
import type { Client } from "tdl";

/**
 * 通知管理员：自动搜索番剧信息失败，需要人工提供
 *
 * 发送一条带内联按钮的消息到管理员群，点击按钮可触发
 * `N_anime?c=<cacheId>` 回调，进入手动绑定流程。
 *
 * @param client - TDLib 客户端实例
 * @param cacheId - cacheItem 集合中已保存的缓存 ID
 * @param item - 待处理的动漫 BT 条目
 */
export async function promptAdminProvideAnimeInfo(
    client: Client,
    cacheId: number,
    item: animeItem
): Promise<void> {
    await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
        topic_id: {
            _: "messageTopicForum",
            forum_topic_id: Number(env.data.NAV_GROUP_THREAD_ID),
        },
        text: `番剧: ${item.title}\n集数: ${item.episode || "未获取到"}\n\n未搜索到的动漫信息\n请手动提供动漫信息`,
        link_preview: true,
        invoke: {
            reply_markup: {
                _: "replyMarkupInlineKeyboard",
                rows: [
                    [
                        {
                            _: "inlineKeyboardButton",
                            text: "点击提供",
                            type: {
                                _: "inlineKeyboardButtonTypeCallback",
                                data: Buffer.from(`N_anime?c=${cacheId}`).toString("base64"),
                            },
                        },
                    ],
                ],
            },
        },
    });
}

/**
 * 通知管理员：确认自动搜索匹配到的番剧信息是否正确
 *
 * 提供"正确"和"错误"两个按钮，对应 `Y_anime` 和 `F_anime` 回调。
 *
 * @param client - TDLib 客户端实例
 * @param anime - 自动搜索并保存到数据库的番剧文档
 * @param episodeId - 匹配到的 bangumi 集数 ID
 * @param cacheId - cacheItem 集合中已保存的缓存 ID
 * @param item - 待处理的动漫 BT 条目
 */
export async function promptAdminConfirmAnime(
    client: Client,
    anime: animeType,
    episodeId: number,
    cacheId: number,
    item: animeItem
): Promise<void> {
    // 根据 episodeId 从 episodes_meta 找到对应剧集的显示集数（sort 字段）
    let epSort: number = episodeId;
    try {
        const episodes = await getEpisodeMetasBySubjectId(anime.id);
        const epEntry = episodes.find((e) => e.id === Number(episodeId));
        epSort = epEntry?.sort ?? episodeId;
    } catch {
        epSort = episodeId;
    }

    await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
        topic_id: {
            _: "messageTopicForum",
            forum_topic_id: Number(env.data.NAV_GROUP_THREAD_ID),
        },
        text:
            `当前番剧为${item.title}\n\n搜索到的动漫信息：\n\n` +
            `**名称：** [${anime.name_cn || anime.name}](https://bgm.tv/subject/${anime.id})\n` +
            `**ID：** ${anime.id}\n` +
            `剧集: [${epSort}](https://bgm.tv/ep/${episodeId})\n\n请确认是否正确`,
        link_preview: true,
        invoke: {
            reply_markup: {
                _: "replyMarkupInlineKeyboard",
                rows: [
                    [
                        {
                            _: "inlineKeyboardButton",
                            text: "正确",
                            type: {
                                _: "inlineKeyboardButtonTypeCallback",
                                data: Buffer.from(
                                    `Y_anime?id=${episodeId}&c=${cacheId}`
                                ).toString("base64"),
                            },
                        },
                        {
                            _: "inlineKeyboardButton",
                            text: "错误",
                            type: {
                                _: "inlineKeyboardButtonTypeCallback",
                                data: Buffer.from(
                                    `F_anime?id=${episodeId}&c=${cacheId}`
                                ).toString("base64"),
                            },
                        },
                    ],
                ],
            },
        },
    });
}

/**
 * 通知管理员：集数自动匹配出现问题（越界、放送日期不符等），需人工干预
 *
 * - `DATE_MISMATCH`：日期不在合理区间，提供"提供正确"按钮（直接确认当前 episodeId）
 * - 其他失败状态：集数无法解析或越界，引导管理员手动输入集数 ID
 *
 * @param client - TDLib 客户端实例
 * @param anime - 数据库中匹配到的番剧文档
 * @param cacheId - cacheItem 集合中已保存的缓存 ID
 * @param item - 待处理的动漫 BT 条目
 * @param matchResult - 集数匹配失败的详细结果对象
 */
export async function promptAdminConfirmAnimeEpisodes(
    client: Client,
    anime: animeType,
    cacheId: number,
    item: animeItem,
    matchResult: EpisodeMatchResult
): Promise<void> {
    if (matchResult.status === "MATCHED") return;

    const baseText =
        `当前番剧为${item.title}\n\n动漫信息：\n\n` +
        `**名称：** [${anime.name_cn || anime.name}](https://bgm.tv/subject/${anime.id})\n` +
        `**ID：** ${anime.id}\n` +
        `匹配集数: ${item.episode}\n` +
        `出现问题：${matchResult.msg}\n\n`;

    if (matchResult.status === "DATE_MISMATCH") {
        await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
            topic_id: {
                _: "messageTopicForum",
                forum_topic_id: Number(env.data.NAV_GROUP_THREAD_ID),
            },
            text: baseText + "请确认是否正确",
            link_preview: true,
            invoke: {
                reply_markup: {
                    _: "replyMarkupInlineKeyboard",
                    rows: [
                        [
                            {
                                _: "inlineKeyboardButton",
                                text: "提供正确",
                                type: {
                                    _: "inlineKeyboardButtonTypeCallback",
                                    data: Buffer.from(
                                        `Y_anime?id=${matchResult.episodeId}&c=${cacheId}`
                                    ).toString("base64"),
                                },
                            },
                        ],
                    ],
                },
            },
        });
        return;
    }

    // NOT_FOUND_IN_DB / INVALID_INPUT：需要管理员手动提供集数 ID
    await sendMessage(client, Number(env.data.ADMIN_GROUP_ID), {
        topic_id: {
            _: "messageTopicForum",
            forum_topic_id: Number(env.data.NAV_GROUP_THREAD_ID),
        },
        text: baseText + "请提供正确的集数id",
        link_preview: true,
        invoke: {
            reply_markup: {
                _: "replyMarkupInlineKeyboard",
                rows: [
                    [
                        {
                            _: "inlineKeyboardButton",
                            text: "提供正确",
                            type: {
                                _: "inlineKeyboardButtonTypeCallback",
                                data: Buffer.from(
                                    `N_ep?c=${cacheId}&id=${anime.id}`
                                ).toString("base64"),
                            },
                        },
                    ],
                ],
            },
        },
    });
}
