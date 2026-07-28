import type { anime } from '../types/anime.d.ts';
import type { EpisodeMetaDoc } from '../types/episodeMeta.d.ts';
import { aiEpisodeSearch } from '../bangumi/bangumiAgent.ts';

/** 匹配结果接口 */
export type EpisodeMatchResult =
    | { status: 'MATCHED'; episodeId: number; ep: number; sort: number } // 成功匹配到章节
    | { status: 'NOT_FOUND_IN_DB'; msg: string } // 集数在列表里找不到（如第13集不在12集的列表里）
    | { status: 'DATE_MISMATCH'; msg: string; diffDays: number; episodeId: number } // 严重的时间不符
    | { status: 'INVALID_INPUT'; msg: string }; // BT标题没解析出集数

/**
 * 将 BT 的集数匹配到 Bangumi 章节 ID 支持越界和时间检查
 * @param dbAnime 数据库中已存的番剧信息
 * @param episodes 从剧集数据库读取的该番剧全部章节（建议已按 sort 排序）
 * @param btEpisodeStr BT 标题提取出的集数 (如 "02", "13", "05v2", "SP01")
 */
export async function matchBangumiEpisode(
    dbAnime: anime,
    episodes: EpisodeMetaDoc[],
    btEpisodeStr: string | undefined
): Promise<EpisodeMatchResult> {
    if (!btEpisodeStr) {
        return { status: 'INVALID_INPUT', msg: 'BT 条目未包含集数' };
    }

    // 1. 预处理 BT 集数
    // 移除 v2, end 等后缀，提取数字
    // 针对 "SP01", "OVA" 等情况，可能需要特殊逻辑，这里优先处理本篇数字
    const rawEp = btEpisodeStr.replace(/[vV]\d+/, '').replace(/END/i, '').trim();
    const epNum = parseFloat(rawEp);

    if (isNaN(epNum)) {
        // 规则无法解析集数，尝试 AI 辅助查找
        const aiCandidate = {
            id: dbAnime.id,
            name: dbAnime.name,
            name_cn: dbAnime.name_cn,
            episode_range: dbAnime.episode,
            summary: dbAnime.summary,
        };
        const aiResult = await aiEpisodeSearch(dbAnime.id, btEpisodeStr, aiCandidate);
        if (aiResult) {
            return {
                status: 'MATCHED',
                episodeId: aiResult.episodeId,
                ep: aiResult.episodeSort,
                sort: aiResult.episodeSort,
            };
        }
        return { status: 'INVALID_INPUT', msg: `无法从标题解析出集数: ${btEpisodeStr}` };
    }

    // 2. 检查数据库是否有章节列表
    if (!Array.isArray(episodes) || episodes.length === 0) {
        // 极端情况：Bangumi 条目存在但没有章节信息（如刚宣布未定档）
        return { status: 'NOT_FOUND_IN_DB', msg: '数据库中没有章节列表' };
    }

    const toNum = (value: unknown): number => {
        if (typeof value === 'number') return value;
        if (typeof value === 'string') {
            const n = Number(value.trim());
            return Number.isFinite(n) ? n : NaN;
        }
        return NaN;
    };

    const isMainEpisode = (episode: EpisodeMetaDoc): boolean => {
        const episodeType = toNum((episode as EpisodeMetaDoc & { type?: number | string }).type);
        return Number.isNaN(episodeType) || episodeType === 0;
    };

    const sortedEpisodes = [...episodes].sort((a, b) => {
        const sortDiff = toNum(a.sort) - toNum(b.sort);
        if (sortDiff !== 0) return sortDiff;
        return toNum(a.id) - toNum(b.id);
    });

    // 3. 在 eps.list 中查找
    // Bangumi API: type 0 = 本篇, 1 = SP, 2 = OP, 3 = ED
    // 我们优先匹配 type === 0 的本篇
    // 显式类型转换匹配：防止 sort 为字符串类型
    const matchedEp = sortedEpisodes.find(e => toNum(e.sort) === epNum && isMainEpisode(e));

    // ==========================================
    // 核心逻辑：越界检查 (Out of Range Check)
    // ==========================================
    if (!matchedEp) {
        // 场景：数据库只有 1-12 集，BT 是 13 集
        // 结果：返回 NOT_FOUND，提示调用者这是潜在的新季或错误
        const mainEpisodeSorts = sortedEpisodes
            .filter(isMainEpisode)
            .map(e => toNum(e.sort))
            .filter(sort => !Number.isNaN(sort));

        const minEpisode = mainEpisodeSorts.length > 0 ? Math.min(...mainEpisodeSorts) : 1;
        const maxEpisode = mainEpisodeSorts.length > 0 ? Math.max(...mainEpisodeSorts) : 1;
        const episodeRange = minEpisode === maxEpisode ? `${minEpisode}` : `${minEpisode}-${maxEpisode}`;

        // 规则匹配失败，尝试 AI 辅助查找
        const aiCandidate = {
            id: dbAnime.id,
            name: dbAnime.name,
            name_cn: dbAnime.name_cn,
            episode_range: dbAnime.episode,
            summary: dbAnime.summary,
        };
        const aiResult = await aiEpisodeSearch(dbAnime.id, btEpisodeStr, aiCandidate);
        if (aiResult) {
            return {
                status: 'MATCHED',
                episodeId: aiResult.episodeId,
                ep: aiResult.episodeSort,
                sort: aiResult.episodeSort,
            };
        }

        return {
            status: 'NOT_FOUND_IN_DB',
            msg: `未在当前季度找到第 ${epNum} 集（当前季集数范围：${episodeRange} 集）`
        };
    }

    // ==========================================
    // 核心逻辑：时间检查 (Time Validation)
    // 使用原生 Date，起始放送时间来源于 dbAnime.dbAnime（格式示例："2026年1月16日"），
    // 结束时间取 eps.list 中最后一集的 airdate（格式示例："2026-03-20"）。
    // 允许的匹配区间为: 起始放送时间 -8 天  到  最后一集放送时间 +30 天。
    if (matchedEp.airdate) {
        const parseDate = (s: string | undefined): Date | null => {
            if (!s) return null;
            const cn = /(?<y>\d{4})\D+(?<m>\d{1,2})\D+(?<d>\d{1,2})/.exec(s);
            if (cn && cn.groups) {
                const y = parseInt((cn.groups as any).y, 10);
                const m = parseInt((cn.groups as any).m, 10);
                const d = parseInt((cn.groups as any).d, 10);
                return new Date(y, m - 1, d);
            }
            const iso = /(?<y>\d{4})-(?<m>\d{1,2})-(?<d>\d{1,2})/.exec(s);
            if (iso && iso.groups) {
                const y = parseInt((iso.groups as any).y, 10);
                const m = parseInt((iso.groups as any).m, 10);
                const d = parseInt((iso.groups as any).d, 10);
                return new Date(y, m - 1, d);
            }
            const d = new Date(s);
            return isNaN(d.getTime()) ? null : d;
        };

        const startStr = (dbAnime as any).dbAnime as string | undefined;
        const startDate = parseDate(startStr || undefined);

        const lastEp = sortedEpisodes[sortedEpisodes.length - 1];
        const lastDate = parseDate(lastEp && lastEp.airdate ? lastEp.airdate : undefined);

        const matchedDate = parseDate(matchedEp.airdate) || null;
        const now = new Date();

        if (!matchedDate) {
            return { status: 'INVALID_INPUT', msg: `无法解析匹配集的放送日期: ${matchedEp.airdate}` };
        }

        // 如果有完整的起止时间，则使用区间判断；否则回退到简单的未来 24 小时 检查
        if (startDate && lastDate) {
            const msDay = 24 * 60 * 60 * 1000;
            const earliest = new Date(startDate.getTime() - 8 * msDay);
            const latest = new Date(lastDate.getTime() + 30 * msDay);

            if (matchedDate < earliest || matchedDate > latest) {
                // 计算距离最近区间边界的天数
                const diff = matchedDate < earliest
                    ? Math.ceil((earliest.getTime() - matchedDate.getTime()) / msDay)
                    : Math.ceil((matchedDate.getTime() - latest.getTime()) / msDay);
                return {
                    status: 'DATE_MISMATCH',
                    msg: `放送日期不在允许区间内（${formatDate(earliest)} ~ ${formatDate(latest)}）: ${formatDate(matchedDate)}`,
                    diffDays: diff,
                    episodeId: matchedEp.id
                };
            }
        } else {
            const diffHours = Math.ceil((matchedDate.getTime() - now.getTime()) / (1000 * 60 * 60));
            const maxFutureHours = 8 * 24;
            if (diffHours > maxFutureHours) {
                return {
                    status: 'DATE_MISMATCH',
                    msg: `放送日期在未来: ${formatDate(matchedDate)}`,
                    diffDays: Math.ceil(diffHours / 24),
                    episodeId: matchedEp.id
                };
            }
        }

        // 辅助：日期格式化（用于消息）
        function pad(n: number) { return n < 10 ? '0' + n : '' + n; }
        function formatDate(d: Date) {
            return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
        }
    }

    // 匹配成功
    return {
        status: 'MATCHED',
        episodeId: matchedEp.id, // Bangumi 章节唯一 ID
        ep: matchedEp.ep,
        sort: matchedEp.sort
    };
}
