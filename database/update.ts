import { formatSubGroupName } from "../function/index.ts";
import type {
  anime as AnimeType,
} from "../types/anime.d.ts";
import type { albumMessageType, messageType, navMessageType } from "../types/message.d.ts";
import type { bangumiAnime, BangumiUser } from "../types/bangumi.d.ts";
import type { EpisodeMetaDoc } from "../types/episodeMeta.d.ts";
import type { EpisodeResourceDoc } from "../types/episodeResource.d.ts";
import type { PendingReviewDoc } from "../types/pendingReview.d.ts";
import { cleanTitle } from "../anime/rss/index.ts";
import logger from "@log/index.ts";
import { getDatabase } from "@db/index.ts";
import { getErrorMessage } from "@utils/error.ts";

const db = await getDatabase();

/**
 * 将单条资源写入主 resources 集合（upsert）。
 * @param payload - 资源数据
 */
async function upsertMainResource(payload: EpisodeResourceDoc & {
  videoids?: string[];
  unique_ids?: string[];
  updatedAt?: Date;
}): Promise<boolean> {
  const resources = db.collection("resources");

  const filter: Record<string, unknown> = {
    anime_id: payload.anime_id,
    episode: payload.episode,
    groups: payload.groups,
  };

  if (payload.episodeId !== undefined) {
    filter.episodeId = payload.episodeId;
  }

  const now = new Date();
  const { createdAt, ...setPayload } = payload;
  const result = await resources.updateOne(
    filter,
    {
      $set: {
        ...setPayload,
        updatedAt: now,
      },
      $setOnInsert: {
        createdAt: createdAt ?? now,
      },
    },
    { upsert: true }
  );

  return result.modifiedCount > 0 || result.upsertedCount > 0;
}

/**
 * 将单条资源写入缓存 resources 集合（upsert）。
 * @param payload - 缓存资源数据
 */
async function upsertCacheResource(payload: EpisodeResourceDoc & {
  videoids?: string[];
  unique_ids?: string[];
  updatedAt?: Date;
}): Promise<boolean> {
  const resources = db.collection("cache_resources");
  const now = new Date();
  const { createdAt, ...setPayload } = payload;

  const cacheIdKey = payload.cache_id ?? `${payload.anime_id}-${payload.episode}-${payload.title}`;

  const result = await resources.updateOne(
    {
      anime_id: payload.anime_id,
      cache_id: cacheIdKey,
      groups: payload.groups,
      episode: payload.episode,
    },
    {
      $set: {
        ...setPayload,
        updatedAt: now,
      },
      $setOnInsert: {
        createdAt: createdAt ?? now,
      },
    },
    { upsert: true }
  );

  return result.modifiedCount > 0 || result.upsertedCount > 0;
}

/**
 * 更新种子状态
 * @param title - 种子标题（用于匹配文档）
 * @param newStatus - 新的状态（等待元数据、下载中、下载完成、上传中、完成、失败）
 * @returns 更新成功返回true，否则返回false
 * @throws 当参数无效时抛出异常
 */
export async function updateTorrentStatus(
  title: string,
  newStatus:
    | "等待下载"
    | "等待元数据"
    | "下载中"
    | "下载完成"
    | "等待纠正"
    | "上传中"
    | "完成"
    | "失败"
) {
  if (!title || typeof title !== "string") {
    throw new Error("标题无效");
  }

  const validStatuses = [
    "等待下载",
    "等待元数据",
    "下载中",
    "下载完成",
    "等待纠正",
    "上传中",
    "完成",
    "失败",
    "等待纠正",
  ];
  if (!validStatuses.includes(newStatus)) {
    throw new Error("无效的状态");
  }

  const result = await db.collection("torrents").updateOne(
    { title: cleanTitle(title) },
    {
      $set: {
        status: newStatus,
        updatedAt: new Date(),
      },
    }
  );

  return result.modifiedCount > 0;
}

/**
 * 更新动漫评分
 * @param animeId - 动漫的id字段值
 * @param score - 新的评分（通常为0-10的数字或数字字符串）
 * @returns 更新成功返回true，否则返回false
 * @throws 当参数无效或数据库操作失败时抛出异常
 */
export async function updateAnimeScore(
  animeId: number,
  score: number | string
) {
  if (!animeId) {
    throw new Error("动漫ID是必需的参数");
  }

  if (score === null || score === undefined) {
    throw new Error("评分不能为空");
  }

  // 将字符串转换为数值
  const numericScore =
    typeof score === "string" ? parseFloat(score.trim()) : Number(score);

  // 验证转换后的数值是否有效
  if (isNaN(numericScore)) {
    throw new Error("评分必须是有效的数字或数字字符串");
  }

  // 验证评分范围（通常动漫评分在0-10之间）
  if (numericScore < 0 || numericScore > 10) {
    throw new Error("评分必须在0到10之间");
  }

  try {
    // 首先查找动漫文档
    const anime = await db.collection("anime").findOne({ id: animeId });

    if (!anime) {
      throw new Error(`未找到ID为 ${animeId} 的动漫`);
    }

    // 更新评分
    const result = await db.collection("anime").updateOne(
      { id: animeId },
      {
        $set: {
          score: numericScore,
          updatedAt: new Date(),
        },
      }
    );

    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新动漫评分失败: ${getErrorMessage(error)}`
    );
  }
}

/**
 * 保存动漫资源数据到主资源库或缓存资源库。
 * 当 `cache=false` 写入 `resources`，当 `cache=true` 写入 `cache_resources`。
 * @param animeId - 动漫的id字段值
 * @param episodeId - 动漫集数id
 * @param subGroup - 字幕组名称
 * @param episode - 集数
 * @param Message - TG消息详细
 * @param title - 集数标题（可选）
 * @param source - 视频来源（如：Baha, bilibili等，用于ANi和黒ネズミたち字幕组区分）
 * @param names - 该集数的别名数组（可选）
 * @param videoid - 视频ID（可选）
 * @param unique_id - 唯一ID（可选）
 * @param cacheItemId - 如果是缓存数据则传入对应的 cacheItem.id
 * @param cache - 是否操作缓存资源集合，默认为 false 操作主资源集合
 * @returns 更新成功返回true，否则返回false
 * @throws 当参数无效或数据库操作失败时抛出异常
 */
export async function saveAnimeResource(
  animeId: number,
  episodeId: number | undefined,
  subGroup: string,
  episode: string,
  Message: messageType,
  title: string,
  source: string | undefined,
  names: string[] | [],
  videoid: string | undefined,
  unique_id: string | undefined,
  cacheItemId: number | undefined = undefined,
  cache: boolean = false,
  videoids: string[] | undefined = undefined,
  unique_ids: string[] | undefined = undefined,
  Messages: albumMessageType[] | undefined = undefined
) {
  if (
    !animeId ||
    !subGroup ||
    !episode ||
    !Message ||
    (cache && !cacheItemId)
  ) {
    throw new Error(
      `动漫ID、字幕组、集数和Message是必需的参数${cache ? ", 当 cache 为 true 时，cacheItemId 也是必需的" : ""
      }`
    );
  }

  // 格式化字幕组名称（处理ANi和黒ネズミたち的source区分）
  const formattedSubGroup = formatSubGroupName(subGroup, source);

  try {
    if (!cache) {
      const anime = await db.collection<AnimeType>("anime").findOne({ id: animeId });
      if (!anime) {
        throw new Error(`未找到ID为 ${animeId} 的动漫`);
      }

      const saved = await upsertMainResource({
        anime_id: animeId,
        episode,
        episodeId,
        groups: [formattedSubGroup],
        title,
        names,
        message: Message,
        messages: Messages,
        videoid,
        unique_id,
        cache_id: cacheItemId,
        source,
        createdAt: new Date(),
        videoids,
        unique_ids,
      });

      if (saved) {
        await db.collection("anime").updateOne(
          { id: animeId },
          {
            $set: {
              updatedAt: new Date(),
            },
          }
        );
      }

      return saved;
    }

    const anime = await db.collection<AnimeType>("cacheAnime").findOne({ id: animeId });

    if (!anime) {
      throw new Error(`未找到ID为 ${animeId} 的动漫`);
    }

    const saved = await upsertCacheResource({
      anime_id: animeId,
      episode,
      episodeId,
      groups: [formattedSubGroup],
      title,
      names,
      message: Message,
      messages: Messages,
      videoid,
      unique_id,
      cache_id: cacheItemId,
      source,
      createdAt: new Date(),
      videoids,
      unique_ids,
    });

    if (saved) {
      await db.collection("cacheAnime").updateOne(
        { id: animeId },
        {
          $set: {
            updatedAt: new Date(),
          },
          $unset: {
            btdata: "",
          },
        }
      );
    }

    return saved;
  } catch (error) {
    throw new Error(
      `更新TGMegLink失败: ${getErrorMessage(error)}`
    );
  }
}

/**
 * 为指定动漫添加别名（names 字段），支持单个字符串或字符串数组，自动去重
 * @param animeId - 动漫的id字段值
 * @param nameToAdd - 要添加的别名（单个或数组）
 * @returns 添加成功返回true，否则返回false
 * @throws 当参数无效或数据库操作失败时抛出异常
 */
export async function addAnimeNameAlias(
  animeId: number,
  nameToAdd: string | string[]
) {
  if (!animeId || !nameToAdd) {
    throw new Error("动漫ID和别名都是必需的参数");
  }

  // 规范化为数组并去除空字符串
  let namesArr;
  if (Array.isArray(nameToAdd)) {
    namesArr = nameToAdd
      .map((n) => (typeof n === "string" ? n.trim() : String(n)))
      .filter(Boolean);
    if (namesArr.length === 0) throw new Error("别名数组不能为空");
  } else if (typeof nameToAdd === "string") {
    namesArr = [nameToAdd.trim()];
    if (!namesArr[0]) throw new Error("别名不能为空");
  } else {
    throw new Error("别名必须为字符串或字符串数组");
  }

  try {
    // 先查出当前 names
    const anime = await db
      .collection("anime")
      .findOne({ id: Number(animeId) }, { projection: { names: 1 } });
    const currentNames = Array.isArray(anime?.names) ? anime.names : [];
    // 合并去重
    const merged = Array.from(new Set([...currentNames, ...namesArr]));
    // 更新
    const result = await db.collection("anime").updateOne(
      { id: Number(animeId) },
      {
        $set: { names: merged, updatedAt: new Date() },
      }
    );
    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `添加别名失败: ${getErrorMessage(error)}`
    );
  }
}

/** 更新动漫的 r18 字段
 * @param animeId - 动漫的id字段值
 * @param r18 - 新的 r18 值（true 或 false）
 * @return 如果更新成功返回 true，否则返回 false
 * @throws 当参数无效或数据库操作失败时抛出异常
 */
export async function updateAnimeR18(animeId: number, r18: boolean) {
  if (
    !animeId ||
    typeof Number(animeId) !== "number" ||
    Number.isNaN(Number(animeId))
  ) {
    throw new Error("动漫ID是必需且必须为有效数字");
  }
  if (r18 === null || r18 === undefined || typeof r18 !== "boolean") {
    throw new Error("r18 参数必须为布尔值");
  }

  try {
    const anime = await db.collection("anime").findOne({ id: Number(animeId) });
    if (!anime) {
      throw new Error(`未找到ID为 ${animeId} 的动漫`);
    }

    const result = await db.collection("anime").updateOne(
      { id: Number(animeId) },
      {
        $set: { r18, updatedAt: new Date() },
      }
    );
    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新动漫R18字段失败: ${getErrorMessage(error)}`
    );
  }
}

/**
 * 更新动漫的导航频道主消息。
 * @param animeId - 动漫的id字段值
 * @param Message - 导航频道消息对象
 * @returns 更新成功返回true，否则返回false
 * @throws 当参数无效或数据库操作失败时抛出异常
 */
export async function updateAnimeNavMessage(
  animeId: number,
  Message: messageType
) {
  if (!animeId || !Message) {
    throw new Error("动漫ID和导航频道消息都是必需的参数");
  }

  try {
    // 首先查找动漫文档
    const anime = await db.collection("anime").findOne({ id: animeId });

    if (!anime) {
      throw new Error(`未找到ID为 ${animeId} 的动漫`);
    }

    // 更新导航频道消息
    const result = await db.collection("anime").updateOne(
      { id: animeId },
      {
        $set: {
          navMessage: Message,
          updatedAt: new Date(),
        },
      }
    );

    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新导航频道消息失败: ${getErrorMessage(error)}`
    );
  }
}

/**
 * 更新动漫导航封面图片的哈希值（用于去重判断）。
 * @param animeId - 动漫的id字段值
 * @param hash - 导航封面图片内容的哈希值
 * @returns 更新成功返回true，否则返回false
 */
export async function updateAnimeNavImageHash(
  animeId: number,
  hash: string
): Promise<boolean> {
  if (!animeId || !hash) {
    throw new Error("动漫ID和哈希值都是必需的参数");
  }

  try {
    const result = await db.collection("anime").updateOne(
      { id: animeId },
      {
        $set: {
          navImageHash: hash,
          updatedAt: new Date(),
        },
      }
    );
    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新导航封面哈希值失败: ${getErrorMessage(error)}`
    );
  }
}

export async function updateAnimeNavVideoMessage(
  animeId: number,
  videoMessage: navMessageType[] | messageType
) {
  if (!animeId || !videoMessage) {
    throw new Error("动漫ID和视频消息都是必需的参数");
  }

  try {
    const collection = db.collection<AnimeType>("anime");
    const anime = await collection.findOne({ id: animeId });

    if (!anime) {
      throw new Error(`未找到ID为 ${animeId} 的动漫`);
    }

    // 如果历史数据是单个对象而非数组，先迁移为数组
    if (anime.navVideoMessage && !Array.isArray(anime.navVideoMessage)) {
      await collection.updateOne(
        { id: animeId },
        {
          $set: {
            navVideoMessage: [anime.navVideoMessage],
          },
        }
      );
    } else if (!anime.navVideoMessage) {
      // 确保字段存在为数组
      await collection.updateOne(
        { id: animeId },
        {
          $set: { navVideoMessage: [] },
        }
      );
    }

    const messages = Array.isArray(videoMessage)
      ? videoMessage
      : [videoMessage];
    if (messages.length === 0) return false;

    const result = await collection.updateOne(
      { id: animeId },
      {
        $addToSet: { navVideoMessage: { $each: messages } },
        $set: { updatedAt: new Date() },
      }
    );

    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新导航频道视频消息失败: ${getErrorMessage(error)}`
    );
  }
}

/**
 * 更新动漫的信息
 * @param animeId - 动漫的id字段值
 * @param animeInfo - 动漫信息
 * @returns 更新成功返回true，否则返回false
 * @throws 当参数无效或数据库操作失败时抛出异常
 */
export async function updateAnimeInfo(
  animeId: number,
  animeInfo: Partial<bangumiAnime>
) {
  if (!animeId || !animeInfo) {
    throw new Error("动漫ID和动漫信息都是必需的参数");
  }

  try {
    // 首先查找动漫文档
    const anime = await db.collection("anime").findOne({ id: animeId });

    if (!anime) {
      throw new Error(`未找到ID为 ${animeId} 的动漫`);
    }

    const newScore = animeInfo.rating?.score ?? undefined;

    const result = await db.collection("anime").updateOne(
      { id: animeId },
      {
        $set: {
          name_cn: animeInfo.name_cn ?? anime.name_cn,
          summary: animeInfo.summary ?? anime.summary,
          // 仅当 API 返回了有效评分时才覆盖（避免用 undefined 覆盖已有值）
          ...(newScore !== undefined ? { score: newScore } : {}),
          updatedAt: new Date(),
        },
      }
    );

    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新动漫基础信息失败: ${getErrorMessage(error)}`
    );
  }
}

/**
 * 更新动漫的集数信息（eps 字段）
 * @param animeId - 动漫的 id 字段值
 * @param EpisodeInfo - 包含 total 与 data 数组的集数信息
 * @returns 更新成功返回 true，否则返回 false
 */
export async function updateAnimeEpisodes(
  animeId: number,
  EpisodeInfo: {
    total: number;
    data: {
      airdate: string;
      name: string;
      name_cn: string;
      duration: string;
      desc: string;
      ep: number;
      sort: number;
      id: number;
      subject_id: number;
      comment: number;
      type: number;
      disc: number;
      duration_seconds: number;
    }[];
  }
) {
  if (!animeId) {
    throw new Error("animeId 是必需的参数");
  }

  if (
    typeof EpisodeInfo?.total !== "number" ||
    !Array.isArray(EpisodeInfo?.data)
  ) {
    throw new Error("EpisodeInfo 必须包含 total:number 和 data: any[]");
  }

  try {
    const animeCollection = db.collection<AnimeType>("anime");
    const anime = await animeCollection.findOne({ id: animeId });
    if (!anime) {
      throw new Error(`未找到ID为 ${animeId} 的动漫`);
    }

    const list: EpisodeMetaDoc[] = EpisodeInfo.data.map((ep: any) => ({
      airdate: ep.airdate,
      name: ep.name,
      name_cn: ep.name_cn,
      duration: ep.duration,
      desc: ep.desc,
      ep: ep.ep,
      sort: ep.sort,
      id: ep.id,
      type: ep.type,
      subject_id: ep.subject_id,
      comment: ep.comment,
    }));

    await db.collection<EpisodeMetaDoc>("episodes_meta").deleteMany({
      subject_id: animeId,
    });

    if (list.length > 0) {
      await db.collection<EpisodeMetaDoc>("episodes_meta").insertMany(list, {
        ordered: false,
      });
    }

    const result = await animeCollection.updateOne(
      { id: animeId },
      {
        $set: {
          // 同步更新 episode 字符串字段（导航消息「本季话数」的数据来源）
          episode: String(EpisodeInfo.total || 0),
          updatedAt: new Date(),
        },
        $unset: {
          eps: "",
        },
      }
    );

    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新动漫集数信息失败: ${getErrorMessage(error)}`
    );
  }
}

/**
 * 更新 Bangumi 用户信息
 * @param id - 用户 ID
 * @param data - 要更新的数据
 * @returns 是否更新成功
 */
export async function updateBangumiUser(
  id: number,
  data: Partial<BangumiUser>
): Promise<boolean> {
  try {
    const result = await db.collection("bangumi_users").updateOne(
      { id },
      { $set: { ...data, updatedAt: new Date() } }
    );
    return result.modifiedCount > 0;
  } catch (err) {
    logger.error(err, "updateBangumiUser Error:");
    throw err;
  }
}

/**
 * 将某个 cache_id 对应的缓存资源从旧缓存动漫迁移到新动漫。
 * 迁移后若旧缓存动漫已无任何资源，会自动删除旧缓存动漫文档。
 * @param fromAnimeId - 原缓存动漫 id
 * @param toAnimeId - 目标动漫 id
 * @param cacheId - 缓存条目 id（cacheItem.id）
 */
export async function rebindCacheResourceAnime(
  fromAnimeId: number,
  toAnimeId: number,
  cacheId: string | number
): Promise<{ moved: number; removedOldCacheAnime: boolean }> {
  if (!fromAnimeId || !toAnimeId || cacheId === undefined || cacheId === null) {
    throw new Error("fromAnimeId、toAnimeId、cacheId 都是必需参数");
  }

  const targetStr = String(cacheId);
  const cacheResources = db.collection("cache_resources");

  const moveRes = await cacheResources.updateMany(
    {
      anime_id: fromAnimeId,
      $or: [
        { cache_id: cacheId },
        { cache_id: targetStr },
        ...(Number.isNaN(Number(targetStr)) ? [] : [{ cache_id: Number(targetStr) }]),
      ],
    },
    {
      $set: {
        anime_id: toAnimeId,
        updatedAt: new Date(),
      },
    }
  );

  let removedOldCacheAnime = false;
  const remaining = await cacheResources.countDocuments({ anime_id: fromAnimeId });
  if (remaining === 0) {
    const delOld = await db.collection("cacheAnime").deleteOne({ id: fromAnimeId });
    removedOldCacheAnime = delOld.deletedCount > 0;
  }

  return {
    moved: moveRes.modifiedCount ?? 0,
    removedOldCacheAnime,
  };
}

/**
 * 更新待审核番剧记录的状态（批准/拒绝）。
 * @param id - 待审核记录的自增 id
 * @param status - 目标状态："approved" | "rejected"
 * @returns 更新成功返回 true，否则返回 false
 */
export async function updatePendingReviewStatus(
  id: number,
  status: Extract<PendingReviewDoc["status"], "approved" | "rejected">
): Promise<boolean> {
  if (!id) {
    throw new Error("待审核ID是必需的参数");
  }
  try {
    const result = await db
      .collection<PendingReviewDoc>("pendingReviews")
      .updateOne({ id }, { $set: { status, updatedAt: new Date() } });
    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(`更新待审核记录状态失败: ${getErrorMessage(error)}`);
  }
}
