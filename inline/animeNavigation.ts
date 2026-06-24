import logger from "@log/index.ts";
import type { InlineContext, InlineResult, InlineResultSet } from "@TDLib/types/inline.ts";
import type { Client } from "tdl";
import { getAnimeById, searchAnime } from "../database/query.ts";
import type { anime } from "../types/anime.d.ts";
import { navmegtext } from "../anime/text.ts";

function buildResultId(prefix: string, value: string | number, index = 0): string {
    return `${prefix}_${value}_${index}`.slice(0, 64);
}

function buildSearchSummaryResult(
    query: string,
    animes: anime[]
): InlineResult {
    const summaryLines: string[] = [`🔍 **搜索结果** (找到 ${animes.length} 个匹配项)`];

    if (query) {
        summaryLines.push(`**搜索关键词：** ${query}`);
    }

    animes.forEach((anime) => {
        const title = anime.name_cn || anime.name || `ID ${anime.id}`;
        const originalName = anime.name || anime.name_cn || `ID ${anime.id}`;
        const messageLink = anime.navMessage?.link;

        summaryLines.push("");
        summaryLines.push(`🎬 ${title}`);
        summaryLines.push(originalName);

        if (messageLink) {
            summaryLines.push(`🔗 [查看详情](${messageLink})`);
        } else {
            summaryLines.push(``);
        }
    });

    return {
        type: "article",
        id: buildResultId("nav_search", query, animes.length),
        title: `🔍 搜索结果 (找到 ${animes.length} 个匹配项)`,
        description: query ? `搜索关键词：${query}` : undefined,
        message: {
            text: summaryLines.join("\n"),
            link_preview: false,
        },
    };
}



function isNumericQuery(query: string): boolean {
    return /^\d+$/.test(query);
}

export function matchAnimeNavigationInline(ctx: InlineContext): boolean | number {
    const query = ctx.query.trim();
    if (!query) return false;

    if (isNumericQuery(query)) {
        return 90;
    }

    return query.length >= 2 ? 70 : false;
}

export default async function animeNavigationInline(
    client: Client,
    ctx: InlineContext
): Promise<InlineResultSet> {
    const query = ctx.query.trim();
    const isNumeric = isNumericQuery(query);

    if (!isNumeric && query.length < 2) {
        return {
            results: [],
            cache_time: 0,
            is_personal: true,
        };
    }

    let animes: anime[] = [];

    if (isNumeric) {
        const anime = await getAnimeById(Number(query));
        if (anime) {
            animes = [anime];
        }
    } else {
        animes = await searchAnime(query);
    }

    const loaded = await Promise.allSettled(
        animes.map(async (item) => {
            return { item, anime: item };
        })
    );

    const results: InlineResult[] = [];

    results.push(
        buildSearchSummaryResult(
            query,
            loaded
                .filter((it): it is PromiseFulfilledResult<{ item: anime; anime: anime }> => it.status === "fulfilled")
                .map((it) => it.value.anime)
        )
    );

    for (let i = 0; i < loaded.length; i++) {
        const it = loaded[i]!;
        if (it.status !== "fulfilled") {
            logger.warn(it.reason, "[XQAnimeBot][inline][animeNavigation] 读取导航消息失败");
            continue;
        }

        const { item, anime } = it.value;
        const text = await navmegtext(client, anime);


        if (anime.image) {
            results.push({
                type: "photo",
                id: buildResultId("nav", item.id, i),
                title: item.name_cn || item.name || `ID ${item.id}`,
                description: `ID: ${item.id}`,
                photo_url: anime.image,
                message: {
                    text: text[0] || `导航消息: ${item.navMessage?.link || "无链接"}`,
                    media: {
                        photo: {
                            id: anime.image,
                        },
                    },
                    link_preview: true,
                },
            });
            continue;
        }

        results.push({
            type: "article",
            id: buildResultId("nav_text", item.id, i),
            title: item.name_cn || item.name || `ID ${item.id}`,
            description: `ID: ${item.id}`,
            message: {
                text: text[0] || `导航消息: ${item.navMessage?.link || "无链接"}`,
                link_preview: true,
            },
        });
    }

    return {
        results,
        cache_time: 300,
        is_personal: false,
    };
}
