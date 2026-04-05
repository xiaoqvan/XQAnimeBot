// 数据库中与 Bangumi 相关的类型声明

export interface BangumiUser {
    /** 内部自增 id（唯一主键，用于 oauth state） */
    id: number;
    /** Telegram 用户 ID（用于绑定，必需） */
    tgUserId: number | string;
    /** OAuth 访问凭证 */
    accessToken?: string;
    /** OAuth 刷新凭证 */
    refreshToken?: string;
    /** 过期时间，UTC 时间字符串 */
    expiresIn?: number;
    /** OAuth Token 类型 */
    tokenType?: string;
    /** Bangumi 平台返回的用户 ID（bgm.tv 的 user id） */
    bgmUserId?: number | string;
    /** OAuth scope 字符串 */
    scope?: string;
    /** 最近一次授权时间（或刷新时间） */
    authTime?: Date | string;
    createdAt?: Date | string;
    updatedAt?: Date | string;
    // 允许存放额外未列出的字段以兼容现有代码
    [key: string]: any;
}

// 用于 API 返回/调用的简化 BGM 用户类型（可选）
export interface BgmApiUser {
    id: number;
    username: string;
    nickname?: string;
    avatar?: {
        large?: string;
        medium?: string;
        small?: string;
    };
    email?: string;
    sign?: string;
}

export type BangumiUserDoc = BangumiUser;

export type bangumiAnime = {
    /** 条目 ID */
    id: number;
    /** 条目类型
     *  1 = 书籍, 2 = 动画, 3 = 音乐, 4 = 游戏, 6 = 三次元（没有 5）
     */
    type: 1 | 2 | 3 | 4 | 6;
    /** 原名 */
    name: string;
    /** 中文名 */
    name_cn?: string;
    /** 简介/剧情概述 */
    summary?: string;
    /** 放送日期 */
    date?: string;
    /** 平台，如 TV、Web 等 */
    platform?: string;
    /** 图片资源 */
    images?: {
        /** 小图 */
        small?: string;
        /** 网格图 */
        grid?: string;
        /** 大图 */
        large?: string;
        /** 中图 */
        medium?: string;
        /** 常用尺寸图 */
        common?: string;
    };
    /** 标签列表 */
    tags?: {
        /** 标签名 */
        name: string;
        /** 标签计数 */
        count?: number;
        /** 总计数（备用字段） */
        total_cont?: number;
    }[];
    /** 信息框，包含各种属性 */
    infobox?: infobox[];
    /** 评分信息 */
    rating?: {
        /** 排名 */
        rank?: number;
        /** 总评分人数 */
        total?: number;
        /** 各分数的人数统计 */
        count?: {
            [score: string]: number; // 1-10 的分数人数
        };
        /** 平均分 */
        score?: number;
    };
    /** bgm.tv中的章节数量 */
    total_episodes?: number;
    /** 收藏状态 */
    collection?: {
        /** 搁置 */
        on_hold?: number;
        /** 放弃 */
        dropped?: number;
        /** 想看 */
        wish?: number;
        /** 已收藏 */
        collect?: number;
        /** 正在看 */
        doing?: number;
    };

    /** 当前总集数 */
    eps?: number;
    /** 由维基人维护的 tags */
    meta_tags?: string[];
    /** 书籍条目的册数 */
    volumes?: number;
    /** 是否为书籍系列的主条目 */
    series?: boolean;
    /** 是否锁定 */
    locked?: boolean;
    /** 是否为成人向 */
    nsfw?: boolean;
};

export type bangumiSearchResult = {
    /** 搜索结果数组 */
    data?: bangumiAnime[];
    /** 总结果数 */
    total: number;
    /** 每页限制数 */
    limit: number;
    /** 偏移量 */
    offset: number;
};


export type infobox = {
    /** 属性名 */
    key: string;
    /** 属性值，可能是字符串或数组（数组项可为纯字符串、{v: string} 或 {k: string, v: string}） */
    value: string | (string | { v: string } | { k: string; v: string })[];
};
