import type { message as messageType } from "tdlib-types";
import type { Client } from "tdl";

import { isUserAdmin } from "@TDLib/function/index.ts";
import { editMessageText, sendMessage } from "@TDLib/function/message.ts";
import { parseInfo, updateAnime } from "../anime/index.ts";
import { getAnimeById } from "../database/query.ts";
import {
  fetchBangumiTags,
  fetchBangumiTeam,
  fetchBangumiTorrent,
  fetchDmhyTorrent,
} from "../anime/info.ts";
import { formatPubDate } from "../anime/rss/bangumi.ts";
import { formatDmhyPubDate } from "../anime/rss/dmhy.ts";

export default async function addAnime(
  client: Client,
  message: messageType,
  commandParts: string[] | undefined
) {
  // 检查是否为管理员
  const isAdmin = await isUserAdmin(
    client,
    Number(process.env.ADMIN_GROUP_ID),
    message.sender_id
  );
  const isBotAdmin =
    message.sender_id._ === "messageSenderUser" &&
    message.sender_id.user_id === Number(process.env.BOT_ADMIN_ID);

  if (!isAdmin && !isBotAdmin) {
    return;
  }
  if (
    message.content._ !== "messageText" ||
    message.sender_id._ !== "messageSenderUser"
  ) {
    return;
  }

  if (!commandParts || commandParts.length < 2) {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: "❌ 用法错误！\n\n**正确用法**:\n`/addanime <动漫ID> <图片URL>` - 为指定ID的动漫添加BT信息\n\n**示例**:\n`/addanime 12345 https://example.com/torrent`",
      link_preview: true,
    });
    return;
  }
  const anime = commandParts[0];
  const url = commandParts[1];
  if (!anime || !url) {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: "用法: /addanime <动漫名称> <图片URL>",
      link_preview: true,
    });
    return;
  }
  const animeinfo = await getAnimeById(Number(anime));

  if (!animeinfo) {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: `未找到ID为 ${anime} 的动漫信息。请确保ID正确。`,
    });
    return;
  }

  const tipsMsg = await sendMessage(client, message.chat_id, {
    reply_to_message_id: message.id,
    text: `正在为动漫 ${
      animeinfo.name_cn || animeinfo.name
    } 添加BT信息，请稍候...`,
  });
  // 获取动漫信息
  let animeBtInfo;

  if (url.includes("bangumi")) {
    const parts = url.split("/torrent/");
    if (parts.length < 2) {
      return;
    }
    const rest = parts[1];
    // 如果后面可能有路径或查询参数，就用 split 再分一次
    const id = rest.split(/[/?#]/)[0];

    const torrentInfo = await fetchBangumiTorrent(id);
    // 获取作者信息
    // 从标题中提取字幕组信息
    let fansub = null;
    const match = torrentInfo.title.match(
      /^(?:\[([^\]]+)\]|【([^】]+)】)/
    ) as string[];
    if (!match) {
      return; // 跳过无法解析的条目
    }
    if (match) {
      const raw = match[1] || match[2];
      fansub = raw
        .split(/\s*[&/|｜、]\s*/)
        .map((s) => s.trim())
        .filter(Boolean);
    }

    if (fansub === null || fansub.length === 0) {
      return;
    }
    // 提取发布组信息
    let team = [];
    if (torrentInfo.team_id) {
      team = await fetchBangumiTeam(torrentInfo.team_id);
    } else {
      team = [{ name: fansub[0] }];
    }
    const tags =
      torrentInfo.tag_ids && torrentInfo.tag_ids.length > 0
        ? await fetchBangumiTags(torrentInfo.tag_ids)
        : [];

    // 从tags中找到type为"bangumi"的项目，提取locale信息
    const bangumiTag = tags.find(
      (tag: {
        type?: string;
        locale?: { zh_cn?: string; ja?: string; en?: string };
      }) => tag.type === "bangumi"
    );
    const nameLocales = bangumiTag
      ? {
          cn: bangumiTag.locale.zh_cn || "",
          jp: bangumiTag.locale.ja || "",
          en: bangumiTag.locale.en || "",
        }
      : {
          cn: "",
          jp: "",
          en: "",
        };

    const infoq = parseInfo(torrentInfo.title, team[0]?.name);
    if (!infoq) {
      return;
    }
    // 将 nameLocales 中每个语言的文本追加到 infoq.names 中，去重并去空
    const localeNames = [nameLocales.cn, nameLocales.jp, nameLocales.en]
      .filter((s) => typeof s === "string" && s.trim() !== "")
      .map((s) => s.trim());

    infoq.names = Array.isArray(infoq.names)
      ? infoq.names
          .map((s) => (typeof s === "string" ? s.trim() : ""))
          .filter(Boolean)
      : [];

    infoq.names = Array.from(new Set([...infoq.names, ...localeNames])).filter(
      Boolean
    );

    // 白名单机制：如果最终 names 为空，跳过该条目
    if (!infoq.names || infoq.names.length === 0) {
      return;
    }

    animeBtInfo = {
      title: torrentInfo.title,
      pubDate: formatPubDate(torrentInfo.pubDate),
      magnet: torrentInfo.magnet,
      team: team[0]?.name,
      fansub,
      ...infoq,
    };
  } else if (url.includes("dmhy")) {
    const dmhyinfo = await fetchDmhyTorrent(url);

    // 从标题中提取字幕组信息
    let fansub = null;
    const match = dmhyinfo.title.match(
      /^(?:\[([^\]]+)\]|【([^】]+)】)/
    ) as string[];
    if (!match) {
      return; // 跳过无法解析的条目
    }
    if (match) {
      const raw = match[1] || match[2];
      fansub = raw
        .split(/\s*[&/|｜、]\s*/)
        .map((s) => s.trim())
        .filter(Boolean);
    }

    if (fansub === null || fansub.length === 0) {
      return;
    }

    const infoq = parseInfo(dmhyinfo.title, dmhyinfo.team);
    if (!infoq) {
      return;
    }

    animeBtInfo = {
      title: dmhyinfo.title,
      pubDate: formatPubDate(formatDmhyPubDate(dmhyinfo.pubDate)),
      magnet: dmhyinfo.magnet,
      team: dmhyinfo.team,
      fansub,
      ...infoq,
    };
  } else {
    await sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: "目前仅支持添加bangumi和dmhy的链接。",
    });
    return;
  }

  await updateAnime(client, animeinfo, animeBtInfo);
  if (!tipsMsg) {
    return;
  }
  await editMessageText(client, message.chat_id, tipsMsg.id, {
    text: `已为动漫 ${animeinfo.name_cn || animeinfo.name} 添加BT信息.`,
  });
}
