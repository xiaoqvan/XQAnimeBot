import type { FastifyInstance } from "fastify";

/** BT 状态 → 中文说明 */
const QB_STATE_ZH: Record<string, string> = {
    downloading: "下载中",
    stalledDL: "等待下载",
    metaDL: "获取元数据",
    forcedDL: "强制下载",
    checkingDL: "校验中",
    pausedDL: "已暂停",
    stoppedDL: "已停止",
    queuedDL: "排队中",
    uploading: "做种中",
    stalledUP: "做种等待",
    forcedUP: "强制做种",
    checkingUP: "校验中",
    pausedUP: "已暂停",
    stoppedUP: "已完成",
    queuedUP: "排队中",
    allocating: "分配磁盘",
    moving: "移动文件",
    checkingResumeData: "校验数据",
    error: "错误",
    missingFiles: "文件缺失",
    unknown: "未知",
};

function formatBytes(bytes: number | undefined): string {
    if (bytes === undefined || bytes === null || Number.isNaN(bytes)) return "—";
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
    return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}

function formatSpeed(bps: number | undefined): string {
    if (!bps) return "0 B/s";
    return `${formatBytes(bps)}/s`;
}

/**
 * 从 hash 创建 qB 客户端方法的统一错误处理
 */
async function withQB<T>(fn: () => Promise<T>): Promise<T> {
    const { getQBClient } = await import("../../qBittorrent/index.ts");
    const client = await getQBClient();
    return fn();
}

/**
 * BT 下载列表与管理路由
 */
export async function torrentsRoutes(app: FastifyInstance): Promise<void> {
    /** 获取 BT 下载列表（含状态、进度、速度） */
    app.get("/api/torrents", async (request, reply) => {
        try {
            const list = await withQB(async () => {
                const { getQBClient } = await import("../../qBittorrent/index.ts");
                const c = await getQBClient();
                return c.getTorrents();
            });
            return {
                items: list.map((t) => ({
                    hash: t.hash,
                    name: t.name,
                    state: t.state,
                    stateLabel: QB_STATE_ZH[t.state] ?? t.state,
                    progress: t.progress,
                    size: t.size,
                    sizeLabel: formatBytes(t.size),
                    downloaded: t.downloaded,
                    uploaded: t.uploaded,
                    dlspeed: t.dlspeed ?? t.dl_speed,
                    upspeed: t.upspeed,
                    dlspeedLabel: formatSpeed(t.dlspeed ?? t.dl_speed),
                    upspeedLabel: formatSpeed(t.upspeed),
                    ratio: t.ratio,
                    num_leechs: t.num_leechs,
                    num_seeds: t.num_seeds,
                    eta: t.eta,
                    save_path: t.save_path,
                    content_path: t.content_path,
                    tags: t.tags,
                    added_on: t.added_on,
                    completion_on: t.completion_on,
                    magnet_uri: t.magnet_uri,
                })),
            };
        } catch (err) {
            return reply.code(500).send({ error: (err as Error).message });
        }
    });

    /** 暂停种子 */
    app.post<{ Params: { hash: string } }>(
        "/api/torrents/:hash/pause",
        async (request, reply) => {
            try {
                await withQB(async () => {
                    const { getQBClient } = await import("../../qBittorrent/index.ts");
                    const c = await getQBClient();
                    await c.pauseTorrent(request.params.hash);
                });
                return { ok: true };
            } catch (err) {
                return reply.code(500).send({ error: (err as Error).message });
            }
        }
    );

    /** 恢复种子 */
    app.post<{ Params: { hash: string } }>(
        "/api/torrents/:hash/resume",
        async (request, reply) => {
            try {
                await withQB(async () => {
                    const { getQBClient } = await import("../../qBittorrent/index.ts");
                    const c = await getQBClient();
                    await c.resumeTorrent(request.params.hash);
                });
                return { ok: true };
            } catch (err) {
                return reply.code(500).send({ error: (err as Error).message });
            }
        }
    );

    /** 删除种子（默认同时删除数据） */
    app.post<{ Params: { hash: string }; Body?: { deleteFiles?: boolean } }>(
        "/api/torrents/:hash/delete",
        async (request, reply) => {
            try {
                const deleteFiles = request.body?.deleteFiles ?? true;
                await withQB(async () => {
                    const { getQBClient } = await import("../../qBittorrent/index.ts");
                    const c = await getQBClient();
                    await c.deleteTorrent(request.params.hash, deleteFiles);
                });
                return { ok: true };
            } catch (err) {
                return reply.code(500).send({ error: (err as Error).message });
            }
        }
    );

    /** 获取传输总览（下载/上传速度、全局大小） */
    app.get("/api/torrents/transfer", async (request, reply) => {
        try {
            const info = await withQB(async () => {
                const { getQBClient } = await import("../../qBittorrent/index.ts");
                const c = await getQBClient();
                return c.getTransferInfo();
            });
            return {
                ...info,
                dlSpeedLabel: formatSpeed(info.dl_info_speed as number),
                upSpeedLabel: formatSpeed(info.up_info_speed as number),
            };
        } catch (err) {
            return reply.code(500).send({ error: (err as Error).message });
        }
    });
}
