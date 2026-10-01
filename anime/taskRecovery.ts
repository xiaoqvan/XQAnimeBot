import logger from "@log/index.ts";
import { animeProcessor } from "./AnimeProcessorManager.ts";
import {
    findResumableCheckpoints,
    giveUpExhaustedCheckpoints,
    incrementCheckpointAttempt,
} from "../database/checkpoints.ts";
import type { TaskCheckpointDoc } from "../types/taskCheckpoint.d.ts";
import type { Client } from "tdl";

/**
 * 启动时恢复被中断的任务（任务中继）。
 *
 * 场景：BT 下载到一半应用崩溃/重启。
 * - qBittorrent 侧的种子与半截文件会保留，`downloadTorrentFromUrl` 复用已有种子继续下
 * - 本函数把仍在 `queued`/`running` 的检查点重新灌回处理队列，接上后续转码/发送
 *
 * 正常失败的任务状态是 `failed`，不会被这里拾取；只有崩溃时残留的
 * `queued`/`running` 才会被恢复。每次恢复 attempt+1，超过上限后放弃。
 *
 * @param client - TDLib 客户端实例
 */
export async function recoverInterruptedTasks(client: Client): Promise<void> {
    try {
        await giveUpExhaustedCheckpoints();
        const pending = await findResumableCheckpoints();
        if (pending.length === 0) {
            logger.info("[TaskRecovery] 没有需要恢复的中断任务");
            return;
        }

        logger.info(`[TaskRecovery] 发现 ${pending.length} 个中断任务，开始恢复`);

        let recovered = 0;
        for (const cp of pending) {
            try {
                await recoverOne(client, cp);
                recovered++;
            } catch (err) {
                logger.error(err, `[TaskRecovery] 恢复任务失败: ${cp.key}`);
            }
        }

        logger.info(`[TaskRecovery] 恢复完成：成功入队 ${recovered}/${pending.length}`);
    } catch (err) {
        // 恢复流程出错不影响 Bot 主启动
        logger.error(err, "[TaskRecovery] 启动恢复流程出错");
    }
}

async function recoverOne(client: Client, cp: TaskCheckpointDoc): Promise<void> {
    // 先计数再入队：进程再次崩溃时 attempt 已递增，避免毒任务无限恢复
    await incrementCheckpointAttempt(cp.key);
    const payload = cp.payload;

    if (payload.kind === "rss") {
        logger.info(`[TaskRecovery] 恢复 RSS 任务: ${cp.key}`);
        await animeProcessor.enqueue(client, [payload.item], { resume: true });
        return;
    }

    if (payload.kind === "mkv") {
        logger.info(`[TaskRecovery] 恢复 MKV 任务: ${cp.key}`);
        await animeProcessor.enqueueMkv(client, payload.item, true);
        return;
    }

    if (payload.kind === "web") {
        logger.info(`[TaskRecovery] 恢复 Web 任务: ${cp.key}`);
        const { resumeWebTask } = await import("../web/tasks.ts");
        await resumeWebTask(payload.web, client);
        return;
    }

    logger.warn(`[TaskRecovery] 未知任务类型，跳过: ${cp.key}`);
}
