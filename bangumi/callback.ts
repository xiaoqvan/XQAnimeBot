import axios from "axios";
import { createBangumiUser } from "../database/create.ts";
import { updateBangumiUser } from "../database/update.ts";
import { env } from "../database/initDb.ts";

// 请在此处填写你的应用信息
const REDIRECT_URI = "https://anime.xiaoqvan.top";

const HOST = "https://bgm.tv";

/**
 * 1. 获取 Bangumi 授权页链接
 * 并在数据库中创建一个新用户记录，将生成的 ID 作为 state 参数
 * @param tgUserId Telegram 用户 ID，用于绑定
 * @returns 授权页 URL
 */
export async function getAuthUrl(tgUserId: number) {
    // 创建或更新用户记录（生成内部自增 id，同时保存唯一 tgUserId）
    const id = await createBangumiUser({ tgUserId });
    const state = id.toString();

    const query = new URLSearchParams({
        client_id: env.data.BG_APP_ID,
        response_type: "code",
        redirect_uri: REDIRECT_URI,
        state: state,
    });

    return `${HOST}/oauth/authorize?${query.toString()}`;
}

/**
 * 2. 使用 code（验证代码）换取 Access Token
 * @param code 回调获取的验证代码
 * @param state 回调获取的 state (即数据库中的 id)
 * @returns Access Token 响应数据
 */
export async function getAccessToken(code: string, state?: string) {
    try {
        const params = new URLSearchParams();
        params.append("grant_type", "authorization_code");
        params.append("client_id", String(env.data.BG_APP_ID));
        params.append("client_secret", String(env.data.BG_APP_SECRET));
        params.append("code", code);
        params.append("redirect_uri", REDIRECT_URI);
        params.append("state", state || "");

        const response = await axios.post(`${HOST}/oauth/access_token`, params, {
            headers: {
                "Content-Type": "application/x-www-form-urlencoded",
                "User-Agent": "XQAnimeBot/1.0 (https://github.com/xiaoqvan/XQAnimeBot)",
            },
        });

        const data = response.data;

        // 如果提供了 state (user id)，则更新数据库中的用户信息
        if (state) {
            const userId = parseInt(state, 10);
            if (!isNaN(userId)) {
                await updateBangumiUser(userId, {
                    accessToken: data.access_token,
                    refreshToken: data.refresh_token,
                    expiresIn: data.expires_in,
                    tokenType: data.token_type,
                    bgmUserId: data.user_id, //邦定 Bangumi 提供的 user_id
                    scope: data.scope,
                    authTime: new Date(),
                });
            }
        }

        return data;
    } catch (error: any) {
        if (error.response) {
            throw new Error(`getAccessToken failed: ${error.response.status} ${JSON.stringify(error.response.data)}`);
        }
        throw error;
    }
}

/**
 * 3. 授权有效期刷新
 * @param refresh_token 之前获取的 refresh token
 * @param userId 可选，如果提供则自动更新数据库
 * @returns 新的 Token 信息
 */
export async function refreshAccessToken(refresh_token: string, userId?: number) {
    try {
        const params = new URLSearchParams();
        params.append("grant_type", "refresh_token");
        params.append("client_id", String(env.data.BG_APP_ID));
        params.append("client_secret", String(env.data.BG_APP_SECRET));
        params.append("refresh_token", refresh_token);
        params.append("redirect_uri", REDIRECT_URI);

        const response = await axios.post(`${HOST}/oauth/access_token`, params, {
            headers: {
                "Content-Type": "application/x-www-form-urlencoded",
                "User-Agent": "XQAnimeBot/1.0 (https://github.com/xiaoqvan/XQAnimeBot)",
            },
        });

        const data = response.data;

        // 更新数据库
        if (userId) {
            await updateBangumiUser(userId, {
                accessToken: data.access_token,
                refreshToken: data.refresh_token,
                expiresIn: data.expires_in,
                tokenType: data.token_type,
                scope: data.scope,
                authTime: new Date(), // 更新刷新时间
            });
        }

        return data;
    } catch (error: unknown) {
        throw error;
    }
}

/**
 * 4. 查询授权信息
 * @param access_token Access Token
 * @returns 授权状态信息
 */
export async function getTokenStatus(access_token: string) {
    try {
        const response = await axios.post(
            `${HOST}/oauth/token_status`,
            new URLSearchParams({
                access_token: access_token,
            }).toString(),
            {
                headers: {
                    "Content-Type": "application/json",
                    "User-Agent": "xiaoqvan/XQAnimeBot (https://github.com/xiaoqvan/XQAnimeBot)",
                },
            }
        );

        return response.data;
    } catch (error) {
        throw error;
    }
}
