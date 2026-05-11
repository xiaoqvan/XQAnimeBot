import type {
  AnimeBlacklistConfig,
  TagExcludeListConfig,
} from "../types/database.d.ts";
import type { anime as animeType } from "../types/anime.d.ts";
import type { animeItem } from "../types/rss.d.ts";
import type { BangumiUser } from "../types/bangumi.d.ts";
import type { EpisodeMetaDoc } from "../types/episodeMeta.d.ts";
import type { EpisodeResourceDoc } from "../types/episodeResource.d.ts";
import { getDatabase } from "@db/index.ts";

const db = await getDatabase();

type AnimeWithRelations = animeType & {
  eps?: {
    total: number;
    list: EpisodeMetaDoc[];
  };
  btdata?: Record<string, any[]>;
};

/**
 * 从 resources 集合构建旧逻辑需要的 btdata 结构。
 * @param animeId - 番剧 ID
 */
export async function getBtDataByAnimeId(
  animeId: number,
  collectionName: "resources" | "cache_resources" = "resources"
): Promise<Record<string, any[]>> {
  const resources = await db
    .collection<EpisodeResourceDoc & { updatedAt?: Date }>(collectionName)
    .find({ anime_id: animeId })
    .sort({ createdAt: 1 })
    .toArray();

  const grouped: Record<string, any[]> = {};

  for (const resource of resources) {
    const groupName =
      Array.isArray(resource.groups) && resource.groups.length > 0
        ? resource.groups[0]
        : "unknown";

    if (!grouped[groupName]) grouped[groupName] = [];

    grouped[groupName].push({
      episode: resource.episode,
      episodeId: resource.episodeId,
      title: resource.title,
      names: resource.names,
      Message: resource.message,
      Messages: resource.messages,
      TGMegLink: resource.message?.link,
      videoid: resource.videoid,
      unique_id: resource.unique_id,
      cache_id: resource.cache_id,
      source: resource.source,
      videoids: (resource as any).videoids,
      unique_ids: (resource as any).unique_ids,
    });
  }

  return grouped;
}

/**
 * 从 episodes_meta 集合构建 eps 字段。
 * @param animeId - 番剧 ID
 */
async function buildEpisodesByAnimeId(animeId: number): Promise<{
  total: number;
  list: EpisodeMetaDoc[];
}> {
  const list = await getEpisodeMetasBySubjectId(animeId);

  return {
    total: list.length,
    list,
  };
}

/**
 * 根据番剧 ID 查询章节元数据，并按 sort/id 升序返回。
 * @param animeId - 番剧 ID（Bangumi subject_id）
 */
export async function getEpisodeMetasBySubjectId(
  animeId: number
): Promise<EpisodeMetaDoc[]> {
  if (!animeId) {
    throw new Error("动漫ID是必需的参数");
  }

  return db
    .collection<EpisodeMetaDoc>("episodes_meta")
    .find({ subject_id: animeId })
    .sort({ sort: 1, id: 1 })
    .toArray();
}

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
  const anime = await db
    .collection<animeType>("anime")
    .find({
      $or: [{ name: { $in: names } }, { names: { $in: names } }],
    })
    .collation({
      locale: "en",
      strength: 2,
    })
    .sort({
      updatedAt: -1,
    })
    .limit(1)
    .next();

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
): Promise<AnimeWithRelations | null> {
  if (!animeId) {
    throw new Error("动漫ID是必需的参数");
  }

  try {
    const anime = await db
      .collection<animeType>(cache ? "cacheAnime" : "anime")
      .findOne({ id: animeId });

    if (!anime) return null;
    if (cache) {
      const btdata = await getBtDataByAnimeId(animeId, "cache_resources");
      return {
        ...(anime as AnimeWithRelations),
        btdata,
      };
    }

    const [eps, btdata] = await Promise.all([
      buildEpisodesByAnimeId(animeId),
      getBtDataByAnimeId(animeId),
    ]);

    return {
      ...(anime as AnimeWithRelations),
      eps,
      btdata,
    };
  } catch (error) {
    throw new Error(
      `查询动漫信息失败: ${error instanceof Error ? error.message : error}`
    );
  }
}

/**
 * 按缓存ID查询某个缓存番剧下的资源条目。
 * @param animeId - 缓存番剧 ID（cacheAnime.id）
 * @param cacheId - 缓存条目 ID（cacheItem.id）
 * @param title - 可选标题，用于兜底匹配
 * @returns 命中时返回资源分组及资源条目，否则返回 null
 */
export async function getCacheResourceByCacheId(
  animeId: number,
  cacheId: string | number,
  title?: string
): Promise<{ group: string; episode: any } | null> {
  if (!animeId || cacheId === undefined || cacheId === null) {
    throw new Error("animeId 和 cacheId 是必需的参数");
  }

  const targetStr = String(cacheId);
  const resources = db.collection<EpisodeResourceDoc & { videoids?: string[]; unique_ids?: string[] }>("cache_resources");

  const list = await resources.find({ anime_id: animeId }).toArray();

  const byCacheId = list.find((item) => String(item.cache_id) === targetStr);
  let target = byCacheId ?? (
    title
      ? list.find((item) => {
        const candidates = [item.title, item.videoid, item.unique_id, item.episode]
          .filter((x): x is string => typeof x === "string" && x.length > 0)
          .map((x) => x.toLowerCase());
        return candidates.includes(String(title).toLowerCase());
      })
      : undefined
  );

  // 纠正场景下 animeId 可能已切换为新番剧，按 cache_id 全局兜底匹配原缓存资源。
  if (!target) {
    const globalCandidates = await resources.find({
      $or: [
        { cache_id: cacheId },
        { cache_id: targetStr },
        ...(Number.isNaN(Number(targetStr)) ? [] : [{ cache_id: Number(targetStr) }]),
      ],
    }).toArray();

    target =
      globalCandidates.find((item) => String(item.cache_id) === targetStr) ??
      (title
        ? globalCandidates.find((item) => {
          const candidates = [item.title, item.videoid, item.unique_id, item.episode]
            .filter((x): x is string => typeof x === "string" && x.length > 0)
            .map((x) => x.toLowerCase());
          return candidates.includes(String(title).toLowerCase());
        })
        : undefined);
  }

  if (!target) return null;

  const group =
    Array.isArray(target.groups) && target.groups.length > 0
      ? target.groups[0]
      : "unknown";

  return {
    group,
    episode: {
      episode: target.episode,
      episodeId: target.episodeId,
      title: target.title,
      names: target.names,
      Message: target.message,
      Messages: target.messages,
      TGMegLink: target.message?.link,
      videoid: target.videoid,
      unique_id: target.unique_id,
      cache_id: target.cache_id,
      source: target.source,
      cache_anime_id: target.anime_id,
      videoids: target.videoids,
      unique_ids: target.unique_ids,
    },
  };
}

/**
 * 根据章节（episode）ID 查询所属的动漫条目
 * @param episodeId - Bangumi 章节 ID
 */
export async function getAnimeByEpisodeId(
  episodeId: number
): Promise<AnimeWithRelations | null> {
  if (!episodeId) throw new Error("章节 id 是必需的参数");
  try {
    const episodeMeta = await db
      .collection<EpisodeMetaDoc>("episodes_meta")
      .findOne({ id: episodeId });

    if (!episodeMeta?.subject_id) return null;

    return getAnimeById(episodeMeta.subject_id, false);
  } catch (error) {
    throw new Error(`查询章节所属动漫失败: ${error instanceof Error ? error.message : error}`);
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
 * 转义正则表达式特殊字符
 * @param str - 要转义的字符串
 * @returns 转义后的字符串
 */
function escapeRegExp(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * 搜索动漫信息
 * @param keyword - 搜索关键词
 * @returns 搜索结果数组，包含匹配的动漫信息，限制返回前20条
 * @throws 数据库查询错误时抛出异常
 */
export async function searchAnime(key: string): Promise<animeType[]> {
  if (!key) {
    throw new Error("搜索查询是必需的参数");
  }

  try {
    // 按关键词模糊搜索
    const keyword = String(key).trim();

    if (keyword.length < 2) {
      throw new Error("关键词搜索至少需要2个字符");
    }

    // 转义正则特殊字符，防止正则注入
    const escapedKeyword = escapeRegExp(keyword);

    // 创建正则表达式进行模糊匹配
    const regex = new RegExp(escapedKeyword, "i"); // 'i' 表示不区分大小写

    // 先搜索 name 和 name_cn
    let animes = await db
      .collection<animeType>("anime")
      .find(
        {
          $or: [
            { name: { $regex: regex } },
            { name_cn: { $regex: regex } },
          ],
        },
      )
      .limit(20) // 限制返回前20条
      .toArray();

    // 如果在 name 和 name_cn 中没找到，再搜索 names 数组
    if (animes.length === 0) {
      animes = await db
        .collection<animeType>("anime")
        .find(
          {
            names: { $regex: regex },
          },
        )
        .limit(20) // 限制返回前20条
        .toArray();
    }

    return animes;
  } catch (error) {
    throw new Error(
      `搜索动漫失败: ${error instanceof Error ? error.message : error}`
    );
  }
}

/**
 * 根据 Telegram User ID 查询 Bangumi 用户信息
 * @param tgUserId - Telegram 用户 ID
 * @returns 用户信息 或 null
 */
export async function getBangumiUserByTgId(
  tgUserId: number
): Promise<BangumiUser | null> {
  if (!tgUserId) throw new Error("Telegram 用户 ID 是必需的参数");

  const user = await db.collection<BangumiUser>("bangumi_users").findOne({
    tgUserId,
  });
  return user || null;
}

/**
 * 根据内部自增 id 查询 Bangumi 用户信息
 * @param id - 内部自增 id
 */
export async function getBangumiUserById(id: number): Promise<BangumiUser | null> {
  if (!id) throw new Error("id 是必需的参数");
  const user = await db.collection<BangumiUser>("bangumi_users").findOne({ id });
  return user || null;
}
