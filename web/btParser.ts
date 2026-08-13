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
): Promise<animeItem> {
    const parts = url.split("/torrent/");
    if (parts.length < 2) {
        throw new Error(`bangumi 链接缺少 /torrent/ 段: ${url}`);
    }
    const id = parts[1]!.split(/[/?#]/)[0]!;

    const torrentInfo = await fetchBangumiTorrent(id);
    if (!torrentInfo?.magnet) {
        throw new Error(`无法获取 bangumi 种子信息（torrent_id=${id}）→ 未取到磁力链接`);
    }
    const fansub = extractFansub(torrentInfo.title);
    if (!fansub || fansub.length === 0) {
        throw new Error(`无法从标题解析字幕组，标题可能不以 [字幕组] 开头：${torrentInfo.title}`);
    }

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
    if (!infoq) {
        throw new Error(`解析标题信息失败：${torrentInfo.title}`);
    }
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
    if (infoq.names.length === 0) {
        throw new Error(`解析后无有效动漫名称（白名单为空）：${torrentInfo.title}`);
    }

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

async function parseDmhy(url: string): Promise<animeItem> {
    let dmhyinfo;
    try {
        dmhyinfo = await fetchDmhyTorrent(url);
    } catch (e) {
        throw new Error(
            `请求 dmhy 页面失败：${(e as Error)?.message ?? String(e)}（请稍后重试，或检查网络/域名可访问性）`
        );
    }
    if (!dmhyinfo?.magnet || dmhyinfo.magnet === "未知磁力链接") {
        throw new Error(`未从 dmhy 页面取到磁力链接（标题=${dmhyinfo?.title ?? "未知"}），页面结构可能已变化`);
    }
    const fansub = extractFansub(dmhyinfo.title);
    if (!fansub || fansub.length === 0) {
        throw new Error(`无法从标题解析字幕组，标题可能不以 [字幕组] 开头：${dmhyinfo.title ?? "未知标题"}`);
    }

    const infoq = parseInfo(dmhyinfo.title, dmhyinfo.team);
    if (!infoq) {
        throw new Error(`解析标题信息失败：${dmhyinfo.title ?? "未知标题"}`);
    }
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
 * 解析失败会抛出带具体原因的错误。
 */
export async function parseBtSource(url: string): Promise<animeItem> {
    if (!url) {
        throw new Error("BT 链接不能为空");
    }
    if (url.includes("bangumi")) {
        return parseBangumi(url);
    }
    if (url.includes("dmhy")) {
        return parseDmhy(url);
    }
    throw new Error(`不支持该 BT 来源（仅支持 bangumi / dmhy）：${url}`);
}
