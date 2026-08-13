import type { FastifyInstance } from "fastify";
import { listAiCalls } from "../../database/ai.ts";

const SCENE_ZH: Record<string, string> = {
    bangumi_match: "番剧匹配",
    episode_match: "集数匹配",
    episode_extract: "集数提取",
};

/**
 * AI 调用记录路由（展示插件内的 LLM 调用历史）
 */
export async function aiRoutes(app: FastifyInstance): Promise<void> {
    /** 获取 AI 调用记录（分页） */
    app.get<{
        Querystring: { page?: string; pageSize?: string; scene?: string };
    }>("/api/ai-calls", async (request, reply) => {
        const page = Number(request.query.page) || 1;
        const pageSize = Number(request.query.pageSize) || 20;
        const scene = request.query.scene as
            | "bangumi_match"
            | "episode_match"
            | "episode_extract"
            | undefined;

        try {
            const res = await listAiCalls(page, pageSize, scene);
            return {
                ...res,
                items: res.items.map((c) => ({
                    id: (c as { _id?: unknown })._id,
                    scene: c.scene,
                    sceneLabel: SCENE_ZH[c.scene] ?? c.scene,
                    input: c.input,
                    output: c.output,
                    success: c.success,
                    model: c.model,
                    durationMs: c.durationMs,
                    meta: c.meta,
                    createdAt: c.createdAt.toISOString(),
                })),
            };
        } catch (err) {
            return reply.code(500).send({ error: (err as Error).message });
        }
    });
}
