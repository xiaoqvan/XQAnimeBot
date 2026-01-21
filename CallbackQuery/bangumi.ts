import type { Client } from "tdl";
import { getBgmToken } from "../utils/getBgmToken.ts";
import { editMessageText } from "@TDLib/function/message.ts";
import { updateEpisodeCollectionInfo, updateSubjectCollectionInfo } from "../bangumi/set.ts";
import { answerCallbackQuery } from "@TDLib/function/index.ts";
export async function colorep(
    client: Client,
    chat_id: number,
    sender_user_id: number,
    message_id: number,
    queryId: string,
    params: string
) {
    const urlParams = new URLSearchParams(params);
    const animeId = urlParams.get("id");
    const episodeId = urlParams.get("ep");
    const at = urlParams.get("at");
    const et = urlParams.get("et");

    const tokenResult = await getBgmToken(sender_user_id);
    if (!tokenResult.success || !tokenResult.access_token) {
        editMessageText(client, chat_id, message_id, {
            text: tokenResult.message || "获取Bangumi访问令牌失败，请先绑定Bangumi账户。",
        });
        return;
    }
    if (!animeId || !episodeId || !at || !et) {
        editMessageText(client, chat_id, message_id, {
            text: "回调参数缺失，无法处理请求。",
        });
        return;
    }
    // 先更新动漫收藏状态
    await updateSubjectCollectionInfo(tokenResult.access_token, Number(animeId), Number(at));
    // 然后更新剧集状态
    // 注意这里没有对更新结果进行检查，可以根据需要添加
    await updateEpisodeCollectionInfo(tokenResult.access_token, Number(episodeId), Number(et));
    editMessageText(client, chat_id, message_id, {
        text: `动漫ID: ${animeId} 和 剧集ID: ${episodeId} 标记成功！\n动漫收藏状态已更新为: ${at}, 剧集状态已更新为: ${et}\n\n条目收藏状态类型 (1: 想看, 2: 看过, 3: 在看, 4: 搁置, 5: 抛弃)\n章节收藏状态类型 (0: 未收藏, 1: 想看, 2: 看过, 3: 抛弃)`,
    });
    await answerCallbackQuery(client, queryId, {
        text: `操作成功！`,
        show_alert: false,
    });
}