
import type { message as messageType } from "tdlib-types";
import type { Client } from "tdl";

import { isUserAdmin } from "@TDLib/function/index.ts";
import { editMessageCaption, editMessageText, sendMessage } from "@TDLib/function/message.ts";
import { getAnimeById } from "../database/query.ts";
import {
    fetchBangumiTags,
    fetchBangumiTeam,
    fetchBangumiTorrent,
    fetchDmhyTorrent,
    getSubjectById,
} from "../anime/get.ts";
import { formatPubDate } from "../anime/rss/bangumi.ts";
import { formatDmhyPubDate } from "../anime/rss/dmhy.ts";
import { env } from "../database/initDb.ts";
import { getConfig } from "@db/config.ts";
import { parseInfo } from "../utils/animeParser.ts";
import { getEpisodeById } from "../bangumi/get.ts";
import { buildAndSaveAnimeFromInfo } from "../utils/buildAnimeinfo.ts";
import { addTorrent } from "../database/create.ts";
import { downloadAndValidateTorrent, removeTorrentAndData } from "../qBittorrent/download.ts";
import { sendMegToAnime, sendMegToNavAnime } from "../anime/sendAnime.ts";
import type {
    albumMessageType,
} from "../types/message.d.ts";
import { getMessageLink } from "@TDLib/function/get.ts";
import { updateAnimeBtdata } from "../database/update.ts";
import { combineFansub } from "../utils/index.ts";
import { AnimeText } from "../anime/text.ts";

export default async function addAnime(
    client: Client,
    message: messageType,
    commandParts: string[] | undefined
) {
    const config = await getConfig("admin");
    // 检查是否为管理员
    const isAdmin = await isUserAdmin(
        client,
        Number(env.data.ADMIN_GROUP_ID),
        message.sender_id
    );
    const isBotAdmin =
        message.sender_id._ === "messageSenderUser" &&
        message.sender_id.user_id === config?.super_admin;

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
            text: "❌ 用法错误！\n\n**正确用法**:\n`/addnewanime <动漫集数ID> <磁力URL>` - 添加新的动漫BT信息\n\n**示例**:\n`/addnewanime 12345 https://example.com/torrent`",
            link_preview: true,
        });
        return;
    }
    const epid = commandParts[0];
    const url = commandParts[1];
    if (!epid || !url) {
        await sendMessage(client, message.chat_id, {
            reply_to_message_id: message.id,
            text: "用法: /addnewanime <动漫集数ID> <磁力URL>",
            link_preview: true,
        });
        return;
    }
    const epinfo = await getEpisodeById(Number(epid));

    const anime = await getSubjectById(epinfo?.subject_id || 0);
    const tipsMsg = await sendMessage(client, message.chat_id, {
        reply_to_message_id: message.id,
        text: `动漫 ${anime.name_cn || anime.name
            } 添加BT信息，请稍候...`,
    });
    // 获取动漫信息
    let animeBtInfo;

    if (url.includes("bangumi")) {
        const parts = url.split("/torrent/");
        if (parts.length < 2) {
            return;
        }
        const rest = parts[1]!;
        // 如果后面可能有路径或查询参数，就用 split 再分一次
        const id = rest.split(/[/?#]/)[0]!;

        const torrentInfo = await fetchBangumiTorrent(id);
        // 获取作者信息
        // 从标题中提取字幕组信息
        let fansub = null;
        const match = torrentInfo.title.match(
            /^(?:\[([^\]]+)]|【([^】]+)】)/
        ) as string[];
        if (match) {
            const raw = match[1] || match[2] || '';
            fansub = raw
                .split(/\s*[&/|｜、]\s*/)
                .map((s) => s.trim())
                .filter(Boolean);
        }

        if (fansub === null || fansub.length === 0) {
            return;
        }
        // 提取发布组信息
        let team: { name: string }[]
        if (torrentInfo.team_id) {
            team = await fetchBangumiTeam(torrentInfo.team_id);
        } else {
            team = [{ name: fansub[0]! }];
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

        const infoq = parseInfo(torrentInfo.title, team[0]?.name!);
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
            team: team[0]?.name!,
            link: url,
            fansub,
            ...infoq,
        };
    } else if (url.includes("dmhy")) {
        const dmhyinfo = await fetchDmhyTorrent(url);

        // 从标题中提取字幕组信息
        let fansub = null;
        const match = dmhyinfo.title.match(
            /^(?:\[([^\]]+)]|【([^】]+)】)/
        ) as string[];
        if (!match) {
            return; // 跳过无法解析的条目
        }
        if (match) {
            const raw = match[1] || match[2] || '';
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
            link: url,
            ...infoq,
        };
    } else {
        await sendMessage(client, message.chat_id, {
            reply_to_message_id: message.id,
            text: "目前仅支持添加bangumi和dmhy的链接。",
        });
        return;
    }

    const newanime = await buildAndSaveAnimeFromInfo(anime, false);

    await addTorrent(animeBtInfo.magnet, "等待下载", animeBtInfo.title);

    const torrent = await downloadAndValidateTorrent(animeBtInfo);

    const animeMeg = await sendMegToAnime(
        client,
        newanime,
        animeBtInfo,
        torrent.content_path,
        Number(epid),
        torrent.segments
    );

    if (!animeMeg) {
        await removeTorrentAndData(torrent.hash);
        throw new Error(`发送动漫消息失败${animeBtInfo.title}`);
    }
    await removeTorrentAndData(torrent.hash);

    // 提取主消息（单条或相册第一条）
    const animeMegList = animeMeg._ === "messages" ? animeMeg.messages.filter((m): m is messageType => m !== null) : [animeMeg];
    const primaryAnimeMeg = animeMegList[0];
    if (!primaryAnimeMeg) throw new Error(`发送动漫消息失败: 无有效消息 ${animeBtInfo.title}`);

    const animeAllMsgData: albumMessageType[] = animeMegList.map((msg) => ({
        chat_id: msg.chat_id,
        message_id: msg.id,
        topic_id: msg.topic_id,
        videoid: msg.content._ === "messageVideo" ? msg.content.video.video.remote.id : undefined,
        unique_id: msg.content._ === "messageVideo" ? msg.content.video.video.remote.unique_id : undefined,
    }));
    const animeAllVideoids = animeAllMsgData.map((m) => m.videoid).filter((id): id is string => !!id);
    const animeAllUniqueIds = animeAllMsgData.map((m) => m.unique_id).filter((id): id is string => !!id);

    const animeLink = await getMessageLink(client, primaryAnimeMeg.chat_id, primaryAnimeMeg.id);

    // 更新动漫的数据库信息
    await updateAnimeBtdata(
        anime.id,
        Number(epid),
        combineFansub(animeBtInfo.fansub),
        animeBtInfo.episode || "未知",
        {
            chat_id: primaryAnimeMeg.chat_id,
            message_id: primaryAnimeMeg.id,
            thread_id: primaryAnimeMeg.topic_id
                ? primaryAnimeMeg.topic_id._ === "messageTopicForum"
                    ? primaryAnimeMeg.topic_id.forum_topic_id
                    : 0
                : 0,
            link: animeLink.link,
        },
        animeBtInfo.title,
        animeBtInfo.source,
        animeBtInfo.names,
        animeAllVideoids[0],
        animeAllUniqueIds[0],
        undefined,
        false,
        animeAllVideoids.length > 1 ? animeAllVideoids : undefined,
        animeAllUniqueIds.length > 1 ? animeAllUniqueIds : undefined,
        animeAllMsgData.length > 1 ? animeAllMsgData : undefined
    );
    await sendMegToNavAnime(client, newanime.id);

    const nanime = await getAnimeById(anime.id);
    if (!nanime) {
        return
    }

    const text = AnimeText(nanime, animeBtInfo, Number(epid));

    if (animeMeg._ === "messages" && animeMeg.messages[0]) {
        await editMessageCaption(client, animeMeg.messages[0].chat_id, animeMeg.messages[0].id, {
            text,
        });
    } else {
        await editMessageCaption(client, primaryAnimeMeg.chat_id, primaryAnimeMeg.id, {
            text,
        });
    }

    if (!tipsMsg) {
        return;
    }
    await editMessageText(client, message.chat_id, tipsMsg.id, {
        text: `已为动漫 ${newanime.name_cn || newanime.name} 添加BT信息并发送相关消息！ ${animeLink.link} \n\n如果需要更新或修改BT信息，请使用 /addanime 命令。`,
    });
}
