import type { messageType, navMessageType } from "./message.d.ts"

export type anime = {
    /** 动漫ID */
    id: number;
    /** 中文名 */
    name_cn?: string;
    /** 原名 */
    name: string;
    /** 其他名称 */
    names?: string[];
    /** 图片 */
    image: string;
    /** 简介 */
    summary?: string;
    /** 标签 */
    tags?: string[];
    /** 总集数 */
    episode?: string;
    /** 评分 */
    score?: number | string;
    /** 新版导航频道消息 */
    navMessage?: messageType;
    /** 多条资源导航消息 */
    navVideoMessage?: navMessageType[];
    /** 是否为R18 */
    r18?: boolean;
    /** 放送星期 */
    airingDay?: string;
    /** 放送开始时间 */
    airingStart?: string;
    /** 数据库中创建时间 */
    createdAt?: Date;
    /** 导航封面图片内容的哈希值（用于去重判断是否需要重新上传） */
    navImageHash?: string;
    /** 数据库中最后更新时间 */
    updatedAt?: Date;
};
