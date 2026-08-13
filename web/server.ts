import Fastify, { type FastifyInstance } from "fastify";
import cors from "@fastify/cors";
import logger from "@log/index.ts";
import type { Client } from "tdl";
import { webConfig, getApiBaseUrl } from "./config.ts";
import { registerAuth } from "./auth.ts";
import { registerRoutes } from "./routes/index.ts";

let app: FastifyInstance | null = null;

/** 持有 Bot 的 TDLib client，供 Web 后端执行 BT 任务（下载/发送等）使用 */
let botClient: Client | null = null;

/** 获取当前持有的 Bot client（Web 任务执行用） */
export function getBotClient(): Client | null {
    return botClient;
}

/**
 * 启动 Web 服务（仅 Fastify API，不托管前端）。
 *
 * - 前端为独立工程，由开发者本地单独运行/部署（Vite），后端不托管、不运行前端。
 * - 仅监听本机 127.0.0.1，如需暴露到局域网请设置 WEB_HOST=0.0.0.0。
 * - API 鉴权：所有 /api/* 需 Bearer 动态密钥（主人/管理员用 Bot 命令 /web 获取）。
 * @param client 可选：Bot 的 TDLib client，供 BT 任务（addanime/addnewanime）执行使用
 */
export async function startWebServer(client?: Client): Promise<FastifyInstance> {
    if (app) {
        return app;
    }

    if (client) {
        botClient = client;
    }

    app = Fastify({
        logger: false,
        bodyLimit: 10 * 1024 * 1024, // 10MB
    });

    // 始终启用 CORS：前端独立部署并配置后端地址（跨域）。
    // 安全性由密钥鉴权（/api/auth/me + Bearer）保障；允许任意来源与 Authorization 头。
    await app.register(cors, {
        origin: true,
        allowedHeaders: ["Content-Type", "Authorization"],
    });

    // 简单 Bearer 鉴权
    registerAuth(app);

    // API 路由
    await registerRoutes(app);

    await app.listen({ host: webConfig.host, port: webConfig.port });

    const base = getApiBaseUrl();
    logger.info(`[Web] API 服务已启动: ${base}/api`);
    logger.info(
        `[Web] 前端为独立工程，请本地运行 Vite (web/frontend) 并填入此 API 地址连接`
    );
    logger.info(
        `[Web] 生成连接密钥请使用 Bot 命令 /web（主人/管理员可用，密钥 24 小时有效）`
    );

    return app;
}

/**
 * 关闭 Web 服务
 */
export async function stopWebServer(): Promise<void> {
    if (!app) {
        return;
    }
    try {
        await app.close();
    } catch (err) {
        logger.warn(err, "[Web] 关闭 Web 服务时出错");
    }
    app = null;
    botClient = null;
    logger.info("[Web] Web 服务已关闭");
}
