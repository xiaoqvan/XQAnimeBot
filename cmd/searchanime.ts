import type { message as messageType } from "tdlib-types";
import { sendMessage } from "@TDLib/function/message.ts";
import { searchAnime } from "../database/query.ts";
import logger from "@log/index.ts";
import type { Client } from "tdl";

/**
 * 处理 /searchanime 命令
 * @param {object} message - 消息对象
 * @param {string[]} commandParts - 命令参数数组（不包含命令头，只包含参数）
 */
export default async function handleSearchAnime(
  client: Client,
  message: messageType,
  commandParts: string[] | undefined
) {
  // 用法：/searchanime "关键词" 或 /searchanime <id>
  // 注意：`commandParts` 现在不包含命令头，只包含命令后的参数
  if (!commandParts || commandParts.length < 1) {
    sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text:
        "请提供搜索关键词\n只能搜索到频道中已有的番剧\n\n**用法：**\n" +
        '`/searchanime "关键词"` - 按关键词搜索（至少2个字符）\n\n' +
        "**示例：**\n" +
        '`/searchanime "CITY RHE ANIMATION"`\n' +
        "`/searchanime 日常`\n\n 注意 /命令和关键词之间必须有空格",
      link_preview: true,
    });
    return;
  }

  // 获取搜索查询（支持带引号的关键词或直接的ID/关键词）
  let searchQuery;
  if (commandParts.length === 1) {
    // 单个参数，可能是ID或关键词
    searchQuery = commandParts[0];
  } else {
    // 多个参数，组合成完整的关键词
    searchQuery = commandParts.join(" ");
  }

  // 移除可能的引号
  searchQuery = searchQuery.replace(/^["']|["']$/g, "").trim();

  if (!searchQuery) {
    sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: "搜索关键词不能为空，请提供有效的关键词。",
      link_preview: true,
    });
    return;
  }

  try {
    // 执行搜索
    if (!searchQuery || searchQuery.trim().length < 2) {
      sendMessage(client, message.chat_id, {
        reply_to_message_id: message.id,
        text: "关键词搜索至少需要2个字符。\n\n请提供更长的关键词进行搜索。",
        link_preview: true,
      });
      return;
    }
    const results = await searchAnime(searchQuery);

    if (results.length === 0) {
      sendMessage(client, message.chat_id, {
        reply_to_message_id: message.id,
        text: `未找到与 "${searchQuery}" 相关的动漫。\n\n请尝试其他关键词进行搜索。`,
        link_preview: true,
      });
      return;
    }

    // 限制显示结果数量，避免消息过长
    const maxResults = 20;
    const displayResults = results.slice(0, maxResults);
    const hasMoreResults = results.length > maxResults;

    // 构建搜索结果文本
    let resultText = `🔍 **搜索结果** (找到 ${results.length} 个匹配项)\n\n`;
    resultText += `**搜索关键词：** ${searchQuery}\n\n`;

    displayResults.forEach((anime, index) => {
      resultText += `**${index + 1}.** \`${anime.id}\`\n`;
      resultText += `**名称：** ${anime.name || "无"}\n`;
      resultText += `**中文名：** ${anime.name_cn || "无"}\n`;

      // 添加频道消息链接（如果存在）
      const messageLink = anime.navMessage?.link;
      if (messageLink) {
        resultText += `**频道链接：** [查看详情](${messageLink})\n`;
      }

      resultText += `\n`;
    });

    if (hasMoreResults) {
      resultText += `⚠️ **注意：** 还有 ${results.length - maxResults
        } 个结果未显示，请使用更具体的关键词缩小搜索范围。\n\n`;
    }

    sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: resultText,
      link_preview: false,
    });

    // 记录搜索日志
    logger.info(`动漫搜索: "${searchQuery}" - 找到 ${results.length} 个结果`);
  } catch (err) {
    // TypeScript's `catch` variable is `unknown` by default. Narrow it to Error for safe access.
    const error: Error =
      err instanceof Error
        ? err
        : new Error(typeof err === "string" ? err : String(err));

    logger.error("搜索动漫时发生错误:", error);

    let errorMessage = "❌ 搜索动漫时发生错误。";

    // 针对特定错误提供更友好的提示（安全地使用 error.message）
    if (error.message && error.message.includes("至少需要2个字符")) {
      errorMessage =
        "❌ 关键词搜索至少需要2个字符。\n\n请提供更长的关键词进行搜索。";
    } else if (error.message) {
      errorMessage += `\n\n**错误信息：** ${error.message}`;
    }

    sendMessage(client, message.chat_id, {
      reply_to_message_id: message.id,
      text: errorMessage,
      link_preview: false,
    });
  }
}
