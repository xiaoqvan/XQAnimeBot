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
