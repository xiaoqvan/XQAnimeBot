import type { FastifyInstance } from "fastify";
import {
    createTask,
    listTasks,
    getTask,
    cancelTask,
    clearFinishedTasks,
} from "../tasks.ts";

/**
 * BT 任务路由（Web 触发 addanime / addnewanime 并追踪进度）
 */
export async function tasksRoutes(app: FastifyInstance): Promise<void> {
    /** 创建 addanime 任务（为已有番剧添加 BT 信息） */
    app.post<{ Body: { epid?: number | string; url?: string } }>(
        "/api/tasks/addanime",
        async (request, reply) => {
            const { epid, url } = request.body ?? {};
            if (!epid || !url) {
                return reply.code(400).send({ error: "需要提供 epid 和 url" });
            }
            if (!String(url).includes("bangumi") && !String(url).includes("dmhy")) {
                return reply.code(400).send({ error: "仅支持 bangumi / dmhy 链接" });
            }
            try {
                const id = await createTask("addanime", epid, url);
                return { ok: true, id };
            } catch (err) {
                return reply.code(500).send({ error: (err as Error).message });
            }
        }
    );

    /** 创建 addnewanime 任务（添加新番剧并关联 BT 信息） */
    app.post<{ Body: { epid?: number | string; url?: string } }>(
        "/api/tasks/addnewanime",
        async (request, reply) => {
            const { epid, url } = request.body ?? {};
            if (!epid || !url) {
                return reply.code(400).send({ error: "需要提供 epid 和 url" });
            }
            if (!String(url).includes("bangumi") && !String(url).includes("dmhy")) {
                return reply.code(400).send({ error: "仅支持 bangumi / dmhy 链接" });
            }
            try {
                const id = await createTask("addnewanime", epid, url);
                return { ok: true, id };
            } catch (err) {
                return reply.code(500).send({ error: (err as Error).message });
            }
        }
    );

    /** 任务列表 */
    app.get("/api/tasks", async () => {
        return { items: listTasks() };
    });

    /** 单个任务详情 */
    app.get<{ Params: { id: string } }>("/api/tasks/:id", async (request, reply) => {
        const id = Number(request.params.id);
        const task = getTask(id);
        if (!task) {
            return reply.code(404).send({ error: "任务不存在" });
        }
        return { task };
    });

    /** 取消任务 */
    app.post<{ Params: { id: string } }>(
        "/api/tasks/:id/cancel",
        async (request, reply) => {
            const id = Number(request.params.id);
            const ok = cancelTask(id);
            if (!ok) {
                return reply.code(404).send({ error: "任务不存在" });
            }
            return { ok: true, id };
        }
    );

    /** 清理已完成/失败任务 */
    app.post("/api/tasks/clear", async () => {
        clearFinishedTasks();
        return { ok: true };
    });
}
