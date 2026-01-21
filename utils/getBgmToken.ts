import { getBangumiUserByTgId } from "../database/query.ts";
import { refreshAccessToken } from "../bangumi/callback.ts";

/**
 * 获取 Bangumi 访问令牌，必要时刷新
 * @param tgUserId Telegram 用户 ID
 * @returns 包含访问令牌或错误信息的对象
 */
export async function getBgmToken(tgUserId: number): Promise<{ success: boolean; access_token?: string; message?: string }> {
    const user = await getBangumiUserByTgId(tgUserId);
    if (!user) {
        return { success: false, message: "未绑定 Bangumi 账户\n请先使用 /bindbangumi 进行Bangumi账户绑定后再操作！" };
    }
    if (!user.accessToken) {
        return { success: false, message: "Bangumi 账户未授权\n请先使用 /bindbangumi 进行Bangumi账户绑定后再操作！" };
    }

    // 如果知道授权时间和有效期，则检查是否过期
    if (user.authTime && user.expiresIn) {
        const authMs = new Date(user.authTime).getTime();
        const expiresMs = Number(user.expiresIn) * 1000;
        const expiresAt = authMs + expiresMs;

        // 提前一分钟刷新以避免临界条件
        const now = Date.now();
        if (now >= expiresAt - 60 * 1000) {
            // 需要刷新
            if (!user.refreshToken) {
                return { success: false, message: "访问令牌已过期且没有 refresh token，可以尝试退出 /exitbangumi 后重新使用 /bindbangumi 绑定 " };
            }

            try {
                const data = await refreshAccessToken(String(user.refreshToken), user.id as number);
                return { success: true, access_token: data.access_token };
            } catch (err: any) {
                return { success: false, message: `当前授权token已过期且刷新失败：${err?.message || String(err)}\n\n可以尝试使用退出 /exitbangumi 后\n重新使用 /bindbangumi 绑定 ` };
            }
        }
    }

    return { success: true, access_token: user.accessToken };
}