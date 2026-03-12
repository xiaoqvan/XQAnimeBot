import logger from "@log/index.ts";
import { fetchMergedRss } from "./rss/index.ts";
import { smartDelayWithInterval } from "../utils/index.ts";
import { ErrorHandler } from "../utils/ErrorHandler.ts";
import { animeProcessor } from "./AnimeProcessorManager.ts";
import type { Client } from "tdl";

/**
 * 动漫 RSS 处理主循环（新版，使用 {@link AnimeProcessorManager} 并发管理）
 *
 * 与旧版 `processItemsWithConcurrency` 的区别：
 * - 本函数**不等待**当前批次全部处理完毕再取下一批，
 *   仅负责周期性抓取 RSS 并将有效条目加入队列。
 * - {@link animeProcessor} 维护固定数量的 worker 槽位，
 *   每个 worker 完成后**立即**从队列取下一个任务，
 *   不会因某一个任务卡住而阻塞其他 worker。
 *
 * @param client - TDLib 客户端实例
 */
export async function anime(client: Client): Promise<void> {
    while (true) {
        try {
            const rss = await fetchMergedRss();
            if (rss && Array.isArray(rss)) {
                const validItems = rss.filter(
                    (item) => item && item.title && item.pubDate && item.type
                );

                if (validItems.length > 0) {
                    logger.debug(
                        `获取到 ${validItems.length} 个 RSS 条目，加入处理队列` +
                        `（活跃: ${animeProcessor.getActiveCount()}, 排队: ${animeProcessor.getQueueSize()}）`
                    );
                    animeProcessor.enqueue(client, validItems);
                }
            }
        } catch (error) {
            logger.error("动漫处理主线程报错", error);
            ErrorHandler(client, error).catch(() => { });
        }

        await smartDelayWithInterval();
    }
}
