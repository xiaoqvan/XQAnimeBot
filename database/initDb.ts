import logger from "@log/index.ts";
import { getDatabase } from "@db/index.ts";
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
  BG_APP_ID: "",
  BG_APP_SECRET: "",
});

/**
 * 初始化数据库连接并设置全局引用
 * 到所需的数据库集合。
 */
async function initdb() {
  const db = await getDatabase();

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

  // 为 anime 集合创建搜索索引
  try {
    const anime = db.collection("anime");

    // 为 name 字段创建索引（用于文本搜索）
    await anime.createIndex(
      { name: 1 },
      { name: "name_idx" }
    );

    // 为 name_cn 字段创建索引（用于文本搜索）
    await anime.createIndex(
      { name_cn: 1 },
      { name: "name_cn_idx" }
    );

    // 为 names 数组字段创建索引（用于文本搜索）
    await anime.createIndex(
      { names: 1 },
      { name: "names_idx" }
    );

    logger.info("anime 集合索引创建成功");
  } catch (err) {
    logger.error("为 anime 创建索引时出错", err);
    throw err;
  }

  return db;
}

// 模块加载时只创建一次
export const databasePromise = initdb();


