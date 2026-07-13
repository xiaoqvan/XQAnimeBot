import axios from "axios";
import { Plugin } from "@plugin/BasePlugin.ts";
import type { Client } from "tdl";
import { sendMessage } from "@TDLib/function/message.ts";
import type { updateNewMessage } from "tdlib-types";
import { generateImage } from "@function/genImg.ts";
import fs from "fs/promises";
import { updateImgCache } from "@db/update.ts";
import logger from "@log/index.ts";

interface BangumiEpisode {
    id: number;
    ep: number;
    sort: number;
    name: string;
    name_cn: string;
    airdate: string;
    type: number;
}

interface BangumiEpisodesResponse {
    data: BangumiEpisode[];
    total: number;
    limit: number;
    offset: number;
}

interface BangumiSubject {
    id: number;
    name: string;
    name_cn: string;
    images: { small?: string; grid?: string; large?: string; medium?: string; common?: string };
    summary: string;
    date: string;
    platform: string;
    total_episodes: number;
    eps: number;
    rating: { rank: number; total: number; count: Record<number, number>; score: number };
    tags: { name: string; count: number }[];
    infobox: { key: string; value: string | { v: string }[] }[];
    collection: { on_hold: number; dropped: number; wish: number; collect: number; doing: number };
    volumes: number;
    series: boolean;
    locked: boolean;
    nsfw: boolean;
    type: number;
}

/** 将 infobox 数组转为 key-value 映射，处理 value 可能是字符串或对象数组 */
function getInfoboxMap(infobox: BangumiSubject["infobox"]): Record<string, string> {
    const map: Record<string, string> = {};
    for (const item of infobox) {
        if (typeof item.value === "string") {
            map[item.key] = item.value;
        } else if (Array.isArray(item.value)) {
            map[item.key] = item.value.map(v => v.v).join(" / ");
        }
    }
    return map;
}

/** 提取精选 Staff 信息 */
const STAFF_KEYS = ["原作", "导演", "动画制作", "音乐", "系列构成", "脚本"];
function getDisplayStaff(infoboxMap: Record<string, string>): { key: string; value: string; align?: string }[] {
    return STAFF_KEYS.map((key, idx) => ({
        key,
        value: infoboxMap[key] || "",
        align: idx % 2 === 1 ? "right" : undefined,
    }));
}

/** 评分等级标签 */
function getRatingLabel(score: number): string {
    if (score >= 8.5) return "神作";
    if (score >= 7.5) return "力荐";
    if (score >= 6.5) return "推荐";
    if (score >= 5.5) return "还行";
    if (score >= 4.5) return "不过不失";
    if (score >= 3.5) return "较差";
    if (score >= 2.5) return "差";
    if (score >= 1.5) return "很差";
    return "不忍直视";
}

export default class BangumiPlugin extends Plugin {
    type = "general";
    name = "bangumi";
    version = "1.0.0";
    description = "Bangumi 番组计划条目信息查询";

    constructor(client: Client) {
        super(client);
        this.cmdHandlers = {
            bangumi: {
                description: "查询 Bangumi 动漫条目信息，用法 /bangumi <条目ID>",
                handler: async (message: updateNewMessage, _args) => {
                    const idStr = _args?.[0];
                    if (!idStr || isNaN(Number(idStr))) {
                        await sendMessage(this.client, message.message.chat_id, {
                            text: "请提供有效的 Bangumi 条目 ID，例如 /bangumi 515594",
                        });
                        return;
                    }
                    const subjectId = Number(idStr);

                    try {
                        // 先发送提示
                        const tipMsg = await sendMessage(this.client, message.message.chat_id, {
                            text: `⏳ 正在获取 Bangumi 条目 #${subjectId} 的信息...`,
                        });

                        const bgmHeaders = { 'User-Agent': 'TDLib-Bangumi-Plugin/1.0 (https://github.com/user/tdlib)' };

                        // 获取条目信息（带重试）
                        let subjectData: BangumiSubject | null = null;
                        for (let retry = 0; retry < 3; retry++) {
                            try {
                                const { data } = await axios.get<BangumiSubject>(
                                    `https://api.bgm.tv/v0/subjects/${subjectId}`,
                                    { timeout: 30000, headers: bgmHeaders }
                                );
                                subjectData = data;
                                break;
                            } catch (err) {
                                if (retry === 2) {
                                    throw err;
                                }
                            }
                        }

                        if (!subjectData || !subjectData.id) {
                            await sendMessage(this.client, message.message.chat_id, {
                                text: `条目 #${subjectId} 未找到或 API 返回异常。`,
                            });
                            return;
                        }
                        const data = subjectData;

                        // 数据处理
                        const infoboxMap = getInfoboxMap(data.infobox || []);
                        const displayStaff = getDisplayStaff(infoboxMap);
                        const ratingLabel = getRatingLabel(data.rating?.score ?? 0);
                        const score = data.rating?.score ?? 0;
                        const ratingEmojiIdx = score >= 8.5 ? 4 : score >= 6.5 ? 3 : score >= 5.5 ? 2 : score >= 3.5 ? 1 : 0;
                        const generatedAt = new Date().toISOString().slice(0, 10).replace(/-/g, '/');

                        // 获取章节列表（带重试）
                        let episodes: BangumiEpisode[] = [];
                        let airedCount = 0;
                        for (let retry = 0; retry < 2; retry++) {
                            try {
                                const epRes = await axios.get<BangumiEpisodesResponse>(
                                    `https://api.bgm.tv/v0/episodes?subject_id=${subjectId}&limit=100&offset=0`,
                                    { timeout: 15000, headers: bgmHeaders }
                                );
                                episodes = epRes.data?.data || [];
                                break;
                            } catch (err) {
                                if (retry === 1) {
                                    logger.warn(err, `获取 Bangumi 章节列表失败 subject_id=${subjectId}`);
                                }
                            }
                        }

                        // 分离本篇集数(type=0) 和其他集数(type!=0, 如 SP/OVA/OP/ED 等)
                        const mainEpisodes = episodes.filter(ep => ep.type === 0);
                        const rawOtherEpisodes = episodes.filter(ep => ep.type !== 0);

                        // 按 type 分组其他集数
                        const EPISODE_TYPE_LABELS: Record<number, string> = {
                            1: 'SP',
                            2: 'OP',
                            3: 'ED',
                            4: '预告',
                            5: 'MAD',
                            6: '其他',
                        };
                        const otherEpisodes = (() => {
                            const groups = new Map<number, typeof rawOtherEpisodes>();
                            for (const ep of rawOtherEpisodes) {
                                const list = groups.get(ep.type) ?? [];
                                list.push(ep);
                                groups.set(ep.type, list);
                            }
                            return Array.from(groups.entries())
                                .sort(([a], [b]) => a - b)
                                .map(([type, eps]) => ({
                                    type,
                                    label: EPISODE_TYPE_LABELS[type] || `类型${type}`,
                                    episodes: eps,
                                }));
                        })();

                        // 计算柱状图百分比
                        const ratingCounts = data.rating?.count || {};
                        const maxCount = Math.max(1, ...Object.values(ratingCounts));
                        const barPcts: number[] = [];
                        for (let i = 10; i >= 1; i--) {
                            barPcts.push(Math.max(3, Math.round(((ratingCounts[i] || 0) / maxCount) * 100)));
                        }

                        const col = data.collection || { on_hold: 0, dropped: 0, wish: 0, collect: 0, doing: 0 };
                        const fmtNum = (n: number) => n >= 1000 ? (n / 1000).toFixed(1) + "K" : String(n);

                        // 当前已播出最新集 & 下一集预告（仅基于本篇集数）
                        const todayStr = new Date().toISOString().slice(0, 10).replace(/-/g, '');
                        const airedEps = mainEpisodes.filter(ep => {
                            if (!ep.airdate || !ep.airdate.trim()) return false;
                            const ad = ep.airdate.replace(/-/g, '');
                            return ad <= todayStr && ad.length === 8;
                        });
                        airedCount = airedEps.length;
                        const currentEp = airedEps.length > 0 ? airedEps.sort((a, b) => b.sort - a.sort)[0] : null;
                        const nextEp = currentEp
                            ? mainEpisodes.find(ep => ep.sort > currentEp.sort)
                            : mainEpisodes.find(ep => ep.airdate && ep.airdate.trim());

                        // 计算当前集数所在分页
                        const episodesPerPage = 30;
                        const currentEpNum = currentEp?.sort ?? 0;
                        const activePageIdx = currentEpNum > 0
                            ? Math.min(Math.floor((currentEpNum - 1) / episodesPerPage), Math.ceil(mainEpisodes.length / episodesPerPage) - 1)
                            : 0;

                        // 读取 rate_emo.gif 转为 base64 Data URI（5 个表情横排 sprite）
                        let rateEmoDataUri = "";
                        let rateEmoW = 170, rateEmoH = 34;
                        try {
                            const rateEmoPath = new URL("rate_emo.gif", import.meta.url);
                            const rateEmoBuffer = await fs.readFile(rateEmoPath);
                            rateEmoDataUri = `data:image/gif;base64,${rateEmoBuffer.toString("base64")}`;
                            const meta = await (await import("sharp")).default(rateEmoBuffer).metadata();
                            if (meta.width && meta.height) {
                                rateEmoW = meta.width;
                                rateEmoH = meta.height;
                            }
                        } catch {
                            rateEmoDataUri = "";
                        }

                        // 构建 props
                        const props = {
                            id: data.id,
                            name: data.name || "",
                            name_cn: data.name_cn || "",
                            images: data.images || {},
                            date: data.date || "",
                            platform: data.platform || "",
                            total_episodes: data.total_episodes || 0,
                            rating: data.rating || { rank: 0, total: 0, count: {}, score: 0 },
                            tags: (data.tags || []).slice(0, 12),
                            infoboxMap,
                            collection: {
                                on_hold: col.on_hold,
                                dropped: col.dropped,
                                wish: col.wish,
                                collect: col.collect,
                                doing: col.doing,
                                on_hold_fmt: fmtNum(col.on_hold),
                                wish_fmt: fmtNum(col.wish),
                                collect_fmt: fmtNum(col.collect),
                                doing_fmt: fmtNum(col.doing),
                            },
                            displayStaff,
                            ratingLabel,
                            ratingEmojiIdx,
                            generatedAt,
                            rateEmoDataUri,
                            rateEmoW,
                            rateEmoH,
                            barPcts,
                            episodes: mainEpisodes,
                            otherEpisodes,
                            airedCount,
                            episodesPerPage,
                            activePageIdx,
                            currentEpNum,
                            currentEpName: currentEp?.name_cn || currentEp?.name || "",
                            currentEpAirDate: currentEp?.airdate || "",
                            nextEpNum: nextEp?.ep ?? 0,
                            nextEpName: nextEp?.name_cn || nextEp?.name || "",
                            nextEpAirDate: nextEp?.airdate || "",
                        };

                        // 读取 Vue 模板
                        const templateStr = await fs.readFile(
                            new URL("bangumi.vue", import.meta.url),
                            "utf-8"
                        );

                        // 生成图片
                        const imageResult = await generateImage(
                            {
                                width: 1000, height: 550, quality: 2
                            },
                            templateStr,
                            props
                        );

                        if (imageResult.path) {
                            const sendResult = await sendMessage(this.client, message.message.chat_id, {
                                media: { photo: { path: imageResult.path } },
                            });

                            if (sendResult?.content?._ === "messagePhoto" && imageResult.hash) {
                                const file_id = sendResult.content.photo.sizes.at(-1)?.photo.remote.id;
                                try {
                                    await updateImgCache(imageResult.hash, file_id!);
                                } catch (err) {
                                    logger.warn(err, "保存 Bangumi 图片 file_id 缓存失败");
                                }
                            }

                            await fs.unlink(imageResult.path).catch(() => { });
                        } else if (imageResult.hash && imageResult.file_id) {
                            // 缓存命中，直接用 file_id 发送
                            await sendMessage(this.client, message.message.chat_id, {
                                media: { photo: { id: imageResult.file_id } },
                            });
                        }

                        // 删除提示消息
                        if (tipMsg) {
                            try {
                                await this.client.invoke({
                                    _: "deleteMessages",
                                    chat_id: message.message.chat_id,
                                    message_ids: [tipMsg.id],
                                    revoke: true,
                                });
                            } catch { }
                        }
                    } catch (err) {
                        logger.error(err, `Bangumi 插件获取条目 #${subjectId} 失败`);
                        await sendMessage(this.client, message.message.chat_id, {
                            text: `获取 Bangumi 条目 #${subjectId} 信息失败，请检查 ID 是否正确或稍后重试。`,
                        });
                    }
                },
            },
        };
    }
}
