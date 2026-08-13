import type { FastifyInstance } from "fastify";
import { searchAnime, getAnimeById, listAnimes, listAnimesBySeason, listAnimeSeasons, hasAnimeSend } from "../../database/query.ts";
import { getSubjectById } from "../../bangumi/get.ts";
import { saveAnime } from "../../database/create.ts";
import { deleteAnime } from "../../database/delete.ts";
import { extractFilteredTagNames } from "../../utils/index.ts";

function mapAnimeForList(a: Record<string, unknown>) {
    return {
        id: a.id,
        name: a.name,
        name_cn: a.name_cn,
        names: a.names,
        image: a.image,
        episode: a.episode,
        score: a.score,
        r18: a.r18,
        airingDay: a.airingDay,
        airingStart: a.airingStart,
        updatedAt: a.updatedAt instanceof Date ? a.updatedAt.toISOString() : a.updatedAt,
    };
}

/**
 * 动漫浏览 / 搜索 / 分页 / 添加 / 详情 路由
 */
export async function animeRoutes(app: FastifyInstance): Promise<void> {
    /** 年季分类统计（按年份 + 季节分组的番剧数量） */
    app.get("/api/anime/seasons", async (request, reply) => {
        try {
            const seasons = await listAnimeSeasons();
            return { items: seasons };
        } catch (err) {
            return reply.code(500).send({ error: (err as Error).message });
        }
    });

    /** 番剧分页列表（封面浏览；支持 season 参数按年季筛选） */
    app.get<{ Querystring: { page?: string; pageSize?: string; season?: string } }>("/api/anime", async (request, reply) => {
        const page = Number(request.query.page) || 1;
        const pageSize = Number(request.query.pageSize) || 30;
        const season = request.query.season ?? "all";
        try {
            const res =
                season && season !== "all"
                    ? await listAnimesBySeason(season, page, pageSize)
                    : await listAnimes(page, pageSize);
            return {
                items: res.items.map(mapAnimeForList),
                total: res.total,
                page: res.page,
                pageSize: res.pageSize,
                season,
            };
        } catch (err) {
            return reply.code(500).send({ error: (err as Error).message });
        }
    });

    /** 添加番剧到动漫库（支持 Bangumi subjectId 或手动信息） */
    app.post<{
        Body: {
            subjectId?: number | string;
            name?: string;
            name_cn?: string;
            image?: string;
            summary?: string;
            tags?: string[];
            episode?: string;
            score?: number | string;
            r18?: boolean;
            airingDay?: string;
            airingStart?: string;
        };
    }>("/api/anime", async (request, reply) => {
        const body = request.body ?? {};
        try {
            // 方式一：通过 Bangumi subjectId 拉取
            if (body.subjectId) {
                const subject = await getSubjectById(Number(body.subjectId));
                if (!subject?.id) {
                    return reply.code(400).send({ error: "Bangumi 条目不存在或拉取失败" });
                }
                const filteredTags = await extractFilteredTagNames(subject.tags ?? []);
                const image =
                    subject.images?.large ??
                    subject.images?.common ??
                    subject.images?.medium ??
                    "";
                const exists = await hasAnimeSend([subject.name, subject.name_cn ?? ""]);
                const doc = {
                    id: subject.id,
                    name: subject.name,
                    name_cn: subject.name_cn,
                    names: exists?.names ?? [],
                    image,
                    summary: subject.summary,
                    tags: filteredTags,
                    episode: subject.eps ? `${subject.eps} 集` : (subject.total_episodes ? `${subject.total_episodes} 集` : ""),
                    score: subject.rating?.score,
                    airingDay: "",
                    airingStart: subject.date,
                    r18: body.r18 ?? false,
                };
                await saveAnime(doc as never, false);
                return { ok: true, id: subject.id, anime: mapAnimeForList(doc as Record<string, unknown>) };
            }

            // 方式二：手动字段添加
            if (!body.name) {
                return reply.code(400).send({ error: "需要提供 subjectId 或 name" });
            }
            let id = Number(body.name);
            if (Number.isNaN(id) || String(body.name).trim() !== String(id)) {
                // 手动添加时若无 id，无法保证唯一自增，这里要求 id（即 Bangumi/数字主键）
                return reply.code(400).send({
                    error: "手动添加需提供数字 id 作为主键，建议使用 Bangumi subjectId",
                });
            }
            await saveAnime({
                id,
                name: body.name,
                name_cn: body.name_cn,
                image: body.image ?? "",
                summary: body.summary,
                tags: body.tags ?? [],
                episode: body.episode,
                score: body.score,
                r18: body.r18 ?? false,
                airingDay: body.airingDay,
                airingStart: body.airingStart,
            } as never, false);
            return { ok: true, id };
        } catch (err) {
            return reply.code(500).send({ error: (err as Error).message });
        }
    });

    /** 搜索番剧（关键词），返回最多 20 条 */
    app.get<{ Querystring: { q?: string } }>("/api/anime/search", async (request, reply) => {
        const q = (request.query.q ?? "").trim();
        if (q.length < 2) {
            return reply.code(400).send({ error: "搜索关键词至少需要 2 个字符" });
        }
        try {
            const results = await searchAnime(q);
            return { items: results.map(mapAnimeForList), total: results.length };
        } catch (err) {
            return reply.code(500).send({ error: (err as Error).message });
        }
    });

    /** 获取番剧详情（含章节与资源分组） */
    app.get<{ Params: { id: string } }>("/api/anime/:id", async (request, reply) => {
        const id = Number(request.params.id);
        if (!Number.isFinite(id)) {
            return reply.code(400).send({ error: "无效的番剧 ID" });
        }
        try {
            const anime = await getAnimeById(id, false);
            if (!anime) {
                return reply.code(404).send({ error: "未找到该番剧" });
            }
            return { anime };
        } catch (err) {
            return reply.code(500).send({ error: (err as Error).message });
        }
    });

    /** 删除番剧（连同其资源、章节、待审核记录） */
    app.delete<{ Params: { id: string } }>("/api/anime/:id", async (request, reply) => {
        const id = Number(request.params.id);
        if (!Number.isFinite(id)) {
            return reply.code(400).send({ error: "无效的番剧 ID" });
        }
        try {
            const ok = await deleteAnime(id);
            if (!ok) {
                return reply.code(404).send({ error: "未找到该番剧" });
            }
            return { ok: true, id };
        } catch (err) {
            return reply.code(500).send({ error: (err as Error).message });
        }
    });
}
