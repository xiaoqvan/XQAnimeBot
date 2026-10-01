import type { Client } from "tdl";
import { getBotClient } from "./server.ts";
import { parseBtSource } from "./btParser.ts";
import { animeProcessor } from "../anime/AnimeProcessorManager.ts";
import { getAnimeById } from "../database/query.ts";
import { getEpisodeById } from "../bangumi/get.ts";
import {
    upsertCheckpoint,
    updateCheckpointStage,
    finishCheckpoint,
} from "../database/checkpoints.ts";
import type { animeItem } from "../types/rss.d.ts";
import type { WebTaskPayload } from "../types/taskCheckpoint.d.ts";

export type TaskType = "addanime" | "addnewanime";
export type TaskStatus = "queued" | "running" | "done" | "failed" | "canceled";

/** Web BT 任务记录 */
export interface BtTask {
    id: number;
    type: TaskType;
    epid: number | string;
    url: string;
    status: TaskStatus;
    title: string;
    animeName: string;
    createdAt: string;
    updatedAt: string;
    startTime?: string;
    /** 实时阶段（与 animeProcessor progressMap 的 stage 同步） */
    stage?: string;
    error?: string;
}

let taskSeq = 0;
const tasks = new Map<number, BtTask>();

/** Web 任务的稳定检查点 key（跨重启可定位同一入口） */
function webCheckpointKey(type: TaskType, epid: number | string, url: string): string {
    return `web:${type}:${epid}:${url}`;
}

/** 同步 progressMap 的 stage 到任务记录 */
function applyProgress(task: BtTask): void {
    const progress = animeProcessor.getProgress().find((p) => p.title === task.title);
    if (progress) {
        task.stage = progress.stage;
        task.animeName = progress.animeName ?? task.animeName;
        task.updatedAt = progress.updatedAt.toISOString();
    }
}

/**
 * 创建并启动一个 BT 任务（addanime / addnewanime）。
 * 返回任务 ID；任务在后台异步执行，进度经 animeProcessor 追踪。
 * @param clientOverride - 可选：直接指定 Bot client（崩溃恢复时用，不依赖 Web 服务已启动）
 */
export async function createTask(
    type: TaskType,
    epid: number | string,
    url: string,
    clientOverride?: Client
): Promise<number> {
    const client = clientOverride ?? getBotClient();
    if (!client) {
        throw new Error("Bot client 未就绪，无法执行 BT 任务");
    }

    taskSeq += 1;
    const id = taskSeq;
    const now = new Date().toISOString();
    const task: BtTask = {
        id,
        type,
        epid,
        url,
        status: "queued",
        title: "",
        animeName: "",
        createdAt: now,
        updatedAt: now,
    };
    tasks.set(id, task);

    await upsertCheckpoint({
        key: webCheckpointKey(type, epid, url),
        kind: "web",
        payload: { kind: "web", web: { type, epid, url } },
        stage: "排队中",
        status: "queued",
    }).catch(() => { });

    // 后台执行，不阻塞请求
    void runTask(id, client)
        .catch((err) => {
            const t = tasks.get(id);
            if (t) {
                t.status = "failed";
                t.error = (err as Error).message;
                t.updatedAt = new Date().toISOString();
            }
            void finishCheckpoint(
                webCheckpointKey(type, epid, url),
                "failed",
                (err as Error).message
            ).catch(() => { });
        });

    return id;
}

/**
 * 崩溃恢复：按保存的 Web 任务入口重新创建并执行。
 * 返回新任务 ID；若 Bot client 未就绪则抛错。
 */
export async function resumeWebTask(web: WebTaskPayload, client?: Client): Promise<number> {
    return createTask(web.type, web.epid, web.url, client);
}

async function runTask(id: number, client: Client): Promise<void> {
    const task = tasks.get(id);
    if (!task) return;

    const cpKey = webCheckpointKey(task.type, task.epid, task.url);
    task.status = "running";
    task.startTime = new Date().toISOString();
    task.updatedAt = task.startTime;
    void updateCheckpointStage(cpKey, "解析 BT 来源").catch(() => { });

    let item: animeItem | null = null;

    // 1. 解析 BT 来源（parseBtSource 抛出具体原因）
    task.stage = "解析 BT 来源";
    try {
        item = await parseBtSource(task.url);
    } catch (e) {
        throw new Error(`解析 BT 来源失败：${(e as Error).message}`);
    }
    if (!item || !item.magnet) {
        throw new Error("解析 BT 来源后未取到磁力链接");
    }
    task.title = item.title;
    task.animeName = item.names?.[0] ?? item.title;
    tasks.set(id, task);
    void updateCheckpointStage(cpKey, "解析完成").catch(() => { });

    // 2. 预置进度条目（以 BT title 为 key，供 handleExisting/handleNew 更新）
    //    这里不直接改 manager 私有 map，任务执行逻辑内部的 updateProgress 会写入。

    if (task.type === "addanime") {
        await runAddAnime(client, id, task, item);
    } else {
        await runAddNewAnime(client, id, task, item);
    }

    // 完成标记
    const t = tasks.get(id);
    if (t) {
        applyProgress(t);
        t.status = "done";
        t.stage = t.stage || "已完成";
        t.updatedAt = new Date().toISOString();
        tasks.set(id, t);
    }
    void finishCheckpoint(cpKey, "done", "已完成").catch(() => { });
}

async function runAddAnime(
    client: Client,
    id: number,
    task: BtTask,
    item: animeItem
): Promise<void> {
    // 通过 epid 定位番剧
    const epinfo = await getEpisodeById(Number(task.epid));
    if (!epinfo?.subject_id) {
        throw new Error(`未找到集数 ID=${task.epid} 对应的动漫`);
    }
    const anime = await getAnimeById(epinfo.subject_id, false);
    if (!anime) {
        throw new Error(`未找到 ID=${epinfo.subject_id} 的动漫信息`);
    }

    const { handleExistingAnime } = await import("../anime/animeHandlers.ts");
    // handleExistingAnime 内部会以 item.title 为 key 调 manager.updateProgress
    await handleExistingAnime(client, item, anime, animeProcessor);
    void id;
}

async function runAddNewAnime(
    client: Client,
    id: number,
    task: BtTask,
    item: animeItem
): Promise<void> {
    const { handleNewAnime } = await import("../anime/animeHandlers.ts");
    // handleNewAnime 会用 LLM 匹配番剧 → 先发后审/先审核后发，并以 item.title 追踪进度
    await handleNewAnime(client, item, animeProcessor);
    void id;
}

/** 任务列表（含实时阶段同步） */
export function listTasks(): BtTask[] {
    const arr: BtTask[] = [];
    for (const t of tasks.values()) {
        applyProgress(t);
        arr.push({ ...t });
    }
    return arr.sort((a, b) => b.id - a.id);
}

/** 获取单个任务 */
export function getTask(id: number): BtTask | null {
    const t = tasks.get(id);
    if (!t) return null;
    applyProgress(t);
    return { ...t };
}

/** 取消一个任务（若正处于 animeProcessor 活跃任务，则从其队列/进度移除） */
export function cancelTask(id: number): boolean {
    const t = tasks.get(id);
    if (!t) return false;
    t.status = "canceled";
    t.stage = "已取消";
    t.updatedAt = new Date().toISOString();
    tasks.set(id, t);
    void finishCheckpoint(
        webCheckpointKey(t.type, t.epid, t.url),
        "canceled",
        "已取消"
    ).catch(() => { });
    // 尝试从 animeProcessor 取消（若有对应活跃任务）
    if (t.title) {
        animeProcessor.cancelActiveByTitle(t.title);
    }
    return true;
}

/** 清理已完成的任务（可选） */
export function clearFinishedTasks(): void {
    for (const [id, t] of tasks) {
        if (t.status === "done" || t.status === "failed" || t.status === "canceled") {
            tasks.delete(id);
        }
    }
}
