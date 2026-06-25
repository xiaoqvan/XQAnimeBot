import logger from "@log/index.ts";
import type { anime as AnimeType } from "../types/anime.d.ts";
import type { EpisodeMetaDoc } from "../types/episodeMeta.d.ts";
import type { animeItem } from "../types/rss.d.ts";
import type { BangumiUser } from "../types/bangumi.d.ts";
import type { PendingReviewDoc } from "../types/pendingReview.d.ts";

import { cleanTitle } from "../anime/rss/index.ts";
import { getDatabase } from "@db/index.ts";
import { getErrorMessage } from "@utils/error.ts";

const db = await getDatabase();

type AnimeWithRelations = AnimeType & {
  eps?: {
    total?: number;
    list?: EpisodeMetaDoc[];
  };
  btdata?: Record<string, unknown>;
};

type CacheAnimeDoc = AnimeWithRelations & {
  btdata?: never;
};

/**
 * 添加一个缓存条目
 * @param item - 要缓存的对象
 * @returns 插入的自增id
 */
export async function addCacheItem(item: animeItem) {
  try {
    const id = await getNextSequence("cacheItemId");

    const doc = {
      id,
      item: { ...item },
      createdAt: new Date(),
    };

    await db.collection("cacheItem").insertOne(doc);
    return id;
  } catch (err) {
    logger.error(err, "addCacheItem 出错:");
    throw err;
  }
}

interface SequenceConfig {
  type: string;
  seq: number;
}

/** 获取下一个自增序列值
 * @param name - 序列名称
 * @returns 下一个序列值
 */
async function getNextSequence(name: string): Promise<number> {
  try {
    // 使用 type 字段保存序列，_id 由 MongoDB 生成为 ObjectId。
    const result = await db
      .collection<SequenceConfig>("config")
      .findOneAndUpdate(
        { type: name },
        { $inc: { seq: 1 }, $setOnInsert: { type: name } },
        {
          upsert: true, // 文档不存在就创建
          returnDocument: "after", // 返回更新后的文档
        }
      );

    if (result) {
      return result.seq;
    }

    // 兜底查询，兼容旧格式或异常情况
    const doc = await db
      .collection<SequenceConfig>("config")
      .findOne({ type: name });

    if (doc?.seq !== undefined) {
      return doc.seq;
    }

    throw new Error(`无法获取序列 ${name} 的值`);
  } catch (err) {
    logger.error(err, "getNextSequence 出错:");
    throw err;
  }
}

interface TorrentData {
  title: string;
  magnetLink: string;
  status: string;
  createdAt: Date;
  updatedAt: Date;
}

/**
 * 添加种子信息到数据库
 * @param torrentId - 种子ID
 * @param magnetLink - 磁力链接
 * @param status - 种子状态（下载中、下载完成、上传中、完成）
 * @returns 插入的文档ID
 * @throws 当参数无效或数据库操作失败时抛出异常
 */
export async function addTorrent(
  magnetLink: string,
  status: string,
  title: string
) {
  if (!status || !title) {
    throw new Error("标题和状态都是必需的参数");
  }

  // 验证状态是否有效
  const validStatuses = [
    "等待下载",
    "等待元数据",
    "下载中",
    "下载完成",
    "上传中",
    "完成",
    "失败",
    "等待纠正",
  ];
  if (!validStatuses.includes(status)) {
    throw new Error(
      "无效的状态，有效状态：等待下载，等待元数据、下载中、下载完成、上传中、完成、失败、等待纠正"
    );
  }

  const torrentData: TorrentData = {
    title: cleanTitle(title),
    magnetLink,
    status,
    createdAt: new Date(),
    updatedAt: new Date(),
  };

  try {
    // 不要在这里重复创建索引
    // await db.collection("torrents").createIndex({ title: 1 }, { unique: true });

    // 使用 title 做为唯一标识，存在则更新，不存在则插入（upsert）
    const result = await db
      .collection<TorrentData>("torrents")
      .findOneAndUpdate(
        { title },
        {
          $set: {
            magnetLink: torrentData.magnetLink,
            status: torrentData.status,
            updatedAt: new Date(),
          },
          $setOnInsert: {
            createdAt: torrentData.createdAt,
            title: torrentData.title,
          },
        },
        { upsert: true, returnDocument: "after" }
      );

    // 返回文档的 _id（无论是新插入还是更新）
    if (result?._id) {
      return result._id;
    }

    // 万一上面没有返回文档，再做一次查询读取 _id
    const doc = await db.collection<TorrentData>("torrents").findOne({ title });
    if (!doc?._id) {
      throw new Error("无法获取插入文档的 ID");
    }
    return doc._id;
  } catch (error) {
    throw new Error(
      `保存或更新种子信息失败: ${getErrorMessage(error)}`
    );
  }
}

/**
 * 批量覆盖保存某个番剧的章节元数据。
 * @param subjectId - 番剧 ID（Bangumi subject_id）
 * @param episodes - 章节元数据列表
 */
export async function replaceEpisodeMetas(
  subjectId: number,
  episodes: EpisodeMetaDoc[]
): Promise<void> {
  if (!subjectId || !Array.isArray(episodes)) {
    throw new Error("replaceEpisodeMetas 参数无效");
  }

  const collection = db.collection<EpisodeMetaDoc>("episodes_meta");

  await collection.deleteMany({ subject_id: subjectId });

  if (episodes.length === 0) return;

  const normalized = episodes.map((episode) => ({
    ...episode,
    subject_id: subjectId,
  }));

  await collection.insertMany(normalized, { ordered: false });
}

/**
 * 保存或更新动漫信息
 * @param anime - 动漫对象，必须包含 id 字段
 * @param cache - 是否保存到缓存集合，默认为 false（保存到正式集合）
 */
export async function saveAnime(anime: AnimeWithRelations, cache: boolean = false) {
  if (!anime || typeof anime !== "object") {
    throw new Error("无效的动漫数据");
  }
  if (!anime.id) {
    throw new Error("动漫数据缺少 id");
  }

  // 选择集合
  const col = db.collection<AnimeWithRelations>(cache ? "cacheAnime" : "anime");
  // 确保 id 唯一索引
  await col.createIndex({ id: 1 }, { unique: true });

  if (!cache) {
    const now = new Date();
    const { _id: _mongoId, eps, btdata: _btdata, ...animeBase } = anime as AnimeWithRelations & {
      _id?: unknown;
    };

    const updatePayload: Partial<AnimeWithRelations> = {
      ...animeBase,
      updatedAt: now,
    };

    const oldDoc = await col.findOne({ id: anime.id });

    if (oldDoc) {
      await col.updateOne(
        { id: anime.id },
        {
          $set: updatePayload,
          $unset: {
            eps: "",
            btdata: "",
          },
        }
      );
    } else {
      await col.insertOne({
        ...(updatePayload as AnimeWithRelations),
        createdAt: now,
      });
    }

    if (Array.isArray(eps?.list)) {
      await replaceEpisodeMetas(anime.id, eps.list);
    }

    return anime.id;
  }

  const oldDoc = await col.findOne({ id: anime.id });

  const { _id: _cacheMongoId, btdata: _cacheBtdata, ...cacheAnimeBase } = anime as AnimeWithRelations & {
    _id?: unknown;
  };

  // 需要更新的字段（包含 names，确保 RSS 解析的名称也能同步到缓存）
  const updateFields: (keyof CacheAnimeDoc)[] = [
    "name",
    "name_cn",
    "names",
    "summary",
    "tags",
    "episode",
    "eps",
    "score",
    "airingDay",
    "airingStart",
  ];

  if (oldDoc) {
    // 只更新指定字段
    const update: Partial<AnimeWithRelations> = {};

    for (const key of updateFields) {
      setField(update, anime, key);
    }

    update.updatedAt = new Date();

    await col.updateOne(
      { id: anime.id },
      {
        $set: update,
        $unset: {
          btdata: "",
        },
      }
    );
    return anime.id;
  } else {
    // 新建
    const doc: CacheAnimeDoc = {
      ...cacheAnimeBase,
      createdAt: new Date(),
      updatedAt: new Date(),
    };
    await col.insertOne(doc);
    return anime.id;
  }
}

/** 设置对象字段值（如果源对象中该字段已定义）
 * @param target - 目标对象
 * @param source - 源对象
 * @param key - 要设置的字段键
 */
function setField<K extends keyof AnimeWithRelations>(
  target: Partial<AnimeWithRelations>,
  source: AnimeWithRelations,
  key: K
) {
  if (source[key] !== undefined) {
    target[key] = source[key];
  }
}

/**
 * 创建一个新的 Bangumi 用户记录
 * @param data - 用户初始数据
 * @returns 新用户的自增 ID
 */
export async function createBangumiUser(
  data: { tgUserId: number | string }
): Promise<number> {
  try {
    const col = db.collection("bangumi_users");

    await Promise.all([
      col.createIndex({ id: 1 }, { unique: true }).catch(() => { }),
      col.createIndex({ tgUserId: 1 }, { unique: true, sparse: true }).catch(() => { }),
    ]);

    const { tgUserId } = data;

    if (tgUserId === undefined || tgUserId === null) {
      throw new Error("createBangumiUser: tgUserId 是必需的参数");
    }

    const existing = await col.findOne({ tgUserId });
    if (existing && existing.id !== undefined) {
      return existing.id;
    }

    const id = await getNextSequence("bangumi_user_id");

    const now = new Date();
    const doc = {
      id,
      ...data,
      createdAt: now,
      updatedAt: now,
    } as BangumiUser;

    try {
      await col.insertOne(doc);
      return id;
    } catch (err) {
      const conflict = await col.findOne({ tgUserId });
      if (conflict && conflict.id !== undefined) return conflict.id;
      throw err;
    }
  } catch (err) {
    logger.error(err, "createBangumiUser 出错:");
    throw err;
  }
}

/**
 * 创建一条待审核记录
 *
 * AI 匹配完成并发送视频后，将审核所需的所有数据保存到此记录中。
 * 回调按钮只携带此记录的 id，审核时从中读取全部上下文。
 *
 * @param data - 待审核数据
 * @returns 自增的待审核 ID
 */
export async function createPendingReview(
  data: Omit<PendingReviewDoc, "id" | "createdAt" | "status">
): Promise<number> {
  try {
    const id = await getNextSequence("pendingReviewId");

    const doc: PendingReviewDoc = {
      id,
      status: "pending",
      createdAt: new Date(),
      ...data,
    };

    await db.collection<PendingReviewDoc>("pendingReviews").insertOne(doc);
    return id;
  } catch (err) {
    logger.error(err, "createPendingReview 出错:");
    throw err;
  }
}
