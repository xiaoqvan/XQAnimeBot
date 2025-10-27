import { getEpisodeInfo } from "../anime/get.ts";
import { saveAnime } from "../database/create.ts";
import type { bangumiAnime, infobox, anime } from "../types/anime.d.ts";
import { extractFilteredTagNames } from "./index.ts";

/**
 * 5.1 新番剧更新动漫信息中构建并保存动漫数据
 * @param info - 动漫信息
 * @param newanime - 是否为新番剧如果你需要直接保存到anime集合中而不是缓存传递 false
 */
export async function buildAndSaveAnimeFromInfo(
  info: bangumiAnime,
  newanime: boolean
) {
  const EpisodeInfo = await getEpisodeInfo(info.id);
  const infobox = extractInfoFromInfobox(info?.infobox || []);
  const anime: anime = {
    id: info.id,
    name_cn: info.name_cn || infobox.name,
    name: info.name,
    names: [
      ...new Set(
        [info.name_cn, info.name, ...infobox.names].filter((x): x is string =>
          Boolean(x)
        )
      ),
    ],
    image:
      info.images?.large ||
      info.images?.medium ||
      info.images?.common ||
      "https://dummyimage.com/350x600/cccccc/ffffff&text=%E6%97%A0%E5%B0%81%E9%9D%A2",
    summary: info.summary
      ? info.summary
          .replace(/\r\n/g, "\\n")
          .replace(/\n/g, "\\n")
          .replace(/\r/g, "\\n")
      : undefined,

    tags: info.tags ? (await extractFilteredTagNames(info.tags)) || [] : [],
    episode: infobox.episodeCount || undefined,
    eps: {
      total: EpisodeInfo.total || 0,
      list: EpisodeInfo.data.map((ep: any) => ({
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
      })),
    },
    score: info.rating?.score,
    navMessageLink: undefined,
    airingDay: infobox.broadcastDay || undefined,
    airingStart: infobox.broadcastStart || undefined,
  };
  if (newanime) {
    await saveAnime(anime, true);
    return anime;
  }
  await saveAnime(anime);
  return anime;
}

/**
 * 提取 bgm 番剧相信信息中的动漫信息
 * @param infobox - 信息盒数组
 * @returns - 提取的动漫信息
 */
export function extractInfoFromInfobox(
  infoboxList: infobox[] | Array<{ key: string; value: any }>
) {
  type InfoboxItem = { key: string; value: any };

  const result: {
    name: string;
    names: string[];
    episodeCount: string;
    broadcastDay: string;
    broadcastStart: string;
  } = {
    name: "",
    names: [],
    episodeCount: "",
    broadcastDay: "",
    broadcastStart: "",
  };

  for (const item of infoboxList as InfoboxItem[]) {
    const key = String(item.key || "");
    const value = item.value;

    switch (key) {
      case "中文名":
        if (typeof value === "string") {
          result.name = value;
          result.names.push(value);
        }
        break;
      case "别名":
        if (Array.isArray(value)) {
          for (const alias of value) {
            // alias 可能是对象 { v: string } 或直接字符串
            if (alias && typeof alias === "object" && "v" in alias) {
              const v = (alias as any).v;
              if (typeof v === "string" && v.trim()) {
                result.names.push(v);
              }
            } else if (typeof alias === "string" && alias.trim()) {
              result.names.push(alias);
            }
          }
        } else if (typeof value === "string" && value.trim()) {
          // 有时别名可能是单个字符串
          result.names.push(value);
        }
        break;
      case "话数":
        if (typeof value === "string") {
          result.episodeCount = value;
        } else if (typeof value === "number") {
          result.episodeCount = String(value);
        }
        break;
      case "放送星期":
        if (typeof value === "string") {
          result.broadcastDay = value;
        }
        break;
      case "放送开始":
        if (typeof value === "string") {
          result.broadcastStart = value;
        }
        break;
      default:
        break;
    }
  }

  // 去重，去除空值
  result.names = [...new Set(result.names.filter(Boolean))];

  return result;
}
