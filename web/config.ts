/**
 * Web 服务配置
 *
 * 可通过环境变量覆盖：
 * - WEB_HOST        监听绑定地址（默认 0.0.0.0，监听所有网卡，默认开放公网/局域网；
 *                    仅本机访问可改回 127.0.0.1）
 * - WEB_PORT        监听端口（默认 3780）
 * - WEB_PUBLIC_URL  后端对外地址（公网域名/IP，供 Bot /web 命令展示给前端连接；
 *                    公网部署建议设置；缺省退回 http://host:port/）
 * - WEB_API_TOKEN  主令牌（可选，兼容旧鉴权；动态密钥始终可用）
 * - WEB_CORS       是否允许跨域（true/false，默认 false）
 *
 * 前端为独立工程（web/frontend），由开发者本地用 Vite 运行/部署，
 * 后端只提供 API，不托管、不运行前端。
 */
export const webConfig = {
    host: process.env.WEB_HOST ?? "0.0.0.0",
    port: Number(process.env.WEB_PORT ?? 3780),
    publicUrl: process.env.WEB_PUBLIC_URL ?? "",
    apiToken: process.env.WEB_API_TOKEN ?? "",
    cors: (process.env.WEB_CORS ?? "false") === "true",
};

/**
 * 计算后端 API 根地址（供 Bot /web 命令展示给前端连接使用）。
 *
 * 注意：监听地址为 0.0.0.0 时它是不可访问的占位地址，公网部署请务必设置
 * WEB_PUBLIC_URL（公网域名/IP），否则 /web 展示的是无效地址。
 */
export function getApiBaseUrl(): string {
    // 优先使用公网对外地址（WEB_PUBLIC_URL），否则退回监听地址
    if (webConfig.publicUrl) {
        return webConfig.publicUrl.replace(/\/+$/, "");
    }
    if (webConfig.host === "0.0.0.0") {
        // 监听所有接口时无法回显具体地址，提示用户配置公网地址
        return `http://<YOUR_PUBLIC_IP_OR_DOMAIN>:${webConfig.port}`;
    }
    return `http://${webConfig.host}:${webConfig.port}`;
}
