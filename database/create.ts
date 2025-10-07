import logger from "@log/index.ts";
import type { animeItem, anime as AnimeType, BtEntry } from "../types/anime.ts";

import { getDatabase } from "./initDb.ts";

const db = await getDatabase();

/**
 * 添加一个缓存条目
 * @param item - 要缓存的对象
 * @returns 插入的自增id 或 null
 */
export async function addCacheItem(item: animeItem) {
  try {
    const id = await getNextSequence("cacheItemId");
    if (id === null) return null;

    const doc = {
      id,
      item: { ...item },
      createdAt: new Date(),
    };

    await db.collection("cacheItem").insertOne(doc);
    return id;
  } catch (err) {
    logger.error("addCacheItem 出错:", err);
    return null;
  }
}

/** 获取下一个自增序列值
 * @param name - 序列名称
 * @returns 下一个序列值 或 null
 */
async function getNextSequence(name: string) {
  try {
    // 使用 key 字段保存序列，_id 由 MongoDB 生成为 ObjectId。
    // upsert 时确保设置 key 字段。
    const result = (await db.collection("config").findOneAndUpdate(
      { key: name } as any,
      { $inc: { seq: 1 }, $setOnInsert: { key: name } }, // 自增并在插入时设置 key
      {
        upsert: true, // 文档不存在就创建
        returnDocument: "after", // 返回更新后的文档
      }
    )) as any;

    if (!result.value) {
      // 兜底查询，兼容旧格式或异常情况
      const doc = (await db
        .collection("config")
        .findOne({ key: name } as any)) as any;
      return doc?.seq ?? null;
    }

    return result.value.seq ?? null;
  } catch (err) {
    logger.error("getNextSequence 出错:", err);
    return null;
  }
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
      "无效的状态，有效状态：等待元数据、下载中、下载完成、上传中、完成、失败、等待纠正"
    );
  }

  const torrentData = {
    title: title || null, // 可选标题
    magnetLink,
    status,
    createdAt: new Date(),
    updatedAt: new Date(),
  };

  try {
    const result = await db.collection("torrents").insertOne(torrentData);
    return result.insertedId;
  } catch (error) {
    throw new Error(
      `保存种子信息失败: ${error instanceof Error ? error.message : error}`
    );
  }
}

/**
 * 保存或更新动漫信息
 * @param anime - 动漫对象，必须包含 id 字段
 * @param cache - 是否保存到缓存集合，默认为 false（保存到正式集合）
 */
export async function saveAnime(anime: AnimeType, cache: boolean = false) {
  if (!anime || typeof anime !== "object") {
    throw new Error("无效的动漫数据");
  }
  if (!anime.id) {
    throw new Error("动漫数据缺少 id");
  }

  // 选择集合
  const col = db.collection<AnimeType>(cache ? "cacheAnime" : "anime");
  // 确保 id 唯一索引
  await col.createIndex({ id: 1 }, { unique: true });

  const oldDoc = await col.findOne({ id: anime.id });

  // 需要更新的字段
  const updateFields: (keyof AnimeType)[] = [
    "name",
    "name_cn",
    "summary",
    "tags",
    "episode",
    "score",
    "airingDay",
    "airingStart",
  ];

  if (oldDoc) {
    // 只更新指定字段
    const update: Partial<AnimeType> = {};

    for (const key of updateFields) {
      setField(update, anime, key);
    }

    // 合并 btdata（按 title 去重）
    if (anime.btdata) {
      update.btdata = { ...oldDoc.btdata };

      for (const [source, newArr] of Object.entries(anime.btdata)) {
        const oldArr = oldDoc.btdata?.[source] ?? [];
        const map = new Map<string, BtEntry>();

        // 放旧的
        for (const item of oldArr) {
          map.set(item.title, item);
        }
        // 放新的（覆盖同 title）
        for (const item of newArr) {
          map.set(item.title, { ...map.get(item.title), ...item });
        }

        update.btdata[source] = Array.from(map.values());
      }
    }

    update.updatedAt = new Date();

    await col.updateOne({ id: anime.id }, { $set: update });
    return anime.id;
  } else {
    // 新建
    const doc: AnimeType = {
      ...anime,
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
function setField<K extends keyof AnimeType>(
  target: Partial<AnimeType>,
  source: AnimeType,
  key: K
) {
  if (source[key] !== undefined) {
    target[key] = source[key];
  }
}
