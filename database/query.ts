import { getDatabase } from "./initDb.ts";
import type {
  AnimeBlacklistConfig,
  TagExcludeListConfig,
} from "../types/database.ts";
import type { animeItem, anime as animeType } from "../types/anime.ts";

const db = await getDatabase();

/**
 * 获取动漫拉黑列表
 * @returns 拉黑列表数组 如果没有配置则返回undefined
 */
export async function getAnimeBlacklist(): Promise<string[] | undefined> {
  const doc = await db.collection<AnimeBlacklistConfig>("config").findOne(
    { key: "animeBlacklist" },
    { projection: { _id: 0, list: 1 } } // 只返回 list 字段，且不返回 _id
  );
  return doc?.list;
}

/**
 * 检查数据库中是否存在指定标题的种子
 * @param title - 种子标题
 * @returns 如果找到种子返回true，否则返回false
 * @throws 当标题为空或数据库查询失败时抛出异常
 */
export async function hasTorrentTitle(title: string): Promise<boolean> {
  if (!title) {
    throw new Error("种子标题是必需的参数");
  }

  const torrent = await db.collection("torrents").findOne({
    title: title,
  });

  return !!torrent; // 如果找到种子就返回true，否则false
}

/**
 * 检查是否已发送过指定名称的动漫
 * @param names - 动漫名称数组
 * @returns 如果找到动漫返回动漫信息
 * @throws 数据库查询错误时抛出异常
 */
export async function hasAnimeSend(names: string[]) {
  const anime = await db.collection<animeType>("anime").findOne({
    $or: [{ name: { $in: names } }, { names: { $in: names } }],
  });

  return anime;
}

/**
 * 获取标签排除列表
 * @returns 返回所有排除的标签关键词
 */
export async function getTagExcludeList() {
  const doc = await db
    .collection<TagExcludeListConfig>("config")
    .findOne({ key: "tagExcludeList" });
  return doc?.list || [];
}

/**
 * 根据动漫ID查询动漫信息
 * @param animeId - 动漫的id
 * @param cache - 是否查询缓存，默认为false
 * @returns 如果找到动漫返回动漫信息，否则返回undefined
 * @throws 数据库查询错误时抛出异常
 */
export async function getAnimeById(
  animeId: number,
  cache: boolean = false
): Promise<animeType | null> {
  if (!animeId) {
    throw new Error("动漫ID是必需的参数");
  }

  try {
    const anime = await db
      .collection<animeType>(cache ? "cacheAnime" : "anime")
      .findOne({ id: animeId });
    return anime;
  } catch (error) {
    throw new Error(
      `查询动漫信息失败: ${error instanceof Error ? error.message : error}`
    );
  }
}

/** 根据缓存ID查询缓存的动漫信息
 * @param id - 缓存ID
 * @returns 如果找到缓存返回动漫信息，否则返回undefined
 * @throws 数据库查询错误时抛出异常
 */
export async function getCacheItemById(id: number) {
  if (!id) {
    throw new Error("缓存ID是必需的参数");
  }
  try {
    const cacheItem = await db
      .collection<{ id: number; item: animeItem; createdAt: Date }>("cacheItem")
      .findOne({ id });
    return cacheItem?.item;
  } catch (error) {
    throw new Error(
      `查询缓存信息失败: ${error instanceof Error ? error.message : error}`
    );
  }
}

/**
 * 搜索动漫信息
 * @param keyword - 搜索关键词
 * @returns 搜索结果数组，包含匹配的动漫信息
 * @throws 数据库查询错误时抛出异常
 */
export async function searchAnime(key: string) {
  if (!key) {
    throw new Error("搜索查询是必需的参数");
  }

  try {
    let searchQuery;

    // 按关键词模糊搜索
    const keyword = String(key).trim();

    if (keyword.length < 2) {
      throw new Error("关键词搜索至少需要2个字符");
    }

    // 创建正则表达式进行模糊匹配
    const regex = new RegExp(keyword, "i"); // 'i' 表示不区分大小写

    searchQuery = {
      $or: [
        { name: { $regex: regex } },
        { name_cn: { $regex: regex } },
        { names: { $regex: regex } },
      ],
    };

    // 查询动漫，只返回需要的字段
    const animes = await db
      .collection("anime")
      .find(searchQuery, {
        projection: {
          id: 1,
          name: 1,
          name_cn: 1,
          names: 1, // 需要参与查询但不显示
          navMessageLink: 1, // 频道消息链接
        },
      })
      .toArray();

    return animes;
  } catch (error) {
    throw new Error(
      `搜索动漫失败: ${error instanceof Error ? error.message : error}`
    );
  }
}
