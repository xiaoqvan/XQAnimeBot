import type { FastifyRequest, FastifyReply, FastifyInstance } from "fastify";
import { webConfig } from "./config.ts";
import { verifyKey } from "./key.ts";

/**
 * Bearer 鉴权：优先接受"动态签名密钥"（HMAC，24h 有效，见 key.ts），
 * 也兼容旧的 WEB_API_TOKEN（本地/内网主令牌，不过期）。
 *
 * - 所有 /api/*（除 /api/health 外）必须带 `Authorization: Bearer <key>`。
 */
export function registerAuth(app: FastifyInstance): void {
    const masterToken = webConfig.apiToken;

    app.addHook("onRequest", async (request: FastifyRequest, reply: FastifyReply) => {
        // 静态资源与健康检查不需要鉴权
        if (
            !request.url.startsWith("/api") ||
            request.url === "/api/health"
        ) {
            return;
        }

        const auth = request.headers.authorization ?? "";
        const token = auth.startsWith("Bearer ") ? auth.slice(7) : "";

        // 校验动态签名密钥（24h 有效）
        if (token && verifyKey(token)) {
            return;
        }

        // 兼容主令牌
        if (masterToken && token === masterToken) {
            return;
        }

        await reply.code(401).send({ error: "未授权：请提供有效的访问密钥" });
    });
}
