import type { message as messageType } from "tdlib-types";
import type { Client } from "tdl";

import { isUserAdmin } from "@TDLib/function/index.ts";
import { sendMessage } from "@TDLib/function/message.ts";
import { animeProcessor } from "../anime/AnimeProcessorManager.ts";
import { getQBClient } from "../qBittorrent/index.ts";
import { env } from "../database/initDb.ts";
import { getConfig } from "@db/config.ts";
import { getSmartDelayInfo } from "../utils/index.ts";

/** qBittorrent 种子状态对照表（英文状态码 → 人类可读中文） */
const QB_STATE_ZH: Record<string, string> = {
    downloading: "⬇️ 下载中",
    stalledDL: "🕐 等待下载（无速度）",
    metaDL: "🔍 获取种子元数据",
    forcedDL: "⬇️ 强制下载",
    checkingDL: "🔄 校验（下载）",
    pausedDL: "⏸️ 已暂停（下载）",
    stoppedDL: "⏹️ 已停止（下载）",
    queuedDL: "🕐 排队（下载）",
    uploading: "⬆️ 上传做种中",
    stalledUP: "⬆️ 做种等待（无速度）",
    forcedUP: "⬆️ 强制做种",
    checkingUP: "🔄 校验（做种）",
    pausedUP: "⏸️ 已暂停（做种）",
    stoppedUP: "✅ 做种完成",
    queuedUP: "🕐 排队（做种）",
    allocating: "💾 分配磁盘空间",
    moving: "🚚 移动文件",
    checkingResumeData: "🔄 校验恢复数据",
    error: "❌ 错误",
    missingFiles: "❌ 文件缺失",
    unknown: "❓ 未知",
};

/**
 * 将毫秒时长格式化为人类可读的中文字符串
 *
 * @param startTime - 任务开始时间
 * @returns 如 "3分42秒"、"1时5分" 或 "58秒" 等格式
 */
function formatDuration(startTime: Date): string {
    const seconds = Math.floor((Date.now() - startTime.getTime()) / 1000);
    if (seconds < 60) return `${seconds}秒`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}分${seconds % 60}秒`;
    return `${Math.floor(minutes / 60)}时${minutes % 60}分`;
}

/**
 * 将毫秒时长格式化为简短倒计时文本
 * @param ms - 毫秒数
 * @returns 如 "42秒"、"3分12秒"、"1时02分" 的文本
 */
function formatCountdown(ms: number): string {
    const seconds = Math.max(0, Math.floor(ms / 1000));
    if (seconds < 60) return `${seconds}秒`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}分${seconds % 60}秒`;
    return `${Math.floor(minutes / 60)}时${String(minutes % 60).padStart(2, "0")}分`;
}

/**
 * 处理 `/progress` 命令，向管理员展示当前所有活跃的动漫下载/处理进度
 *
 * 显示内容包括：
 * - 活跃 worker 数量和排队任务数量
 * - 每个活跃任务的番剧名、BT 标题（截断）、处理阶段和已耗时
 * - 若任务处于下载阶段（已填充 torrentHash），额外查询 qBittorrent 实时状态并展示
 *
 * 权限：仅管理员或 super_admin 可使用
 *
 * @param client - TDLib 客户端实例
 * @param message - 触发命令的 Telegram 消息对象
 */
export default async function progress(
    client: Client,
    message: messageType
): Promise<void> {
    const config = await getConfig("admin");
    const isAdmin = await isUserAdmin(
        client,
        Number(env.data.ADMIN_GROUP_ID),
        message.sender_id
    );
    const isBotAdmin =
        message.sender_id._ === "messageSenderUser" &&
        message.sender_id.user_id === config?.super_admin;

    if (!isAdmin && !isBotAdmin) return;

    const activeItems = animeProcessor.getProgress();
    const queueSize = animeProcessor.getQueueSize();
    const activeCount = animeProcessor.getActiveCount();
    const delayInfo = getSmartDelayInfo();
    const nextRefreshText = delayInfo.waitEnd.toLocaleString("zh-CN", {
        hour12: false,
    });
    const policyMinutes = Math.round(delayInfo.intervalMs / 60000);

    let text = `**📊 动漫处理进度**\n\n`;
    text += `🔄 活跃任务: **${activeCount}** | 📋 排队中: **${queueSize}**\n`;
    text += `🕒 下次RSS刷新: **${nextRefreshText}**（约 ${formatCountdown(delayInfo.waitMs)} 后）\n`;
    text += `⏱️ 当前smartDelay间隔: **${policyMinutes} 分钟**\n\n`;

    if (activeItems.length === 0) {
        text += queueSize === 0
            ? `_暂无任务，等待下一轮 RSS 抓取_`
            : `_Worker 正在启动，请稍后再查询_`;
    } else {
        // 批量查询 qBittorrent 状态，失败时优雅降级（不展示 qBT 状态，不影响主文本）
        const qbStateMap: Record<string, string> = {};
        const hasHashItems = activeItems.some((p) => p.torrentHash);
        if (hasHashItems) {
            try {
                const QBclient = await getQBClient();
                const torrents = await QBclient.getTorrents();
                for (const t of torrents) {
                    qbStateMap[t.hash.toLowerCase()] = t.state as string;
                }
            } catch {
                // qBittorrent 暂时不可用，跳过状态展示
            }
        }

        for (const [index, item] of activeItems.entries()) {
            const displayName = item.animeName
                ? `**${item.animeName}**`
                : "_（解析中）_";
            const shortTitle =
                item.title.length > 50
                    ? `${item.title.slice(0, 50)}…`
                    : item.title;

            text += `**${index + 1}.** ${displayName}\n`;
            text += `     🎬 \`${shortTitle}\`\n`;
            text += `     📍 阶段: \`${item.stage}\`\n`;
            text += `     ⏱️ 耗时: ${formatDuration(item.startTime)}\n`;

            if (item.torrentHash) {
                const rawState = qbStateMap[item.torrentHash.toLowerCase()];
                if (rawState) {
                    const stateLabel = QB_STATE_ZH[rawState] ?? `❓ ${rawState}`;
                    text += `     🧲 qBT: ${stateLabel}\n`;
                }
            }

            text += `\n`;
        }
    }

    await sendMessage(client, message.chat_id, {
        reply_to_message_id: message.id,
        text,
        link_preview: false,
    });
}
