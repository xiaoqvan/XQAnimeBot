import axios from "axios";
import * as cheerio from "cheerio";
import logger from "@log/index.ts";
import type { RssAnimeItem } from "../../types/anime.ts";

const authorMapping: Record<string, string> = {
  smzase: "三明治摆烂组",
  nekomoekissaten: "喵萌奶茶屋",
  TrySail: "Kirara Fantasia",
  ANiTorrent: "ANi",
  dmguo: "動漫國字幕組",
  MingYSub: "MingYSub",
  SweetSub: "SweetSub",
  Haruhanasub: "拨雪寻春",
  星空字幕组: "星空字幕组",
  悠哈璃羽字幕社: "悠哈璃羽字幕社",
  orion: "猎户压制部",
  aagaguai: "奇怪机翻组",
  晚街与灯: "晚街与灯",
  亿次研同好会: "亿次研同好会",
  北宇治字幕组: "北宇治字幕组",
  sakuratosub: "桜都字幕组",
  云歌: "云歌字幕组",
  樱桃花字幕组: "樱桃花字幕组",
};

// 白名单：只允许这些作者的内容
const authorWhitelist = Object.keys(authorMapping);

/**
 * 格式化发布时间
 * @param pubDateString 发布时间字符串
 * @returns
 */
function formatPubDate(pubDateString: string) {
  const date = new Date(pubDateString);

  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");

  let hours = date.getHours();
  const minutes = String(date.getMinutes()).padStart(2, "0");
  const ampm = hours >= 12 ? "PM" : "AM";

  hours = hours % 12;
  hours = hours ? hours : 12; // 0点显示为12点
  const formattedHours = String(hours).padStart(2, "0");

  return `${year}年${month}月${day}日 ${formattedHours}:${minutes}${ampm}`;
}

export async function fetchAcgnxRss() {
  try {
    const response = await axios.get("https://share.acgnx.se/rss-sort-1.xml");
    const xml = response.data;
    const $ = cheerio.load(xml, { xmlMode: true });
    const items = $("channel > item");
    const acgnxList: RssAnimeItem[] = [];
    for (const item of items) {
      const title = $("<div>").html($(item).find("title").text()).text().trim();
      const link = $(item).find("link").text();
      const pubDateRaw = $(item).find("pubDate").text();
      const pubDate = formatPubDate(pubDateRaw);
      const magnet = $(item).find("enclosure").attr("url");

      const originalAuthor = $(item).find("author").text();

      if (originalAuthor.includes("鏡像")) {
        continue;
      }
      if ($(item).find("category").text() !== "動畫") {
        continue;
      }

      // 过滤标题中含有特定内容的条目
      if (
        title.includes("内封") ||
        title.includes("繁") ||
        title.includes("合集") ||
        title.includes("無字幕") ||
        title.includes("粵語") ||
        title.includes("整理搬运") ||
        title.includes("無對白字幕") ||
        /\bFin\b/i.test(title) || // 精确匹配 Fin
        title.includes("BIG5") || // 匹配所有包含 BIG5 的情况
        /\bMKV\b/i.test(title) || // 精确匹配 MKV（不是 xxxMKVxxx）
        title.includes("[720p]")
      )
        continue;

      // 幻樱字幕组特殊过滤：只要中文版本和1080P
      if (title.includes("【幻樱字幕组】")) {
        // 过滤掉BIG5繁体版本
        if (title.includes("【BIG5_MP4】")) {
          continue;
        }
        // 过滤掉720P版本
        if (title.includes("【1280X720】")) {
          continue;
        }
        // 只保留GB简体和1080P版本
        if (!title.includes("【GB_MP4】") || !title.includes("【1920X1080】")) {
          continue;
        }
      }

      // 悠哈璃羽字幕社特殊过滤：只要CHS简体版本，排除CHT繁体版本
      if (title.includes("【悠哈璃羽字幕社】")) {
        // 过滤掉CHT繁体版本
        if (title.includes("[CHT]")) {
          continue;
        }
      }

      // 过滤集数区间
      if (/\[\d{1,3}-\d{1,3}\]|\(\d{1,3}-\d{1,3}\)/.test(title)) continue;

      // 检查作者是否在白名单中
      if (!authorWhitelist.includes(originalAuthor)) {
        continue;
      }
      if (!magnet) {
        continue;
      }

      // 使用映射表转换作者名称，如果没有匹配则保持原名
      const author = authorMapping[originalAuthor] || originalAuthor;

      acgnxList.push({
        type: "acgnx",
        title,
        link,
        author,
        pubDate,
        magnet,
      });
    }

    return acgnxList;
  } catch (error) {
    logger.error("Error fetching Acgnx data:", error);
    throw error;
  }
}
