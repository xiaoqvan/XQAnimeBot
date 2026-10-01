import type { RssAnimeItem, animeItem } from "./rss.d.ts";

/** 任务来源类型 */
export type TaskCheckpointKind = "rss" | "mkv" | "web";

/** 任务生命周期状态（仅 queued/running 会在重启时被恢复） */
export type TaskCheckpointStatus = "queued" | "running" | "done" | "failed" | "canceled";

/** Web 任务入口载荷（重启后按原始入口重放） */
export interface WebTaskPayload {
    type: "addanime" | "addnewanime";
    epid: number | string;
    url: string;
    /** 可选：已解析出的标题，用于进度展示 */
    title?: string;
}

/** 恢复时根据 kind 分发的载荷 */
export type TaskCheckpointPayload =
    | { kind: "rss"; item: RssAnimeItem }
    | { kind: "mkv"; item: animeItem }
    | { kind: "web"; web: WebTaskPayload };

/**
 * 任务检查点文档（MongoDB `task_checkpoints` 集合）
 *
 * 设计目标：应用崩溃/重启后，能根据检查点把未完成任务重新入队。
 * - 正常失败会写成 `failed`，不会被启动恢复拾取
 * - 崩溃时状态停留在 `queued` / `running`，启动恢复会拾取
 */
export interface TaskCheckpointDoc {
    /** 唯一键：RSS/MKV 用种子 title，web 用 `web:<taskId>` */
    key: string;
    kind: TaskCheckpointKind;
    /** 恢复所需完整载荷 */
    payload: TaskCheckpointPayload;
    /** 当前阶段文案（与 progressMap.stage 对齐，仅展示用） */
    stage: string;
    status: TaskCheckpointStatus;
    /** 已被启动恢复重放的次数，超过上限后不再恢复 */
    attempt: number;
    /** qBittorrent infoHash（进入下载阶段后可选填充） */
    torrentHash?: string;
    createdAt: Date;
    updatedAt: Date;
}
