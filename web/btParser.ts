import type { animeItem } from "../types/rss.d.ts";
import {
    fetchBangumiTags,
    fetchBangumiTeam,
    fetchBangumiTorrent,
    fetchDmhyTorrent,
} from "../anime/get.ts";
import { formatPubDate } from "../anime/rss/bangumi.ts";
import { formatDmhyPubDate } from "../anime/rss/dmhy.ts";
import { extractEpisodeByAI, parseInfo } from "../utils/animeParser.ts";

const FANSUB_RE = /^(?:\[([^\]]+)]|【([^】]+)】)/;

function extractFansub(title: string): string[] | null {
    const match = title.match(FANSUB_RE) as string[] | null;
    if (!match) return null;
    const raw = match[1] || match[2] || "";
    return raw
        .split(/\s*[&/|｜、]\s*/)
        .map((s) => s.trim())
        .filter(Boolean);
}

/** 兼容 addanime / btParser：解析 bangumi 种子并组装 animeItem */
async function parseBangumi(
    url: string
): Promise<animeItem | null> {
    const parts = url.split("/torrent/");
    if (parts.length < 2) return null;
    const id = parts[1]!.split(/[/?#]/)[0]!;

    const torrentInfo = await fetchBangumiTorrent(id);
    const fansub = extractFansub(torrentInfo.title);
    if (!fansub || fansub.length === 0) return null;

    let team: { name: string }[];
    if (torrentInfo.team_id) {
        team = await fetchBangumiTeam(torrentInfo.team_id);
    } else {
        team = [{ name: fansub[0]! }];
    }

    const tags =
        torrentInfo.tag_ids && torrentInfo.tag_ids.length > 0
            ? await fetchBangumiTags(torrentInfo.tag_ids)
            : [];
    const bangumiTag = tags.find(
        (tag: { type?: string; locale?: { zh_cn?: string; ja?: string; en?: string } }) =>
            tag.type === "bangumi"
    );
    const nameLocales = bangumiTag
        ? {
            cn: bangumiTag.locale?.zh_cn || "",
            jp: bangumiTag.locale?.ja || "",
            en: bangumiTag.locale?.en || "",
        }
        : { cn: "", jp: "", en: "" };

    const infoq = parseInfo(torrentInfo.title, team[0]?.name ?? "");
    if (!infoq) return null;
    if (!infoq.episode || infoq.episode === "未知") {
        const aiEpisode = await extractEpisodeByAI(torrentInfo.title, infoq.names);
        if (aiEpisode) infoq.episode = aiEpisode;
    }

    const localeNames = [nameLocales.cn, nameLocales.jp, nameLocales.en]
        .filter((s) => typeof s === "string" && s.trim() !== "")
        .map((s) => s.trim());
    infoq.names = Array.isArray(infoq.names)
        ? infoq.names.map((s) => (typeof s === "string" ? s.trim() : "")).filter(Boolean)
        : [];
    infoq.names = Array.from(new Set([...infoq.names, ...localeNames])).filter(Boolean);
    if (infoq.names.length === 0) return null;

    return {
        title: torrentInfo.title,
        pubDate: formatPubDate(torrentInfo.pubDate),
        magnet: torrentInfo.magnet,
        team: team[0]?.name ?? "",
        link: url,
        fansub,
        ...infoq,
    };
}

async function parseDmhy(url: string): Promise<animeItem | null> {
    const dmhyinfo = await fetchDmhyTorrent(url);
    const fansub = extractFansub(dmhyinfo.title);
    if (!fansub || fansub.length === 0) return null;

    const infoq = parseInfo(dmhyinfo.title, dmhyinfo.team);
    if (!infoq) return null;
    if (!infoq.episode || infoq.episode === "未知") {
        const aiEpisode = await extractEpisodeByAI(dmhyinfo.title, infoq.names);
        if (aiEpisode) infoq.episode = aiEpisode;
    }

    return {
        title: dmhyinfo.title,
        pubDate: formatPubDate(formatDmhyPubDate(dmhyinfo.pubDate)),
        magnet: dmhyinfo.magnet,
        team: dmhyinfo.team,
        link: url,
        fansub,
        ...infoq,
    };
}

/**
 * 根据 BT 来源 URL 解析出 animeItem（兼容 bangumi / dmhy）。
 * 解析失败返回 null。
 */
export async function parseBtSource(url: string): Promise<animeItem | null> {
    if (!url) return null;
    if (url.includes("bangumi")) {
        return parseBangumi(url);
    }
    if (url.includes("dmhy")) {
        return parseDmhy(url);
    }
    return null;
}
