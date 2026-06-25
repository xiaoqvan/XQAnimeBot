import type { animeItem } from "./rss.d.ts";
import type { anime as animeType } from "./anime.d.ts";
import type { EpisodeMatchResult } from "../utils/matcher.ts";
import type { MatchResult } from "../bangumi/bangumiAgent.ts";
import type { albumMessageType } from "./message.d.ts";

/**
 * 待审核番剧文档
 *
 * AI 匹配完成后，先发视频到动漫频道，再将审核信息保存到此集合。
 * 回调只携带 pendingReviewId，所有关键数据都从此文档读取。
 */
export type PendingReviewDoc = {
    /** 自增 ID（回调数据中携带） */
    id: number;

    /** 创建时间 */
    createdAt: Date;

    /** 审核状态 */
    status: "pending" | "approved" | "rejected";

    /** 原始 RSS 条目数据 */
    item: animeItem;

    /** 匹配到的番剧信息 */
    anime: animeType;

    /** 匹配到的章节 ID */
    episodeId: number;

    /** 展示用的集数编号（sort 字段） */
    episodeSort: number;

    /** 已发送到动漫频道的消息数据 */
    sentMessages: albumMessageType[];

    /** 发送的主消息 IDs */
    primaryMessage: {
        chat_id: number;
        message_id: number;
    };

    /** LLM 匹配详情 */
    matchDetail?: MatchResult;

    /** 集数匹配结果（如果 episodeMatch 没成功才需要） */
    episodeMatch?: EpisodeMatchResult;
};
