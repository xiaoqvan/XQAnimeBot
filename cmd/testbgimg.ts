import type { message as messageType } from "tdlib-types";
import type { Client } from "tdl";
import { sendMessage } from "@TDLib/function/message.ts";
import { isUserAdmin } from "@TDLib/function/index.ts";
import { generateBangumiNavImage } from "../img/generateBangumiImage.ts";
import { getSubjectById, getEpisodeInfo } from "../bangumi/get.ts";
import { getAnimeById, getResourcesByAnimeId } from "../database/query.ts";
import { getConfig } from "@db/config.ts";
import { env } from "../database/initDb.ts";
import { navmegtext } from "../anime/text.ts";
import { extractInfoFromInfobox } from "../utils/buildAnimeinfo.ts";
import type { anime as animeType } from "../types/anime.d.ts";
import logger from "@log/index.ts";

/**
 * 处理 /testbgimg 命令
 * 测试生成 Bangumi 番剧信息卡片图片，仅限管理员/主人使用
 *
 * 用法: /testbgimg <subjectId>
 */
export default async function handleTestBgImg(
    client: Client,
    message: messageType,
    commandParts: string[] | undefined
) {
    // ── 权限校验 ──
    const isAdmin = await isUserAdmin(
        client,
        Number(env.data.ADMIN_GROUP_ID),
        message.sender_id
    );
    const config = await getConfig("admin");
    const isBotAdmin =
        message.sender_id._ === "messageSenderUser" &&
        message.sender_id.user_id === config?.super_admin;

    if (!isAdmin && !isBotAdmin) {
        return;
    }

    // ── 参数校验 ──
    if (!commandParts || commandParts.length < 1) {
        await sendMessage(client, message.chat_id, {
            reply_to_message_id: message.id,
            text: "❌ 用法错误！\n\n**正确用法**:\n`/testbgimg <subjectId>` - 测试生成指定番剧的图片卡片\n\n**示例**:\n`/testbgimg 515594`",
            link_preview: true,
        });
        return;
    }

    const subjectId = Number(commandParts[0]);
    if (Number.isNaN(subjectId) || subjectId <= 0) {
        await sendMessage(client, message.chat_id, {
            reply_to_message_id: message.id,
            text: "❌ 无效的番剧 ID，请传入数字 ID，例如：`/testbgimg 515594`",
            link_preview: true,
        });
        return;
    }

    // ── 发送处理中提示 ──
    const tipMsg = await sendMessage(client, message.chat_id, {
        reply_to_message_id: message.id,
        text: `⏳ 正在获取 Bangumi 条目 #${subjectId} 的信息并生成图片...`,
        link_preview: true,
    });
    if (!tipMsg) return;

    try {
        // ── 获取 Bangumi API 数据 ──
        const subjectData = await getSubjectById(subjectId);
        let episodeInfo = null;
        try {
            episodeInfo = await getEpisodeInfo(subjectId);
        } catch (epErr) {
            logger.warn(epErr, `获取集数信息失败，将继续: subject_id=${subjectId}`);
        }

        // ── 尝试从本地数据库获取番剧记录（可选，用于补充 tags） ──
        let localAnime = null;
        try {
            localAnime = await getAnimeById(subjectId);
        } catch {
            // 本地不存在不影响生成
        }

        // ── 获取活跃字幕组列表 ──
        let fansubs: string[] = [];
        try {
            const grouped = await getResourcesByAnimeId(subjectId);
            fansubs = Object.keys(grouped).filter(Boolean);
        } catch { /* ignore */ }

        // ── 生成图片 ──
        const imgResult = await generateBangumiNavImage({
            subjectData,
            episodeInfo,
            anime: localAnime,
            fansubs,
        });

        // ── 构建临时番剧对象用于生成导航文本 ──
        const infobox = extractInfoFromInfobox(subjectData.infobox || []);
        const tempAnime: animeType = {
            id: subjectData.id,
            name: subjectData.name || "",
            name_cn: subjectData.name_cn || infobox.name,
            image: subjectData.images?.large || subjectData.images?.medium || "",
            summary: subjectData.summary || "",
            tags: localAnime?.tags?.length
                ? localAnime.tags
                : (subjectData.tags || []).map((t) => t.name),
            episode: String(episodeInfo?.total ?? subjectData.total_episodes ?? infobox.episodeCount ?? "未知"),
            score: subjectData.rating?.score ?? undefined,
            airingStart: infobox.broadcastStart || subjectData.date,
            airingDay: infobox.broadcastDay,
        };

        // ── 生成导航文本（首条消息，含评分/简介/标签） ──
        let navText = "";
        try {
            const navTexts = await navmegtext(client, tempAnime);
            navText = navTexts[0] ?? "";
        } catch (navErr) {
            logger.warn(navErr, "生成导航文本失败，将使用简易文本");
        }

        // 若 navmegtext 未生成有效文本，构建简易版本
        if (!navText) {
            const score = subjectData.rating?.score;
            const scoreStr = score != null ? `评分: ${score}` : "";
            navText =
                `${subjectData.name_cn || subjectData.name}\n` +
                `> 原名: ${subjectData.name}\n` +
                `> ID: ${subjectData.id}\n` +
                `> ${scoreStr}`;
        }

        // ── 构建回复消息文本 ──
        const statusInfo =
            imgResult.file_id
                ? "✅ **缓存命中**"
                : "🆕 **新生成**";

        const hashShort = imgResult.hash.slice(0, 12);
        const detailInfo =
            `**条目**: ${subjectData.name_cn || subjectData.name} (#${subjectData.id})\n` +
            `**评分**: ${subjectData.rating?.score ?? "无"} (${subjectData.rating?.total ?? 0} votes)\n` +
            `**集数**: ${episodeInfo?.total ?? "无"}\n` +
            `**哈希**: \`${hashShort}...\`\n` +
            `**状态**: ${statusInfo}`;

        // ── 发送图片 + 导航文本 caption ──
        const caption = `${detailInfo}\n\n---\n${navText}`;

        if (imgResult.file_id) {
            await sendMessage(client, message.chat_id, {
                text: caption,
                media: { photo: { id: imgResult.file_id } },
            });
        } else if (imgResult.path) {
            await sendMessage(client, message.chat_id, {
                text: caption,
                media: { photo: { path: imgResult.path } },
            });
            try {
                const fs = await import("fs/promises");
                await fs.unlink(imgResult.path);
            } catch { /* ignore */ }
        } else {
            await sendMessage(client, message.chat_id, {
                reply_to_message_id: message.id,
                text: "❌ 图片生成失败，未返回有效结果",
                link_preview: true,
            });
        }
    } catch (err) {
        const errMsg = err instanceof Error ? err.message : String(err);
        logger.error(err, `testbgimg 失败: subject_id=${subjectId}`);
        await sendMessage(client, message.chat_id, {
            reply_to_message_id: message.id,
            text: `❌ 生成失败: ${errMsg}`,
            link_preview: true,
        });
    } finally {
        // 删除提示消息
        if (tipMsg) {
            try {
                await client.invoke({
                    _: "deleteMessages",
                    chat_id: message.chat_id,
                    message_ids: [tipMsg.id],
                    revoke: true,
                });
            } catch { /* ignore */ }
        }
    }
}
