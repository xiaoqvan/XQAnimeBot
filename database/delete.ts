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

/**
 * 删除正式番剧及其关联数据（供 Web/命令调用）。
 *
 * 会联动删除：
 * - `anime` 文档（含内嵌 navMessage / navVideoMessage）
 * - `episodes_meta`（按 subject_id）
 * - `resources`（按 anime_id）
 * - `cache_resources`（按 anime_id）与 `cacheAnime`（按 id）
 * - `pendingReviews`（其中 anime.id === animeId）
 *
 * 注意：不删除 `torrents` 集合（该集合用于 BT 标题去重，删除会导致重复下载）。
 *
 * @param animeId - 番剧 id（Bangumi subject_id）
 * @returns 是否删除了 anime 文档（若本来就是 deleteMany 返回 deletedCount）
 */
export async function deleteAnime(animeId: number): Promise<boolean> {
  if (!animeId) {
    throw new Error("番剧 ID 是必需的参数");
  }

  try {
    const [delAnime] = await Promise.all([
      db.collection("anime").deleteOne({ id: animeId }),
      db.collection("episodes_meta").deleteMany({ subject_id: animeId }),
      db.collection("resources").deleteMany({ anime_id: animeId }),
      db.collection("cache_resources").deleteMany({ anime_id: animeId }),
      db.collection("cacheAnime").deleteMany({ id: animeId }),
      db.collection("pendingReviews").deleteMany({ "anime.id": animeId }),
    ]);

    if (delAnime.deletedCount === 0) {
      return false;
    }
    logger.info(`已删除番剧 ${animeId} 及其关联资源`);
    return true;
  } catch (error: any) {
    logger.error(`删除番剧失败: ${error?.message ?? error}`);
    throw new Error(`删除番剧失败: ${error?.message ?? error}`);
  }
}
