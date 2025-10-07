import logger from "@log/index.ts";
import type { anime } from "../types/anime.ts";
import { getDatabase } from "./initDb.ts";

const db = await getDatabase();

/**
 * 删除 cacheAnime 中的记录（根据 id）
 * @param cacheId - cacheAnime 的 id 字段值
 * @param cache_id - btdata 中要删除的 episode 的 cache_id
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
    const coll = db.collection<anime>("cacheAnime");
    const doc = await coll.findOne({ id: cacheId });
    if (!doc) {
      logger.warn(`deleteCacheAnime: 未找到 id=${cacheId} 的文档`);
      return false;
    }

    const btdata = (doc as any).btdata as Record<string, any[]> | undefined;
    if (!btdata || Object.keys(btdata).length === 0) {
      logger.warn(`deleteCacheAnime: id=${cacheId} 的 btdata 为空`);
      // 没有 btdata，则没有要删除的集，返回 false
      return false;
    }

    const targetIdStr = String(cache_id);
    let changed = false;
    const newBtdata: Record<string, any[]> = {};

    for (const key of Object.keys(btdata)) {
      const arr = btdata[key] ?? [];
      const filtered = arr.filter(
        (item) => String(item?.cache_id) !== targetIdStr
      );
      if (filtered.length !== arr.length) {
        changed = true; // 有删除发生
      }
      if (filtered.length > 0) {
        newBtdata[key] = filtered;
      }
      // 若 filtered.length === 0 则不将该 key 加入 newBtdata（等于删除该分组）
    }

    if (!changed) {
      logger.warn(
        `deleteCacheAnime: id=${cacheId} 中未找到 cache_id=${targetIdStr}`
      );
      return false;
    }

    // 如果所有分组都被删除，则删除整条文档
    if (Object.keys(newBtdata).length === 0) {
      const delRes = await coll.deleteOne({ id: cacheId });
      return delRes.deletedCount > 0;
    }

    // 否则更新 btdata 字段
    const updateRes = await coll.updateOne(
      { id: cacheId },
      { $set: { btdata: newBtdata } }
    );
    return updateRes.modifiedCount > 0;
  } catch (error: any) {
    logger.error(`删除 cacheAnime 失败: ${error?.message ?? error}`);
    throw new Error(`删除 cacheAnime 失败: ${error?.message ?? error}`);
  }
}
