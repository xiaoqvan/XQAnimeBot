import logger from "@log/index.ts";
import { ErrorHandler } from "../utils/ErrorHandler.ts";
import { handleRssAnimeItem } from "./rssItemHandler.ts";
import { hasTorrentTitle } from "../database/query.ts";
import type { RssAnimeItem } from "../types/rss.d.ts";
import type { Client } from "tdl";

/**
 * 单个处理任务的实时进度信息
 */
export interface ProgressInfo {
    /** BT 种子原始标题（用作任务唯一 key） */
    title: string;
    /** 番剧名称，解析/搜索成功后填充 */
    animeName?: string;
    /** 当前处理阶段的文字描述 */
    stage: string;
    /** qBittorrent 种子 infoHash（进入下载阶段后填充，用于查询实时状态） */
    torrentHash?: string;
    /** 任务开始处理的时间 */
    startTime: Date;
    /** 最后一次阶段更新的时间 */
    updatedAt: Date;
}

/** 取消任务的执行结果 */
export interface CancelResult {
    /** 是否执行成功 */
    ok: boolean;
    /** 目标任务标题 */
    title?: string;
    /** 结果描述 */
    message: string;
}

/** 内部队列条目，封装了 client 和 RSS 条目 */
interface QueueItem {
    client: Client;
    item: RssAnimeItem;
}

/**
 * 动漫处理并发管理器
 *
 * 维护固定大小的 worker 槽位池：
 * - 每个 worker 完成（无论成功或失败）后，立即从队列取下一个任务，
 *   不等待其他 worker，从而避免"一个卡住导致整批等待"的问题。
 * - 通过 {@link progressMap} 追踪每个活跃任务的处理阶段，供 /progress 命令查询。
 */
export class AnimeProcessorManager {
    /** 允许同时运行的最大 worker 数量 */
    private readonly maxConcurrency: number;

    /** 活跃任务的进度 Map，key 为 BT 种子标题 */
    private readonly progressMap: Map<string, ProgressInfo> = new Map();

    /** 待处理的任务队列（FIFO） */
    private readonly queue: QueueItem[] = [];

    /** 当前正在运行的 worker 数量（原子计数） */
    private activeCount = 0;

    /** 记录已经被“强制释放槽位”的任务标题，避免 finally 重复扣减 activeCount */
    private readonly forceReleasedTitles: Set<string> = new Set();

    /**
     * 创建管理器实例
     * @param maxConcurrency - 最大并发 worker 数量，默认 3
     */
    constructor(maxConcurrency = 3) {
        this.maxConcurrency = maxConcurrency;
    }

    /**
     * 将 RSS 条目批量加入队列（入队前先按种子标题做数据库去重过滤），并立即尝试启动新 worker 填满空闲槽位
     * @param client - TDLib 客户端实例
     * @param items - 需要处理的 RSS 动漫条目列表
     * @returns 入队统计：added 为成功入队数量，filtered 为被 hasTorrentTitle 过滤掉的数量
     */
    async enqueue(
        client: Client,
        items: RssAnimeItem[]
    ): Promise<{ added: number; filtered: number }> {
        let added = 0;
        let filtered = 0;

        for (const item of items) {
            try {
                const exists = await hasTorrentTitle(item.title);
                if (exists) {
                    filtered++;
                    continue;
                }
            } catch (error) {
                // 查询失败时不阻塞主流程，保守策略为继续入队
                logger.warn(error, `[AnimeProcessor] 入队前去重查询失败，继续入队: ${item.title}`);
            }

            this.queue.push({ client, item });
            added++;
        }

        this.trySpawnWorkers();
        return { added, filtered };
    }

    /**
     * 获取当前所有活跃任务的进度快照（浅拷贝数组，不影响内部状态）
     * @returns 进度信息数组，每一项对应一个正在处理中的条目
     */
    getProgress(): ProgressInfo[] {
        return Array.from(this.progressMap.values());
    }

    /**
     * 获取尚未开始处理、仍在排队中的任务数量
     * @returns 队列长度
     */
    getQueueSize(): number {
        return this.queue.length;
    }

    /**
     * 获取当前正在运行的 worker 数量
     * @returns 活跃 worker 数量
     */
    getActiveCount(): number {
        return this.activeCount;
    }

    /**
     * 按 /progress 展示顺序取消指定序号的活跃任务
     * @param index - 任务序号（从 1 开始）
     * @returns 取消结果
     */
    cancelActiveByIndex(index: number): CancelResult {
        const items = this.getProgress();
        if (!Number.isInteger(index) || index < 1 || index > items.length) {
            return {
                ok: false,
                message: `无效序号: ${index}，当前活跃任务数为 ${items.length}`,
            };
        }
        const target = items[index - 1];
        return this.cancelActiveByTitle(target.title);
    }

    /**
     * 按标题取消活跃任务，并立即释放并发槽位
     * @param title - 任务标题
     * @returns 取消结果
     */
    cancelActiveByTitle(title: string): CancelResult {
        const current = this.progressMap.get(title);
        if (!current) {
            return {
                ok: false,
                message: `未找到活跃任务: ${title}`,
            };
        }

        this.progressMap.delete(title);
        if (!this.forceReleasedTitles.has(title)) {
            this.forceReleasedTitles.add(title);
            this.activeCount = Math.max(0, this.activeCount - 1);
        }
        // 立即尝试补位，避免因为堵塞任务占槽而停滞
        this.trySpawnWorkers();

        return {
            ok: true,
            title,
            message: `已取消任务并释放并发槽位: ${title}`,
        };
    }

    /**
     * 取消当前全部活跃任务，并立即释放并发槽位
     * @returns 取消结果列表
     */
    cancelAllActive(): CancelResult[] {
        const items = this.getProgress();
        if (items.length === 0) {
            return [{ ok: false, message: "当前没有可取消的活跃任务" }];
        }
        return items.map((item) => this.cancelActiveByTitle(item.title));
    }

    /**
     * 更新指定任务的进度阶段及可选的附加信息
     * 若任务不在活跃 Map 中（已完成或未开始），则调用无效
     * @param title - BT 种子标题（任务唯一标识）
     * @param stage - 新的阶段描述文本
     * @param extra - 可选的额外字段：animeName 或 torrentHash
     */
    updateProgress(
        title: string,
        stage: string,
        extra?: Partial<Pick<ProgressInfo, "animeName" | "torrentHash">>
    ): void {
        const current = this.progressMap.get(title);
        if (current) {
            this.progressMap.set(title, {
                ...current,
                ...extra,
                stage,
                updatedAt: new Date(),
            });
        }
    }

    /**
     * 尝试启动尽可能多的新 worker，直到达到并发上限或队列为空
     * 每次 worker 完成时也会被调用，以自动补充槽位
     */
    private trySpawnWorkers(): void {
        while (this.activeCount < this.maxConcurrency && this.queue.length > 0) {
            const next = this.queue.shift()!;
            this.spawnWorker(next.client, next.item);
        }
    }

    /**
     * 以非阻塞方式启动单个 worker 处理指定条目
     * Worker 完成（无论成功/失败）后自动尝试从队列补充下一个任务
     * @param client - TDLib 客户端实例
     * @param item - 待处理的 RSS 条目
     */
    private spawnWorker(client: Client, item: RssAnimeItem): void {
        this.activeCount++;
        this.progressMap.set(item.title, {
            title: item.title,
            stage: "初始化",
            startTime: new Date(),
            updatedAt: new Date(),
        });

        // 注意：intentionally 不 await，让 worker 独立运行不阻塞调用方
        handleRssAnimeItem(client, item, this)
            .catch((error: unknown) => {
                logger.error(error, `[AnimeProcessor] 处理动漫项出错: ${item.title}`);
                ErrorHandler(
                    client,
                    new Error(`处理动漫项出错: ${item.title}\n${String(error)}`)
                ).catch(() => { });
            })
            .finally(() => {
                const wasForceReleased = this.forceReleasedTitles.delete(item.title);
                if (!wasForceReleased) {
                    this.activeCount = Math.max(0, this.activeCount - 1);
                    this.progressMap.delete(item.title);
                }
                // Worker 退出后立即尝试补充新任务，保持槽位始终满载
                this.trySpawnWorkers();
            });
    }
}

/**
 * 全局单例（最大并发数 3）
 *
 * 由 newindex.ts 的主循环写入条目，由 cmd/progress.ts 读取进度。
 * 两者共享同一实例以保证数据一致。
 */
export const animeProcessor = new AnimeProcessorManager(3);
