import axios from "axios";
import * as cheerio from "cheerio";
import type { RssAnimeItem } from "../../types/rss.d.ts";
import { isTitleAllowed } from "./common.ts";

export const authorMapping: Record<string, string> = {
  smzase: "三明治摆烂组",
  nekomoekissaten: "喵萌奶茶屋",
  ANiTorrent: "ANi",
  春音爱良aira: "動漫國字幕組",
  MingY明: "MingYSub",
  wudihongjing: "MingYSub",
  SweetSub: "SweetSub",
  芙宁娜: "拨雪寻春",
  Needfire: "拨雪寻春",
  XKSub: "星空字幕组",
  千恋万花: "星空字幕组",
  捷德本尊: "星空字幕组",
  悠哈C9字幕社: "悠哈璃羽字幕社",
  望月月: "悠哈璃羽字幕社",
  orion321: "猎户压制部",
  aagaguai: "奇怪机翻组",
  晚街与灯: "晚街与灯",
  microseventh: "亿次研同好会",
  pianolibrary: "亿次研同好会",
  小圆香径独徘徊: "亿次研同好会",
  Kitauji: "北宇治字幕组",
  mistakey: "北宇治字幕组",
  sakurato: "桜都字幕组",
  shkong: "桜都字幕组",
  anmli: "桜都字幕组",
  清风夏沐: "云歌字幕组",
  萌樱字幕组: "云歌字幕组",
  Alicest: "S1百综字幕组",
  InsWan: "云歌字幕组",
  樱桃花字幕组desu: "樱桃花字幕组",
  D4869: "云光字幕组",
  KissSub: "爱恋字幕社",
  nagi123: "爱恋字幕社",
  kevin14827: "爱恋字幕社",
  HYSUB: "幻樱字幕组",
  summer1278: "幻樱字幕组",
  MingHyuk: "雪飄工作室"
};

// 白名单：只允许这些作者的内容
const authorWhitelist = Object.keys(authorMapping);

/** 格式化发布时间
 * @param pubDateString 发布时间字符串
 * @returns
 */
export function formatDmhyPubDate(pubDateString: string): string {
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

export async function fetchDmhyRss() {
  try {
    const response = await axios.get(
      "https://dmhy.org/topics/rss/sort_id/2/rss.xml",
      {
        timeout: 15000,
        headers: {
          "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
          "Referer": "https://dmhy.org/"
        }
      }
    );
    const xml = response.data;
    const $ = cheerio.load(xml, { xmlMode: true });
    const items = $("channel > item");
    const dmhyList: RssAnimeItem[] = [];
    // 只处理前50条数据
    const limitedItems = items.slice(0, 50);
    for (const item of limitedItems) {
      const title = $(item).find("title").text().trim().replace(/\s+/g, " ");
      const link = $(item).find("link").text();
      const pubDateRaw = $(item).find("pubDate").text();
      const pubDate = formatDmhyPubDate(pubDateRaw);
      const magnet = $(item).find("enclosure").attr("url");

      if ($(item).find("category").text() !== "動畫") {
        continue;
      }

      const originalAuthor = $(item).find("author").text();

      if (originalAuthor.includes("鏡像")) {
        continue;
      }

      if (!isTitleAllowed(title)) continue;

      // 检查作者是否在白名单中
      if (!authorWhitelist.includes(originalAuthor)) {
        continue;
      }
      if (!magnet) {
        continue;
      }

      // 使用映射表转换作者名称，如果没有匹配则保持原名
      const author = authorMapping[originalAuthor] || originalAuthor;

      dmhyList.push({
        type: "dmhy",
        title,
        link,
        author,
        pubDate,
        magnet,
      });
    }

    return dmhyList;
  } catch (error) {
    throw error;
  }
}
