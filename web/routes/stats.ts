import type { FastifyInstance } from "fastify";
import { getDatabase } from "@db/index.ts";
import { animeProcessor } from "../../anime/AnimeProcessorManager.ts";

/**
 * 概览统计路由
 */
export async function statsRoutes(app: FastifyInstance): Promise<void> {
    /** 健康检查（无需鉴权） */
    app.get("/api/health", async () => {
        return { status: "ok", time: new Date().toISOString() };
    });

    /** 概览统计（动漫库规模与处理状态） */
    app.get("/api/stats", async (request, reply) => {
        try {
            const db = await getDatabase();

            const [animeCount, cacheAnimeCount, torrentCount, episodeCount] =
                await Promise.all([
                    db.collection("anime").estimatedDocumentCount(),
                    db.collection("cacheAnime").estimatedDocumentCount(),
                    db.collection("torrents").estimatedDocumentCount(),
                    db.collection("episodes_meta").estimatedDocumentCount(),
                ]);

            return {
                animeCount,
                cacheAnimeCount,
                torrentCount,
                episodeCount,
                processing: {
                    active: animeProcessor.getActiveCount(),
                    queued: animeProcessor.getQueueSize(),
                },
                time: new Date().toISOString(),
            };
        } catch (err) {
            return reply.code(500).send({ error: (err as Error).message });
        }
    });
}
