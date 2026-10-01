import logger from "@log/index.ts";
import { getDatabase } from "@db/index.ts";
import { getErrorMessage } from "@utils/error.ts";
import type {
    TaskCheckpointDoc,
    TaskCheckpointKind,
    TaskCheckpointPayload,
    TaskCheckpointStatus,
} from "../types/taskCheckpoint.d.ts";

const db = await getDatabase();

const col = () => db.collection<TaskCheckpointDoc>("task_checkpoints");

/** 启动恢复允许的最大重放次数，超过后标记 failed 不再恢复 */
export const MAX_RESUME_ATTEMPTS = 3;

/**
 * 写入/更新检查点（按 key upsert）。
 * 不覆盖已有的 attempt 计数；status/stage/payload 每次以最新为准。
 */
export async function upsertCheckpoint(doc: {
    key: string;
    kind: TaskCheckpointKind;
    payload: TaskCheckpointPayload;
    stage: string;
    status: TaskCheckpointStatus;
    torrentHash?: string;
}): Promise<void> {
    try {
        await col().updateOne(
            { key: doc.key },
            {
                $set: {
                    kind: doc.kind,
                    payload: doc.payload,
                    stage: doc.stage,
                    status: doc.status,
                    updatedAt: new Date(),
                    ...(doc.torrentHash ? { torrentHash: doc.torrentHash } : {}),
                },
                $setOnInsert: {
                    key: doc.key,
                    attempt: 0,
                    createdAt: new Date(),
                },
            },
            { upsert: true }
        );
    } catch (err) {
        // 检查点写失败不阻断主流程，仅记录
        logger.warn(err, `[taskCheckpoint] upsert 失败: ${getErrorMessage(err)}`);
    }
}

/** 更新阶段文案（可选同步 torrentHash） */
export async function updateCheckpointStage(
    key: string,
    stage: string,
    torrentHash?: string
): Promise<void> {
    try {
        await col().updateOne(
            { key },
            {
                $set: {
                    stage,
                    updatedAt: new Date(),
                    ...(torrentHash ? { torrentHash } : {}),
                },
            }
        );
    } catch {
        // 忽略
    }
}

/** 标记完成/失败/取消，并在失败时累加 attempt（已取消的任务不被 done/failed 覆盖） */
export async function finishCheckpoint(
    key: string,
    status: Extract<TaskCheckpointStatus, "done" | "failed" | "canceled">,
    stage?: string
): Promise<void> {
    try {
        const update: Record<string, unknown> = {
            status,
            updatedAt: new Date(),
        };
        if (stage) update.stage = stage;
        if (status === "canceled") {
            await col().updateOne({ key }, { $set: update });
            return;
        }
        // 取消是用户显式操作，完成后不要再改写状态
        const filter: { key: string; status?: { $ne: TaskCheckpointStatus } } = {
            key,
            status: { $ne: "canceled" as TaskCheckpointStatus },
        };
        if (status === "failed") {
            await col().updateOne(filter, [
                {
                    $set: {
                        ...update,
                        attempt: { $add: [{ $ifNull: ["$attempt", 0] }, 1] },
                    },
                },
            ]);
            return;
        }
        await col().updateOne(filter, { $set: update });
    } catch {
        // 忽略
    }
}

/**
 * 查询启动时需要恢复的检查点：
 * status ∈ {queued, running} 且 attempt < MAX_RESUME_ATTEMPTS
 */
export async function findResumableCheckpoints(): Promise<TaskCheckpointDoc[]> {
    try {
        return await col()
            .find({
                status: { $in: ["queued", "running"] },
                attempt: { $lt: MAX_RESUME_ATTEMPTS },
            })
            .sort({ createdAt: 1 })
            .toArray();
    } catch (err) {
        logger.error(err, `[taskCheckpoint] 查询可恢复任务失败: ${getErrorMessage(err)}`);
        return [];
    }
}

/** 把超过重放上限仍处于 queued/running 的检查点标记为 failed */
export async function giveUpExhaustedCheckpoints(): Promise<void> {
    try {
        await col().updateMany(
            {
                status: { $in: ["queued", "running"] },
                attempt: { $gte: MAX_RESUME_ATTEMPTS },
            },
            {
                $set: {
                    status: "failed",
                    stage: "重放次数用尽，放弃恢复",
                    updatedAt: new Date(),
                },
            }
        );
    } catch {
        // 忽略
    }
}

/** 恢复重放前累加 attempt（崩溃不会走 finish，必须在恢复入口计数，防止毒任务死循环） */
export async function incrementCheckpointAttempt(key: string): Promise<void> {
    try {
        await col().updateOne(
            { key },
            {
                $set: { updatedAt: new Date() },
                $inc: { attempt: 1 },
            }
        );
    } catch {
        // 忽略
    }
}

/** 按 key 读取检查点 */
export async function getCheckpoint(key: string): Promise<TaskCheckpointDoc | null> {
    try {
        return await col().findOne({ key });
    } catch {
        return null;
    }
}

/** 清理终态检查点（可选维护接口） */
export async function clearFinishedCheckpoints(): Promise<void> {
    try {
        await col().deleteMany({ status: { $in: ["done", "failed", "canceled"] } });
    } catch {
        // 忽略
    }
}
