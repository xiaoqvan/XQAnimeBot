import type { FastifyInstance } from "fastify";
import { animeProcessor } from "../../anime/AnimeProcessorManager.ts";
import { getSmartDelayInfo } from "../../utils/index.ts";

/** qBittorrent 种子状态 → 中文说明（与 progress 命令保持一致，供前端展示） */
const QB_STATE_ZH: Record<string, string> = {
    downloading: "下载中",
    stalledDL: "等待下载（无速度）",
    metaDL: "获取种子元数据",
    forcedDL: "强制下载",
    checkingDL: "校验（下载）",
    pausedDL: "已暂停（下载）",
    stoppedDL: "已停止（下载）",
    queuedDL: "排队（下载）",
    uploading: "上传做种中",
    stalledUP: "做种等待（无速度）",
    forcedUP: "强制做种",
    checkingUP: "校验（做种）",
    pausedUP: "已暂停（做种）",
    stoppedUP: "做种完成",
    queuedUP: "排队（做种）",
    allocating: "分配磁盘空间",
    moving: "移动文件",
    checkingResumeData: "校验恢复数据",
    error: "错误",
    missingFiles: "文件缺失",
    unknown: "未知",
};

/**
 * 处理进度 / qBittorrent 状态 路由
 */
export async function progressRoutes(app: FastifyInstance): Promise<void> {
    /** 获取当前处理进度总览（活跃任务、队列、延迟信息） */
    app.get("/api/progress", async () => {
        const activeItems = animeProcessor.getProgress();
        const delayInfo = getSmartDelayInfo();

        // 若存在已进入下载阶段的种子，尝试查询 qBittorrent 实时状态（失败则降级）
        let torrentStates: Record<string, { state: string; label: string }> = {};
        const hasHash = activeItems.some((p) => p.torrentHash);
        if (hasHash) {
            try {
                const { getQBClient } = await import("../../qBittorrent/index.ts");
                const QBclient = await getQBClient();
                const torrents = await QBclient.getTorrents();
                torrentStates = Object.fromEntries(
                    torrents.map((t) => [
                        t.hash.toLowerCase(),
                        { state: t.state, label: QB_STATE_ZH[t.state] ?? t.state },
                    ])
                );
            } catch {
                // qBittorrent 暂不可用，跳过状态
            }
        }

        return {
            activeCount: animeProcessor.getActiveCount(),
            queueSize: animeProcessor.getQueueSize(),
            delay: {
                intervalMs: delayInfo.intervalMs,
                waitMs: delayInfo.waitMs,
                waitEnd: delayInfo.waitEnd.toISOString(),
                currentHourInBeijing: delayInfo.currentHourInBeijing,
                nextChangeHourInBeijing: delayInfo.nextChangeHourInBeijing,
            },
            items: activeItems.map((item) => ({
                title: item.title,
                animeName: item.animeName,
                stage: item.stage,
                torrentHash: item.torrentHash,
                startTime: item.startTime.toISOString(),
                updatedAt: item.updatedAt.toISOString(),
                qb: item.torrentHash
                    ? torrentStates[item.torrentHash.toLowerCase()]
                    : undefined,
            })),
        };
    });

    /** 取消一个活跃任务（按标题） */
    app.post<{ Body: { title?: string } }>(
        "/api/progress/cancel",
        async (request, reply) => {
            const title = request.body?.title;
            if (!title) {
                return reply.code(400).send({ error: "缺少任务标题" });
            }
            const result = animeProcessor.cancelActiveByTitle(title);
            if (!result.ok) {
                return reply.code(404).send({ error: result.message });
            }
            return { ok: true, message: result.message };
        }
    );

    /** 取消全部活跃任务 */
    app.post("/api/progress/cancel-all", async () => {
        const results = animeProcessor.cancelAllActive();
        return { results };
    });
}
