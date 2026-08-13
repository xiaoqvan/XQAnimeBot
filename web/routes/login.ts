import type { FastifyInstance } from "fastify";
import { verifyKey } from "../key.ts";

/**
 * 鉴权相关路由（受 /api 鉴权钩子保护）。
 * 前端用 /api/auth/me 验证所填密钥是否有效并探明连接。
 */
export async function loginRoutes(app: FastifyInstance): Promise<void> {
    /** 校验当前请求携带的密钥是否有效（能到达这里说明已通过鉴权） */
    app.get("/api/auth/me", async (request) => {
        const auth = request.headers.authorization ?? "";
        const token = auth.startsWith("Bearer ") ? auth.slice(7) : "";
        const payload = verifyKey(token);
        return {
            ok: true,
            isDynamicKey: !!payload,
            expiresAt: payload ? new Date(payload.exp).toISOString() : undefined,
            time: new Date().toISOString(),
        };
    });
}
