import logger from "@log/index.ts";
import { getMongoClient } from "@db/index.ts";
import { JSONFilePreset } from "lowdb/node";
import type { animeenv } from "../types/json.d.ts";
import setjons from "./set.json?file";

// 初始化，set.json 文件不存在时会用默认值创建
export const env = await JSONFilePreset<animeenv>(setjons, {
  QBITTORRENT_HOST: "",
  QBITTORRENT_USERNAME: "",
  QBITTORRENT_PASSWORD: "",
  NAV_CHANNEL: 0,
  ANIME_CHANNEL: 0,
  ADMIN_GROUP_ID: 0,
  ANIME_GROUP_THREAD_ID: 0,
  NAV_GROUP_THREAD_ID: 0,
  ERROR_GROUP_THREAD_ID: 0,
});

/**
 * 初始化数据库连接并设置全局引用
 * 到所需的数据库集合。
 */
async function initdb() {
  const dbclient = await getMongoClient();

  const db = dbclient.db("anime");

  // 为 torrents 集合创建 title 字段的唯一索引
  try {
    const torrents = db.collection("torrents");
    await torrents.createIndex(
      { title: 1 },
      { unique: true, name: "title_unique_idx" }
    );
  } catch (err) {
    logger.error("为 torrents 创建索引时出错", err);
    throw err;
  }

  return db;
}

// 模块加载时只创建一次
export const databasePromise = initdb();

/**
 * 获取数据库连接
 * @returns 数据库连接的Promise
 */
export async function getDatabase() {
  return await databasePromise;
}
