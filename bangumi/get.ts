import axios from "axios";
import { env } from "../database/initDb.ts";

/**
 * 获取章节信息
 * @param episodesid 章节ID
 * @returns 章节信息
 */
export async function getEpisodeById(episodesid: number) {
    if (!episodesid) {
        throw new Error("无效的章节ID");
    }
    const response = await axios.get(`https://api.bgm.tv/v0/episodes/${episodesid}`, {
        headers: {
            "User-Agent": "XQAnimeBot/1.0 (https://github.com/xiaoqvan/XQAnimeBot)",
            Authorization: `Bearer ${env.data.BG_ACCESS_TOKEN}`,
        },
    });
    return response.data;
}