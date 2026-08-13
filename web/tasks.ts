import type { Client } from "tdl";
import { getBotClient } from "./server.ts";
import { parseBtSource } from "./btParser.ts";
import { animeProcessor } from "../anime/AnimeProcessorManager.ts";
import { getAnimeById } from "../database/query.ts";
import { getEpisodeById } from "../bangumi/get.ts";
import type { animeItem } from "../types/rss.d.ts";

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
 */
export async function createTask(
    type: TaskType,
    epid: number | string,
    url: string
): Promise<number> {
    const client = getBotClient();
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

    // 后台执行，不阻塞请求
    void runTask(id, client)
        .catch((err) => {
            const t = tasks.get(id);
            if (t) {
                t.status = "failed";
                t.error = (err as Error).message;
                t.updatedAt = new Date().toISOString();
            }
        });

    return id;
}

async function runTask(id: number, client: Client): Promise<void> {
    const task = tasks.get(id);
    if (!task) return;

    task.status = "running";
    task.startTime = new Date().toISOString();
    task.updatedAt = task.startTime;

    let item: animeItem | null = null;

    // 1. 解析 BT 来源
    task.stage = "解析 BT 来源";
    item = await parseBtSource(task.url);
    if (!item || !item.magnet) {
        throw new Error("BT 链接解析失败（仅支持 bangumi / dmhy）");
    }
    task.title = item.title;
    task.animeName = item.names?.[0] ?? item.title;
    tasks.set(id, task);

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
    await handleExistingAnime(client, item, anime as never, animeProcessor);
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
