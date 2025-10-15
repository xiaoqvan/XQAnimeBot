import { getDatabase } from "./initDb.ts";
import { formatSubGroupName } from "../function/index.ts";
import type {
  anime as AnimeType,
  bangumiAnime,
  messageType,
} from "../types/anime.ts";
import { cleanTitle } from "../anime/rss/index.ts";
const db = await getDatabase();

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
      `更新动漫评分失败: ${error instanceof Error ? error.message : error}`
    );
  }
}

/**
 * 更新动漫的导航频道消息链接
 * @param animeId - 动漫的id字段值
 * @param navMessageLink - 导航频道消息链接
 * @returns 更新成功返回true，否则返回false
 * @throws 当参数无效或数据库操作失败时抛出异常
 * @deprecated 请使用 updateAnimeNavMessage 方法
 */
export async function updateAnimeNavMessageLink(
  animeId: number,
  navMessageLink: string
) {
  if (!animeId || !navMessageLink) {
    throw new Error("动漫ID和导航频道消息链接都是必需的参数");
  }

  try {
    // 首先查找动漫文档
    const anime = await db.collection("anime").findOne({ id: animeId });

    if (!anime) {
      throw new Error(`未找到ID为 ${animeId} 的动漫`);
    }

    // 更新导航频道消息链接
    const result = await db.collection("anime").updateOne(
      { id: animeId },
      {
        $set: {
          navMessageLink: navMessageLink,
          updatedAt: new Date(),
        },
      }
    );

    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新导航频道消息链接失败: ${
        error instanceof Error ? error.message : error
      }`
    );
  }
}
/**
 * 在cacheAnime数据库中为btdata对应字幕组的其中一集添加TGMegLink
 * 如果字幕组不存在则自动创建，如果集数不存在则自动添加
 * @param animeId - 动漫的id字段值
 * @param subGroup - 字幕组名称
 * @param episode - 集数
 * @param Message - TG消息详细
 * @param [title] - 集数标题（可选）
 * @param [names] - 该集数的别名数组（可选）
 * @param [pubDate] - 发布时间（可选）
 * @param [videoid] - 视频ID（可选）
 * @param [unique_id] - 唯一ID（可选）
 * @param [source] - 视频来源（如：Baha, bilibili等，用于ANi和黒ネズミたち字幕组区分）
 * @returns 更新成功返回true，否则返回false
 * @throws 当参数无效或数据库操作失败时抛出异常
 */
export async function updateAnimeBtdata(
  animeId: number,
  subGroup: string,
  episode: string | "未知",
  Message: messageType,
  title: string,
  source: string | undefined,
  names: string[] | [],
  videoid: string | undefined,
  unique_id: string | undefined,
  cacheItemId: number | undefined = undefined,
  cache: boolean = false
) {
  if (
    !animeId ||
    !subGroup ||
    !episode ||
    !Message ||
    (cache && !cacheItemId)
  ) {
    throw new Error(
      `动漫ID、字幕组、集数和Message是必需的参数${
        cache ? ", 当 cache 为 true 时，cacheItemId 也是必需的" : ""
      }`
    );
  }

  // 格式化字幕组名称（处理ANi和黒ネズミたち的source区分）
  const formattedSubGroup = formatSubGroupName(subGroup, source);

  try {
    // 根据 cache 参数选择集合（cacheAnime 或 anime）
    const collectionName = cache ? "cacheAnime" : "anime";

    // 首先查找动漫文档
    const anime = await db
      .collection<AnimeType>(collectionName)
      .findOne({ id: animeId });

    if (!anime) {
      throw new Error(`未找到ID为 ${animeId} 的动漫`);
    }

    // 初始化btdata（如果不存在）
    if (!anime.btdata) {
      anime.btdata = {};
    }

    // 如果字幕组不存在，则创建字幕组
    if (!anime.btdata[formattedSubGroup]) {
      let newEpisode = {
        episode: episode,
        Message: Message,
        title: title,
        cache_id: cacheItemId ? cacheItemId : undefined,
        videoid: videoid ? videoid : undefined,
        names: names ? names : undefined,
        unique_id: unique_id ? unique_id : undefined,
      };

      const updateQuery = {
        [`btdata.${formattedSubGroup}`]: [newEpisode],
      };

      const result = await db
        .collection<AnimeType>(collectionName)
        .updateOne({ id: animeId }, { $set: updateQuery });

      return result.modifiedCount > 0;
    }

    // 查找对应的集数
    const episodeIndex = anime.btdata[formattedSubGroup].findIndex(
      (ep) => ep.episode === episode
    );

    if (episodeIndex === -1) {
      // 如果集数不存在，则添加新集数
      const newEpisode = {
        episode: episode,
        Message: Message,
        title: title,
        videoid: videoid ? videoid : undefined,
        names: names ? names : undefined,
        unique_id: unique_id ? unique_id : undefined,
      };

      const updateQuery = {
        [`btdata.${formattedSubGroup}`]: newEpisode,
      };

      const result = await db
        .collection<AnimeType>(collectionName)
        .updateOne({ id: animeId }, { $push: updateQuery });

      return result.modifiedCount > 0;
    } else {
      // 更新对应集数的TGMegLink
      const updateQuery = {
        [`btdata.${formattedSubGroup}.${episodeIndex}.Message`]: Message,
        [`btdata.${formattedSubGroup}.${episodeIndex}.title`]: title,
        [`btdata.${formattedSubGroup}.${episodeIndex}.videoid`]: videoid
          ? videoid
          : undefined,
        [`btdata.${formattedSubGroup}.${episodeIndex}.names`]: names
          ? names
          : undefined,
        [`btdata.${formattedSubGroup}.${episodeIndex}.unique_id`]: unique_id
          ? unique_id
          : undefined,
      };

      const result = await db
        .collection<AnimeType>(collectionName)
        .updateOne({ id: animeId }, { $set: updateQuery });

      return result.modifiedCount > 0;
    }
  } catch (error) {
    throw new Error(
      `更新TGMegLink失败: ${error instanceof Error ? error.message : error}`
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
    let currentNames = Array.isArray(anime?.names) ? anime.names : [];
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
      `添加别名失败: ${error instanceof Error ? error.message : error}`
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
      `更新动漫R18字段失败: ${error instanceof Error ? error.message : error}`
    );
  }
}

/**
 * 更新动漫的导航频道消息链接
 * @param animeId - 动漫的id字段值
 * @param navMessageLink - 导航频道消息链接
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

    // 更新导航频道消息并移除旧的 navMessageLink 字段
    const result = await db.collection("anime").updateOne(
      { id: animeId },
      {
        $set: {
          navMessage: Message,
          updatedAt: new Date(),
        },
        $unset: {
          navMessageLink: "",
        },
      }
    );

    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新导航频道消息失败: ${error instanceof Error ? error.message : error}`
    );
  }
}

export async function updateAnimeNavVideoMessage(
  animeId: number,
  videoMessage:
    | {
        /** 索引，从1开始，navMessage为主消息 */
        page: number;
        /** 消息所属的聊天 ID */
        chat_id: number;
        /** 消息 ID */
        message_id: number;
        /** 线程 ID */
        thread_id?: number;
        /** 消息链接 */
        link: string;
      }[]
    | {
        /** 索引，从1开始，navMessage为主消息 */
        page: number;
        /** 消息所属的聊天 ID */
        chat_id: number;
        /** 消息 ID */
        message_id: number;
        /** 线程 ID */
        thread_id?: number;
        /** 消息链接 */
        link: string;
      }
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
      `更新导航频道视频消息失败: ${
        error instanceof Error ? error.message : error
      }`
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

    // 更新导航频道消息并移除旧的 navMessageLink 字段
    const result = await db.collection("anime").updateOne(
      { id: animeId },
      {
        $set: {
          name_cn: animeInfo.name_cn ?? anime.name_cn,
          summary: animeInfo.summary ?? anime.summary,

          updatedAt: new Date(),
        },
      }
    );

    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新导航频道消息失败: ${error instanceof Error ? error.message : error}`
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
    const collection = db.collection<AnimeType>("anime");
    const anime = await collection.findOne({ id: animeId });
    if (!anime) {
      throw new Error(`未找到ID为 ${animeId} 的动漫`);
    }

    // 构建新的 list，尽量保持与 buildAndSaveAnimeFromInfo 中相同的字段
    const list = EpisodeInfo.data.map((ep: any) => ({
      airdate: ep.airdate,
      name: ep.name,
      name_cn: ep.name_cn,
      duration: ep.duration,
      desc: ep.desc,
      ep: ep.ep,
      sort: ep.sort,
      id: ep.id,
      subject_id: ep.subject_id,
      comment: ep.comment,
    }));

    const result = await collection.updateOne(
      { id: animeId },
      {
        $set: {
          "eps.total": EpisodeInfo.total || 0,
          "eps.list": list,
          updatedAt: new Date(),
        },
      }
    );

    return result.modifiedCount > 0;
  } catch (error) {
    throw new Error(
      `更新动漫集数信息失败: ${error instanceof Error ? error.message : error}`
    );
  }
}
