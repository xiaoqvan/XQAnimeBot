import logger from "@log/index.ts";
import { getDatabase } from "@db/index.ts";
import type { PendingReviewDoc } from "../types/pendingReview.d.ts";

const db = await getDatabase();

/**
 * 删除缓存资源记录（根据 cacheAnime.id + cacheItem.id）。
 * 当该缓存番剧已无任何缓存资源时，同时删除 cacheAnime 文档。
 * @param cacheId - cacheAnime 的 id 字段值
 * @param cache_id - 缓存资源的 cache_id（通常是 cacheItem.id）
 * @returns 删除成功返回 true，否则返回 false
 * @throws 数据库操作失败时抛出异常
 */
export async function deleteCacheAnime(
  cacheId: number,
  cache_id: string | number
) {
  if (!cacheId || cache_id === undefined || cache_id === null) {
    throw new Error("需要提供 cacheId 和 cache_id（要删除的集的 cache_id）");
  }

  try {
    const cacheResources = db.collection("cache_resources");
    const targetIdStr = String(cache_id);

    const deleteRes = await cacheResources.deleteMany({
      anime_id: cacheId,
      $or: [
        { cache_id },
        { cache_id: targetIdStr },
        ...(Number.isNaN(Number(targetIdStr))
          ? []
          : [{ cache_id: Number(targetIdStr) }]),
      ],
    });

    if (deleteRes.deletedCount === 0) {
      logger.warn(
        `deleteCacheAnime: anime_id=${cacheId} 中未找到 cache_id=${targetIdStr}`
      );
      return false;
    }

    const remaining = await cacheResources.countDocuments({ anime_id: cacheId });
    if (remaining === 0) {
      await db.collection("cacheAnime").deleteOne({ id: cacheId });
    }

    return true;
  } catch (error: any) {
    logger.error(`删除 cacheAnime 失败: ${error?.message ?? error}`);
    throw new Error(`删除 cacheAnime 失败: ${error?.message ?? error}`);
  }
}

/**
 * 删除待审核记录（审核完成后清理）
 * @param id - 待审核记录的自增 ID
 * @returns 删除成功返回 true，否则返回 false
 */
export async function deletePendingReview(id: number): Promise<boolean> {
  if (!id) {
    throw new Error("待审核ID是必需的参数");
  }

  try {
    const result = await db
      .collection<PendingReviewDoc>("pendingReviews")
      .deleteOne({ id });
    return result.deletedCount > 0;
  } catch (error: any) {
    logger.error(`删除待审核记录失败: ${error?.message ?? error}`);
    throw new Error(`删除待审核记录失败: ${error?.message ?? error}`);
  }
}
