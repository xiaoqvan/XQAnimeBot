import type { FastifyInstance } from "fastify";
import { animeRoutes } from "./anime.ts";
import { statsRoutes } from "./stats.ts";
import { progressRoutes } from "./progress.ts";
import { torrentsRoutes } from "./torrents.ts";
import { reviewsRoutes } from "./reviews.ts";
import { aiRoutes } from "./ai.ts";
import { tasksRoutes } from "./tasks.ts";
import { loginRoutes } from "./login.ts";

/**
 * 注册所有 API 路由
 */
export async function registerRoutes(app: FastifyInstance): Promise<void> {
    await app.register(statsRoutes);
    await app.register(animeRoutes);
    await app.register(progressRoutes);
    await app.register(torrentsRoutes);
    await app.register(reviewsRoutes);
    await app.register(aiRoutes);
    await app.register(tasksRoutes);
    await app.register(loginRoutes);
}
