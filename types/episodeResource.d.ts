import type { messageType, albumMessageType } from "./message.d.ts"

export type EpisodeResourceDoc = {
    /** Mongo ObjectId */
    id?: string;

    /** 所属动漫 */
    anime_id: number;

    /** 集数 */
    episode: string;

    /** 对应 eps.id */
    episodeId?: number;

    /** 字幕组 */
    groups: string[];

    /** 标题 */
    title: string;

    /** 名称（解析用） */
    names?: string[];

    /** TG 单条消息 */
    message?: messageType;

    /** TG 分段消息 */
    messages?: albumMessageType[];

    /** 单视频 */
    videoid?: string;
    unique_id?: string;

    /** 缓存ID */
    cache_id?: number | string;

    /** 来源 */
    source?: string;

    /** 创建时间 */
    createdAt: Date;
};