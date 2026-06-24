import logger from "@log/index.ts";
import { getMessage } from "@TDLib/function/get.ts";
import type { InlineContext, InlineResult, InlineResultSet } from "@TDLib/types/inline.ts";

import { getDatabase } from "@db/index.ts";
import type { Client } from "tdl";
import type { EpisodeResourceDoc } from "../types/episodeResource.js";


function parseEpisodeId(query: string): number | null {
    const text = query.trim();

    if (/^\d+$/.test(text)) {
        return Number(text);
    }

    const match = text.match(/https?:\/\/(?:bgm\.tv|bangumi\.tv)\/ep\/(\d+)(?:\/|$)/i);
    if (!match) return null;

    return Number(match[1]);
}

function buildResultId(prefix: string, epid: number, index = 0): string {
    return `${prefix}_${epid}_${index}`.slice(0, 64);
}

async function extractVideoIdFromMessage(client: Client, ref: { chat_id: number; message_id: number }): Promise<{ videoId?: string; coverId?: string, width?: number; height?: number }> {

    try {
        const message = await getMessage(client, ref.chat_id, ref.message_id);
        const content = message?.content;

        if (!content) return {};

        if (content._ === "messageVideo") {
            return {
                videoId: content.video?.video?.remote?.id,
                coverId: content.cover?.sizes.at(-1)?.photo?.remote?.id,
                width: content.video?.width,
                height: content.video?.height,
            };
        }

        return {};
    } catch (error) {
        logger.warn(error, "[XQAnimeBot][inline][episodeVideo] 读取消息失败");
        return {};
    }
}

export function matchEpisodeVideoInline(ctx: InlineContext): boolean | number {
    const epid = parseEpisodeId(ctx.query);
    if (!epid) return false;
    return 100;
}

export default async function episodeVideoInline(
    client: Client,
    ctx: InlineContext
): Promise<InlineResultSet> {
    const epid = parseEpisodeId(ctx.query);
    if (!epid) {
        return {
            results: [],
            cache_time: 0,
            is_personal: true,
        };
    }

    const db = await getDatabase();

    const resources = await db
        .collection<EpisodeResourceDoc>("resources")
        .find({
            $or: [
                { episodeId: epid },
            ],
        })
        .sort({ createdAt: -1 })
        .toArray();

    if (!resources.length) {
        return {
            results: [
                {
                    type: "article",
                    id: buildResultId("ep_res_not_found", epid, 0),
                    title: `EP ${epid} 暂无资源`,
                    description: "数据库没有该章节资源",
                    message: {
                        text: `https://bgm.tv/ep/${epid}`,
                        link_preview: true,
                    },
                },
            ],
            cache_time: 180,
            is_personal: false,
        };
    }

    const results: InlineResult[] = [];

    for (let i = 0; i < resources.length; i++) {
        const resource = resources[i]!;
        let videoId: string | undefined = resource.videoid;
        let coverId: string | undefined = undefined;
        let width: number | undefined = undefined;
        let height: number | undefined = undefined;
        const captionText = resource.message?.link ? `[${resource.title}](${resource.message.link})` : resource.title;

        if (resource.message?.chat_id && resource.message?.message_id) {
            const messageInfo = await extractVideoIdFromMessage(client, {
                chat_id: resource.message.chat_id,
                message_id: resource.message.message_id,
            });

            if (!videoId && messageInfo.videoId) {
                videoId = messageInfo.videoId;
            }

            if (!coverId && messageInfo.coverId) {
                coverId = messageInfo.coverId;
            }
            if (!width && messageInfo.width) {
                width = messageInfo.width;
            }
            if (!height && messageInfo.height) {
                height = messageInfo.height;
            }
        }

        if (!videoId && Array.isArray(resource.messages)) {
            for (const messageRef of resource.messages) {
                if (!messageRef?.chat_id || !messageRef?.message_id) continue;
                if (messageRef.videoid) {
                    videoId = messageRef.videoid;
                    break;
                }
                const messageInfo = await extractVideoIdFromMessage(client, {
                    chat_id: messageRef.chat_id,
                    message_id: messageRef.message_id,
                });
                if (messageInfo.videoId) {
                    videoId = messageInfo.videoId;
                    break;
                }
                if (!coverId && messageInfo.coverId) {
                    coverId = messageInfo.coverId;
                }
            }
        }

        const episodeLabel = resource.title ? `${resource.title}` : `EP ${epid}`;

        if (!videoId) {
            results.push({
                type: "article",
                id: buildResultId("ep_link", epid, i),
                title: `${episodeLabel}`,
                description: `epid: ${epid}`,
                message: {
                    text: resource.message?.link || `https://bgm.tv/ep/${epid}`,
                    link_preview: true,
                },
            });
            continue;
        }

        results.push({
            type: "video",
            id: buildResultId("ep_video", epid, i),
            title: episodeLabel,
            description: `epid: ${epid}`,
            video_url: videoId,
            thumbnail_url: coverId,
            message: {
                text: captionText || episodeLabel,
                media: {
                    video: {
                        id: videoId,

                    },
                    ...(coverId ? { cover: { id: coverId } } : {}),
                    width: width,
                    height: height,
                },
            },
        });
    }

    return {
        results,
        cache_time: 600,
        is_personal: false,
    };
}
