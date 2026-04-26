import logger from "@log/index.ts";
import axios from "axios";

const HOST = "https://api.bgm.tv";

const headers = {
    "Content-Type": "application/json",
    "User-Agent": "xiaoqvan/XQAnimeBot (https://github.com/xiaoqvan/XQAnimeBot)",
}

/**
 * 更新章节的收藏信息
 * @param token - BGM Token
 * @param episode_id - 章节ID
 * @param type - 收藏状态类型 (`0`: 未收藏, 1: 想看, 2: 看过, 3: 抛弃)
 * @returns 成功返回 true，失败抛出异常
 */
export async function updateEpisodeCollectionInfo(
    token: string,
    episode_id: number,
    type: number
): Promise<boolean> {
    try {
        await axios.put(
            `${HOST}/v0/users/-/collections/-/episodes/${episode_id}`,
            { type: type },
            {
                headers: {
                    Authorization: `Bearer ${token}`,
                    ...headers,
                },
            }
        );
        return true;
    } catch (error: any) {
        // 如果返回 400，表示该用户并未对所属条目进行收藏，因此无法对单集操作
        if (error.response && error.response.status === 400) {
            return false;
        }
        logger.error("修改章节收藏信息失败:", error);
        throw error;
    }
}

/**
 * 修改条目收藏状态
 * 修改条目收藏状态, 如果不存在则创建，如果存在则修改
 * 由于直接修改剧集条目的完成度可能会引起意料之外效果，只能用于修改书籍类条目的完成度。
 * 方法的所有请求体字段均可选
 * @param token - BGM Token
 * @param subject_id - 条目ID
 * @param type - 收藏状态类型 (1: 想看, 2: 看过, 3: 在看, 4: 搁置, 5: 抛弃)
 * @returns 成功返回 true，失败抛出异常
 */
export async function updateSubjectCollectionInfo(token: string, subject_id: number, type?: number) {
    try {
        await axios.post(`${HOST}/v0/users/-/collections/${subject_id}`,
            { type: type },
            {
                headers: {
                    Authorization: `Bearer ${token}`,
                    ...headers,
                },
            }
        );
        return true
    } catch (error) {
        logger.error(error, "修改条目收藏信息失败:");
        throw error
    }
}

/**
 * 获取用户信息
 * @param token - BGM Token
 * @returns 用户信息对象
 * @throws 当请求失败时抛出异常
 */
export async function getMe(token: string): Promise<user> {
    try {
        const response = await axios.get<user>(`${HOST}/v0/me`, {
            headers: {
                Authorization: `Bearer ${token}`,
                ...headers,
            },
        });
        return response.data;
    } catch (error) {
        logger.error(error, "获取用户信息失败:");
        throw error;
    }
}

type user = {
    /* 用户头像 */
    avatar: {
        large: string
        medium: string
        small: string
    },
    /* 个人签名 */
    sign: string,
    /* 用户名 */
    username: string,
    /* 昵称 */
    nickname: string,
    /* 用户ID */
    id: number,
    /* 用户组(1 = 管理员 - 2 = Bangumi 管理猿 - 3 = 天窗管理猿 - 4 = 禁言用户 - 5 = 禁止访问用户 - 8 = 人物管理猿 - 9 = 维基条目管理猿 - 10 = 用户 - 11 = 维基人) */
    user_group: number,
    /* 注册时间 */
    reg_time: string,
    /* 邮箱 */
    email: string,
    /* 用户设置的时区偏移，以小时为单位。比如 GMT+8（shanghai/beijing）为 8 */
    time_offset: number
}