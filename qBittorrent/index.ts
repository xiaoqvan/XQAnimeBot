import { QbittorrentClient } from "./client.ts";
import logger from "@log/index.ts";
import { env } from "../database/initDb.ts";

let QBclient: QbittorrentClient | undefined = undefined;

/** 创建并登录 qBittorrent 客户端实例
 * @returns 已登录的 QBittorrent 实例
 */
async function createQBClient() {
  const client = new QbittorrentClient({
    baseURL: env.data.QBITTORRENT_HOST,
    username: env.data.QBITTORRENT_USERNAME,
    password: env.data.QBITTORRENT_PASSWORD,
    autoLogin: true
  });

  try {
    await client.login();
    return client;
  } catch (error) {
    logger.fatal(error, "[XQ动漫插件]qBittorrent链接失败: 请检查Web UI是否开启或密码是否正确。");
    process.exit(1);
  }
}

/** 获取已登录的 qBittorrent 客户端实例
 * 如果未登录或连接失效，则重新登录
 * @returns 已登录的 QBittorrent 实例
 */
export async function getQBClient() {
  if (!QBclient) {
    QBclient = await createQBClient();
  } else {
    try {
      const version = await QBclient.getAppVersion();
      logger.info("[XQ动漫插件]qBittorrent连接成功，版本: " + version);
    } catch (error) {
      logger.warn(error, "[XQ动漫插件]qBittorrent连接失效，尝试重新登录。");
      QBclient = await createQBClient();
    }
  }
  return QBclient;
}
