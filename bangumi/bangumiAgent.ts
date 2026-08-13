/**
 * @file Bangumi 番剧匹配 Agent
 *
 * 基于 LLM Tool Calling 的番剧匹配流程。
 * 将输入信息（标题、别名、集数）与 Bangumi 数据库中的条目进行匹配。
 *
 * #sym:bangumiAgent 供 AI Agent 使用
 *
 * @requires openai
 * @requires zod
 */

import { OpenAI } from "openai";
import { z } from "zod";
import { searchAnimeCandidates, getRelatedSubjects, getEpisodeDetail } from "./tools.ts";
import type { AnimeCandidate, RelatedSubject, EpisodeDetail } from "./tools.ts";
import { recordAiCall } from "../database/ai.ts";
import type { AiCallDoc } from "../database/ai.ts";

/** 当前 AI 调用场景（由各 Agent 入口设置，用于记录） */
let activeAiScene: AiCallDoc["scene"] = "bangumi_match";

export function setActiveAiScene(scene: AiCallDoc["scene"]): void {
  activeAiScene = scene;
}

// ─── 公开类型 ────────────────────────────────────────────────────────────────

/** 输入：待匹配的番剧信息 */
export type animeItem = {
    title: string;
    names: string[];
    episode?: string;
};

/** 输出：匹配结果 */
export type MatchResult = {
    subjectId?: number;
    /** 匹配到的章节 ID（精确到集），可选 */
    episodeId?: number;
    /** 匹配到的集数编号（sort 字段），可选 */
    episodeSort?: number;
    confidence: number;
    reason: string;
};

// ─── 内部类型 ────────────────────────────────────────────────────────────────

/** OpenAI Chat Completion Message 的联合类型 */
type ChatMessage =
    | { role: "system"; content: string }
    | { role: "user"; content: string }
    | { role: "assistant"; content: string | null; tool_calls?: OpenAI.Chat.Completions.ChatCompletionMessageToolCall[] }
    | { role: "tool"; tool_call_id: string; content: string };

// ─── 常量 ────────────────────────────────────────────────────────────────────

const MODEL = "deepseek-v4-flash";

// ─── 客户端（懒加载）────────────────────────────────────────────────────────

function getClient(): OpenAI {
    const apiKey = process.env.DEEPSEEK_API_KEY ?? process.env.OPENAI_API_KEY;
    if (!apiKey) {
        throw new Error(
            "缺少 API Key：请设置环境变量 DEEPSEEK_API_KEY 或 OPENAI_API_KEY",
        );
    }
    const client = new OpenAI({
        apiKey,
        baseURL: process.env.DEEPSEEK_BASE_URL ?? "https://api.deepseek.com",
    });

    // 包装底层 completions.create，自动记录每次 AI 调用（供 Web 展示）
    const completions = client.chat.completions as unknown as {
      create: (...args: any[]) => any;
    };
    const originalCreate = completions.create.bind(completions);
    const scene = activeAiScene;
    completions.create = async (...args: any[]) => {
        const start = Date.now();
        let output: string | undefined;
        let ok = false;
        try {
            const result = await originalCreate(...args);
            ok = true;
            output = result?.choices?.[0]?.message?.content ?? "";
            return result;
        } finally {
            const req = (args[0] ?? {}) as { messages?: Array<{ role: string; content: unknown }> };
            const lastUserMsg = [...(req.messages ?? [])].reverse().find((m) => m.role === "user");
            const input = typeof lastUserMsg?.content === "string" ? lastUserMsg.content : "";
            void recordAiCall({
                scene,
                input: input || "(AI 调用)",
                output: ok ? output ?? undefined : undefined,
                success: ok,
                model: (args[0] as { model?: string } | undefined)?.model,
                durationMs: Date.now() - start,
            });
        }
    };

    return client;
}

// ─── Zod Schemas ─────────────────────────────────────────────────────────────

/** searchAnimeCandidates 工具参数 */
const SearchAnimeCandidatesSchema = z.object({
    keyword: z.string().min(1, "搜索关键词不能为空"),
    limit: z.number().int().positive().max(50).default(10),
});

/** getRelatedSubjects 工具参数 */
const GetRelatedSubjectsSchema = z.object({
    subjectId: z.number().int().positive("条目 ID 必须为正整数"),
});

/** getEpisodeDetail 工具参数 */
const GetEpisodeDetailSchema = z.object({
    subjectId: z.number().int().positive("条目 ID 必须为正整数"),
    episodeSort: z.number().int().positive("集数编号必须为正整数"),
});

/** 最终输出 Schema */
const FinalOutputSchema = z.object({
    subjectId: z.number().int().optional(),
    episodeId: z.number().int().optional(),
    episodeSort: z.number().int().optional(),
    confidence: z.number().min(0).max(1),
    reason: z.string(),
});

// ─── Prompt ──────────────────────────────────────────────────────────────────

const SYSTEM_PROMPT = `你是 Bangumi 动画匹配 Agent。

目标：
根据动漫名称、别名、集数、候选条目和关联条目找到最准确的 Bangumi Subject ID，
并尽可能精确到具体的章节 ID（集数）。

优先级：
1. episode_range - 集数范围匹配（如第5集落在1-12范围内）
2. 中文名称 (name_cn)
3. 原名 (name)
4. 放送日期 (date)
5. 前传/续集关系 (relation)

可用工具：
- searchAnimeCandidates: 搜索番剧候选项
- getRelatedSubjects: 获取关联条目（前传/续集/番外）
- getEpisodeDetail: 获取指定条目中某集的具体信息（包括集数 ID）

匹配流程建议：
1. 先调用 searchAnimeCandidates 搜索候选
2. 如果候选能确定 subjectId，且输入包含集数信息，
   调用 getEpisodeDetail 查找匹配的集数 ID
3. 如果仍不确定，调用 getRelatedSubjects 查看关联条目

限制：
- 禁止在 searchAnimeCandidates、getRelatedSubjects 和 getEpisodeDetail 之间无限交替调用。
- 如果已经对某个条目调过 getRelatedSubjects，不要再次对相同条目重复调用。
- 优先根据已有信息得出结论，而不是无限制地探索。
- 只有当 episodeSort 完全匹配时才填入 episodeId。

confidence 必须是 0~1 之间的小数（如 0.97），不是 0~100。
- confidence = 1 表示完全确定
- confidence = 0 表示完全无法匹配

最终必须返回 JSON，格式如下：
{
  "subjectId": 123,
  "episodeId": 456,
  "episodeSort": 5,
  "confidence": 0.97,
  "reason": "..."
}

如果能确定条目但无法确定具体集数，只返回 subjectId：
{
  "subjectId": 123,
  "confidence": 0.97,
  "reason": "..."
}

匹配不上时返回：
{
  "confidence": 0,
  "reason": "..."
}

禁止输出 markdown 代码块标记或其他包装文本。`;

// ─── 工具执行器 ──────────────────────────────────────────────────────────────

/**
 * 执行单个工具调用并返回结果字符串。
 * 使用 zod 严格校验参数，捕获所有异常。
 */
async function executeToolCall(
    toolCall: OpenAI.Chat.Completions.ChatCompletionMessageFunctionToolCall,
    searchedKeywords: Set<string>,
    exploredSubjectIds: Set<number>,
): Promise<string> {
    const name = toolCall.function.name;
    let args: Record<string, unknown>;

    // 安全解析参数
    try {
        args = JSON.parse(toolCall.function.arguments) as Record<string, unknown>;
    } catch {
        return JSON.stringify({ error: `无法解析工具参数 JSON: ${toolCall.function.arguments}` });
    }

    try {
        switch (name) {
            case "searchAnimeCandidates": {
                const parsed = SearchAnimeCandidatesSchema.parse(args);

                // 去重：相同关键词不重复搜索
                const normalizedKeyword = parsed.keyword.trim().toLowerCase();
                if (searchedKeywords.has(normalizedKeyword)) {
                    return JSON.stringify({
                        _notice: `关键词 "${parsed.keyword}" 已搜索过，跳过重复调用`,
                        data: [],
                    });
                }
                searchedKeywords.add(normalizedKeyword);

                const results: AnimeCandidate[] = await searchAnimeCandidates(
                    parsed.keyword,
                    parsed.limit,
                );
                return JSON.stringify(results);
            }

            case "getRelatedSubjects": {
                const parsed = GetRelatedSubjectsSchema.parse(args);

                // 去重：相同 subjectId 不重复查关联
                if (exploredSubjectIds.has(parsed.subjectId)) {
                    return JSON.stringify({
                        _notice: `条目 ${parsed.subjectId} 的关联作品已查过，跳过重复调用`,
                        data: [],
                    });
                }
                exploredSubjectIds.add(parsed.subjectId);

                const results: RelatedSubject[] = await getRelatedSubjects(parsed.subjectId);
                return JSON.stringify(results);
            }

            case "getEpisodeDetail": {
                const parsed = GetEpisodeDetailSchema.parse(args);
                const result: EpisodeDetail | null = await getEpisodeDetail(
                    parsed.subjectId,
                    parsed.episodeSort,
                );
                return JSON.stringify(result);
            }

            default:
                return JSON.stringify({ error: `未知工具: ${name}` });
        }
    } catch (error: unknown) {
        if (error instanceof z.ZodError) {
            return JSON.stringify({
                error: `工具参数校验失败: ${error.message}`,
                issues: error.issues,
            });
        }
        if (error instanceof Error) {
            return JSON.stringify({ error: `工具执行异常: ${error.message}` });
        }
        return JSON.stringify({ error: "工具执行时发生未知错误" });
    }
}

/**
 * 从 LLM 返回的文本中提取 JSON 字符串。
 * 支持：
 * - 纯 JSON（{...}）
 * - markdown 代码块包裹的 JSON（```json\n{...}\n```）
 * - 文字说明 + JSON 混合
 */
function extractJsonFromText(raw: string): string {
    // 先尝试匹配 ```json ... ``` 代码块
    const codeBlockMatch = raw.match(/```(?:json)?\s*([\s\S]*?)```/);
    if (codeBlockMatch?.[1]) {
        return codeBlockMatch[1].trim();
    }

    // 尝试匹配最外层 { } 对象
    const braceMatch = raw.match(/\{[\s\S]*\}/);
    if (braceMatch) {
        return braceMatch[0];
    }

    // 保底：返回原文本
    return raw.trim();
}

/**
 * 尝试从文本中直接通过正则提取关键字段，
 * 用于 LLM 输出的 JSON 格式不标准时的兜底解析。
 */
function extractFieldsViaRegex(raw: string): MatchResult | undefined {
    const subjectIdMatch = raw.match(/"subjectId"\s*:\s*(\d+)/);
    const episodeIdMatch = raw.match(/"episodeId"\s*:\s*(\d+)/);
    const episodeSortMatch = raw.match(/"episodeSort"\s*:\s*(\d+)/);
    const confidenceMatch = raw.match(/"confidence"\s*:\s*([\d.]+)/);
    const reasonMatch = raw.match(/"reason"\s*:\s*"([\s\S]*?)"(?:\s*[,\}])/);

    if (confidenceMatch) {
        const confidence = parseFloat(confidenceMatch[1]!);
        if (!isNaN(confidence) && confidence >= 0 && confidence <= 1) {
            return {
                subjectId: subjectIdMatch ? parseInt(subjectIdMatch[1]!, 10) : undefined,
                episodeId: episodeIdMatch ? parseInt(episodeIdMatch[1]!, 10) : undefined,
                episodeSort: episodeSortMatch ? parseInt(episodeSortMatch[1]!, 10) : undefined,
                confidence,
                reason: reasonMatch?.[1]?.trim() ?? "（正则兜底提取）",
            };
        }
    }
    return undefined;
}

/**
 * 安全地从 JSON 字符串解析 MatchResult。
 * 先尝试标准 JSON 解析，失败后用正则兜底。
 */
function safeParseMatchResult(raw: string): MatchResult {
    const jsonStr = extractJsonFromText(raw);

    // 尝试标准 JSON 解析
    try {
        const parsed = JSON.parse(jsonStr) as Record<string, unknown>;
        const result = FinalOutputSchema.parse(parsed);
        return result;
    } catch {
        // JSON 解析失败，用正则兜底
        const fallback = extractFieldsViaRegex(jsonStr);
        if (fallback) {
            return fallback;
        }

        return {
            confidence: 0,
            reason: `LLM 返回结果解析失败，原始输出: ${raw}`,
        };
    }
}

// ─── 主函数 ──────────────────────────────────────────────────────────────────

/**
 * 将 animeItem 与 Bangumi 条目进行匹配。
 *
 * 流程：
 * 1. 遍历 anime.names 调用 searchAnimeCandidates，合并去重
 * 2. 候选为空 → 直接返回 confidence: 0
 * 3. 唯一候选 → 直接返回该候选
 * 4. 若 anime.episode 存在，提取数字并匹配 episode_range
 * 5. 若仍不确定 → 调用 LLM Tool Calling Loop 做最终决策
 *
 * @param anime - 待匹配的番剧信息
 * @param llmThreshold - 候选数量超过此值时调用 LLM 决策；
 *                       候选数量 ≤ 此值时优先用规则匹配（唯一候选即可置信返回）。
 *                       默认 1，即超过 1 个候选时触发 LLM。
 *                       设为 0 可跳过 LLM 直接返回最佳猜测。
 * @returns 匹配结果
 */
export async function matchAnimeSubject(
    anime: animeItem,
    llmThreshold = 1,
): Promise<MatchResult> {
    setActiveAiScene("bangumi_match");
    if (!anime || !anime.title) {
        return { confidence: 0, reason: "输入数据无效：缺少 title" };
    }

    // ── 第一阶段：搜索合并 ──────────────────────────────────────────────────

    const candidateMap = new Map<number, AnimeCandidate>();

    const namesToSearch = [anime.title, ...anime.names].filter(
        (n): n is string => typeof n === "string" && n.trim().length > 0,
    );

    for (const name of namesToSearch) {
        try {
            const results = await searchAnimeCandidates(name);
            for (const candidate of results) {
                if (!candidateMap.has(candidate.id)) {
                    candidateMap.set(candidate.id, candidate);
                }
            }
        } catch {
            // 单个搜索失败不影响其他搜索
        }
    }

    const candidates = Array.from(candidateMap.values());

    // ── 第二阶段：无候选 ────────────────────────────────────────────────────

    if (candidates.length === 0) {
        return { confidence: 0, reason: "未找到候选" };
    }

    // ── 第三阶段：候选数 ≤ llmThreshold，规则匹配 ──────────────────────────

    if (candidates.length <= llmThreshold) {
        if (candidates.length === 1) {
            const subjectId = candidates[0]!.id;
            // 尝试在规则阶段解析集数
            if (anime.episode) {
                const episodeSort = extractEpisodeNumber(anime.episode);
                if (episodeSort !== undefined) {
                    const epDetail = await getEpisodeDetail(subjectId, episodeSort);
                    if (epDetail) {
                        return {
                            subjectId,
                            episodeId: epDetail.id,
                            episodeSort,
                            confidence: 0.97,
                            reason: `唯一候选 + 集数 ${episodeSort} 匹配（章节 ID: ${epDetail.id}）`,
                        };
                    }

                    // 规则集数匹配失败，使用 AI 辅助查找
                    const aiEp = await aiEpisodeSearch(subjectId, anime.episode, candidates[0]);
                    if (aiEp) {
                        return {
                            subjectId,
                            episodeId: aiEp.episodeId,
                            episodeSort: aiEp.episodeSort,
                            confidence: 0.95,
                            reason: `唯一候选 + AI 辅助集数匹配（集数 ${aiEp.episodeSort}，章节 ID: ${aiEp.episodeId}）`,
                        };
                    }
                }
            }
            return {
                subjectId,
                confidence: 0.95,
                reason: "唯一候选",
            };
        }

        // llmThreshold > 1 且候选数 ≤ llmThreshold
        // 尝试集数匹配缩小范围
        if (anime.episode) {
            const episodeNumber = extractEpisodeNumber(anime.episode);
            if (episodeNumber !== undefined) {
                const rangeMatched = candidates.filter((c) =>
                    isEpisodeInRange(episodeNumber, c.episode_range),
                );

                if (rangeMatched.length === 1) {
                    const subjectId = rangeMatched[0]!.id;
                    // 尝试在规则阶段解析集数
                    const epDetail = await getEpisodeDetail(subjectId, episodeNumber);
                    if (epDetail) {
                        return {
                            subjectId,
                            episodeId: epDetail.id,
                            episodeSort: episodeNumber,
                            confidence: 0.98,
                            reason: `集数范围命中 + 集数 ${episodeNumber} 匹配（章节 ID: ${epDetail.id}）`,
                        };
                    }

                    // 规则集数匹配失败，使用 AI 辅助查找
                    if (anime.episode) {
                        const aiEp = await aiEpisodeSearch(subjectId, anime.episode, rangeMatched[0]);
                        if (aiEp) {
                            return {
                                subjectId,
                                episodeId: aiEp.episodeId,
                                episodeSort: aiEp.episodeSort,
                                confidence: 0.96,
                                reason: `集数范围命中 + AI 辅助集数匹配（集数 ${aiEp.episodeSort}，章节 ID: ${aiEp.episodeId}）`,
                            };
                        }
                    }

                    return {
                        subjectId,
                        confidence: 0.98,
                        reason: "集数范围命中",
                    };
                }
            }
        }

        // 规则无法区分，返回候选列表让调用方自行处理
        return {
            confidence: 0,
            reason: `候选数 ${candidates.length} 未超过阈值 ${llmThreshold}，但规则无法决策`,
        };
    }

    // ── 第四阶段：集数匹配（缩小 LLM 的候选范围）───────────────────────────

    let narrowedCandidates = candidates;

    if (anime.episode) {
        const episodeNumber = extractEpisodeNumber(anime.episode);
        if (episodeNumber !== undefined) {
            const rangeMatched = narrowedCandidates.filter((c) =>
                isEpisodeInRange(episodeNumber, c.episode_range),
            );

            if (rangeMatched.length === 1) {
                const subjectId = rangeMatched[0]!.id;
                // 尝试在规则阶段解析集数
                const epDetail = await getEpisodeDetail(subjectId, episodeNumber);
                if (epDetail) {
                    return {
                        subjectId,
                        episodeId: epDetail.id,
                        episodeSort: episodeNumber,
                        confidence: 0.98,
                        reason: `集数范围命中 + 集数 ${episodeNumber} 匹配（章节 ID: ${epDetail.id}）`,
                    };
                }

                // 规则集数匹配失败，使用 AI 辅助查找
                if (anime.episode) {
                    const aiEp = await aiEpisodeSearch(subjectId, anime.episode, rangeMatched[0]);
                    if (aiEp) {
                        return {
                            subjectId,
                            episodeId: aiEp.episodeId,
                            episodeSort: aiEp.episodeSort,
                            confidence: 0.96,
                            reason: `集数范围命中 + AI 辅助集数匹配（集数 ${aiEp.episodeSort}，章节 ID: ${aiEp.episodeId}）`,
                        };
                    }
                }

                return {
                    subjectId,
                    confidence: 0.98,
                    reason: "集数范围命中",
                };
            }

            if (rangeMatched.length > 1) {
                narrowedCandidates = rangeMatched;
            }
        }
    }

    // ── 第五阶段：LLM 决策（候选数超过 llmThreshold 时触发）─────────────────

    return llmDecision(anime, narrowedCandidates);
}

// ─── 集数提取与匹配 ─────────────────────────────────────────────────────────

/**
 * 从字符串中提取数字。
 *
 * 示例：
 * - "37" → 37
 * - "第37集" → 37
 * - "EP37" → 37
 * - "ep.37" → 37
 *
 * @param raw - 原始集数字符串
 * @returns 提取的数字，无法提取则返回 undefined
 */
function extractEpisodeNumber(raw: string): number | undefined {
    const match = raw.match(/(\d+)/);
    if (!match) {
        return undefined;
    }
    return parseInt(match[1]!, 10);
}

/**
 * 判断集数是否落在范围字符串内。
 * 范围格式：如 "1-12"、"13-24"、"25-36"。
 * 也支持纯数字格式如 "12"（表示 1-12）。
 *
 * @param episode - 集数
 * @param range - 范围字符串，如 "1-12"
 * @returns 是否在范围内
 */
function isEpisodeInRange(episode: number, range?: string): boolean {
    if (!range) {
        return false;
    }

    const parts = range.split("-").map((s) => s.trim());

    if (parts.length === 2) {
        const lower = parseInt(parts[0]!, 10);
        const upper = parseInt(parts[1]!, 10);
        if (!isNaN(lower) && !isNaN(upper)) {
            return episode >= lower && episode <= upper;
        }
    }

    // 尝试解析为单数字（如 "12" 表示 1-12）
    const single = parseInt(range, 10);
    if (!isNaN(single)) {
        return episode >= 1 && episode <= single;
    }

    return false;
}

// ─── AI 辅助集数查找 ─────────────────────────────────────────────────────────

/**
 * 使用 LLM Tool Calling Loop 辅助查找正确的集数 ID。
 * 当规则提取的集数编号无法匹配时，调用此函数通过迭代工具调用找到正确的集数。
 */
export async function aiEpisodeSearch(
    subjectId: number,
    episode: string,
    candidate?: AnimeCandidate,
): Promise<{ episodeId: number; episodeSort: number } | null> {
    setActiveAiScene("episode_match");
    const AI_EPISODE_SYSTEM_PROMPT = `你是 Bangumi 集数匹配助手。
根据番剧信息和集数字符串，通过工具调用迭代查找最准确的集数编号（sort 值）和章节 ID（id）。

番剧信息包含原名、中文名、放送日期和集数范围。
集数字符串可能包含"第X集"、"EP X"、"X話"等各种格式。

可用工具：
- searchAnimeCandidates: 搜索番剧候选项。当需要从当前候选之外搜索更多相关信息时调用。
- getRelatedSubjects: 获取关联条目（前传/续集/番外）。当集数可能属于关联条目时调用。
- getEpisodeDetail: 获取指定条目中某集的具体详情。调用此工具验证集数是否存在并获取章节 ID。

流程建议：
1. 根据番剧信息和集数字符串确定目标条目
2. 调用 getEpisodeDetail 验证集数是否存在
3. 如果不在当前条目中，可调用 getRelatedSubjects 查看关联条目
4. 需要更多信息时调用 searchAnimeCandidates
5. 重复直到找到匹配的集数或确定无法匹配

限制：
- 不要对相同的集数编号重复调用 getEpisodeDetail
- 优先根据已有信息得出结论，而不是无限制地探索

最终必须返回 JSON，格式如下：
{
  "episodeSort": 5,
  "episodeId": 12345,
  "reason": "解释为什么是这个集数以及验证过程"
}

如果完全无法确定，返回：
{
  "episodeSort": null,
  "episodeId": null,
  "reason": "..."
}

禁止输出 markdown 代码块标记或其他包装文本。`;

    const candidateLines: string[] = [];
    if (candidate) {
        candidateLines.push("番剧信息：");
        candidateLines.push(`- 原名: ${candidate.name}`);
        candidateLines.push(`- 中文名: ${candidate.name_cn ?? "(无)"}`);
        candidateLines.push(`- 放送日期: ${candidate.date ?? "(未知)"}`);
        candidateLines.push(`- 集数范围: ${candidate.episode_range ?? "(未知)"}`);
        candidateLines.push(`- 简介: ${candidate.summary ?? "(无)"}`);
        candidateLines.push("");
    }
    candidateLines.push(`集数字符串: "${episode}"`);
    candidateLines.push("");
    candidateLines.push("可用工具：searchAnimeCandidates（搜索更多候选项）、getRelatedSubjects（查看关联条目）、getEpisodeDetail（验证集数）。");
    candidateLines.push("请先确定目标条目，然后调用 getEpisodeDetail 验证集数。");

    const userPrompt = candidateLines.join("\n");

    const messages: ChatMessage[] = [
        { role: "system", content: AI_EPISODE_SYSTEM_PROMPT },
        { role: "user", content: userPrompt },
    ];

    const MAX_ITERATIONS = 8;
    let iteration = 0;

    // 去重追踪
    const exploredEpisodeSorts = new Set<number>();
    const searchedKeywords = new Set<string>();
    const exploredSubjectIds = new Set<number>();

    while (iteration < MAX_ITERATIONS) {
        iteration++;

        let response;
        try {
            response = await getClient().chat.completions.create({
                model: MODEL,
                messages,
                tools: [
                    {
                        type: "function",
                        function: {
                            name: "searchAnimeCandidates",
                            description:
                                "使用关键词搜索 Bangumi 番剧候选列表。返回 AnimeCandidate[]，包含 id、名称、中文名、放送日期、集数范围和简介。在需要从当前候选之外查找更多番剧信息时调用此工具。",
                            parameters: {
                                type: "object",
                                properties: {
                                    keyword: {
                                        type: "string",
                                        description: "搜索关键词，可以是番剧原名、中文名、别名等",
                                    },
                                    limit: {
                                        type: "number",
                                        description: "返回结果数量上限（1-50），默认 10",
                                        default: 10,
                                    },
                                },
                                required: ["keyword"],
                            },
                        },
                    },
                    {
                        type: "function",
                        function: {
                            name: "getRelatedSubjects",
                            description:
                                "获取指定 Bangumi 条目的关联作品列表（如前传、续集、番外篇等）。仅返回 type=2（动画）的关联条目。当需要查找关联作品的集数信息时调用此工具。",
                            parameters: {
                                type: "object",
                                properties: {
                                    subjectId: {
                                        type: "number",
                                        description: "Bangumi 条目 ID",
                                    },
                                },
                                required: ["subjectId"],
                            },
                        },
                    },
                    {
                        type: "function",
                        function: {
                            name: "getEpisodeDetail",
                            description:
                                "获取指定 Bangumi 条目中某集的具体详情，包括章节 ID（id）、集数编号（sort）和放送日期。当确定了条目和集数编号时，调用此工具验证该集是否存在并获取精确的章节 ID。",
                            parameters: {
                                type: "object",
                                properties: {
                                    subjectId: {
                                        type: "number",
                                        description: "Bangumi 条目 ID",
                                    },
                                    episodeSort: {
                                        type: "number",
                                        description: "集数编号，如第 5 集则传入 5",
                                    },
                                },
                                required: ["subjectId", "episodeSort"],
                            },
                        },
                    },
                ],
                tool_choice: "auto" as const,
                temperature: 0.1,
            });
        } catch {
            return null;
        }

        const choice = response.choices[0];
        if (!choice) {
            return null;
        }

        const message = choice.message;
        const toolCalls = message.tool_calls;
        const content = message.content;

        // 只处理 type === 'function' 的 tool_calls
        const functionToolCalls = (toolCalls ?? []).filter(
            (tc): tc is OpenAI.Chat.Completions.ChatCompletionMessageFunctionToolCall =>
                tc.type === "function",
        );

        if (functionToolCalls.length === 0) {
            // 没有工具调用，视为最终回答，尝试解析 JSON
            try {
                const jsonStr = extractJsonFromText(content ?? "");
                const parsed = JSON.parse(jsonStr) as Record<string, unknown>;
                const episodeSort = parsed.episodeSort;
                const episodeId = parsed.episodeId;
                if (
                    typeof episodeSort === "number" && episodeSort > 0 &&
                    typeof episodeId === "number" && episodeId > 0
                ) {
                    return { episodeId, episodeSort };
                }
            } catch {
                // 解析失败
            }
            return null;
        }

        // 记录助手消息
        messages.push({
            role: "assistant",
            content: message.content,
            tool_calls: toolCalls,
        });

        // 执行工具调用
        for (const toolCall of functionToolCalls) {
            const name = toolCall.function.name;
            let args: Record<string, unknown>;
            try {
                args = JSON.parse(toolCall.function.arguments) as Record<string, unknown>;
            } catch {
                continue;
            }

            switch (name) {
                case "searchAnimeCandidates": {
                    try {
                        const parsed = SearchAnimeCandidatesSchema.parse(args);
                        const normalizedKeyword = parsed.keyword.trim().toLowerCase();
                        if (searchedKeywords.has(normalizedKeyword)) {
                            messages.push({
                                role: "tool",
                                tool_call_id: toolCall.id,
                                content: JSON.stringify({
                                    _notice: `关键词 "${parsed.keyword}" 已搜索过，跳过重复调用`,
                                    data: [],
                                }),
                            });
                            continue;
                        }
                        searchedKeywords.add(normalizedKeyword);
                        const results = await searchAnimeCandidates(parsed.keyword, parsed.limit);
                        messages.push({
                            role: "tool",
                            tool_call_id: toolCall.id,
                            content: JSON.stringify(results),
                        });
                    } catch {
                        messages.push({
                            role: "tool",
                            tool_call_id: toolCall.id,
                            content: JSON.stringify({ error: "参数校验或执行失败" }),
                        });
                    }
                    break;
                }

                case "getRelatedSubjects": {
                    try {
                        const parsed = GetRelatedSubjectsSchema.parse(args);
                        if (exploredSubjectIds.has(parsed.subjectId)) {
                            messages.push({
                                role: "tool",
                                tool_call_id: toolCall.id,
                                content: JSON.stringify({
                                    _notice: `条目 ${parsed.subjectId} 的关联作品已查过，跳过重复调用`,
                                    data: [],
                                }),
                            });
                            continue;
                        }
                        exploredSubjectIds.add(parsed.subjectId);
                        const results = await getRelatedSubjects(parsed.subjectId);
                        messages.push({
                            role: "tool",
                            tool_call_id: toolCall.id,
                            content: JSON.stringify(results),
                        });
                    } catch {
                        messages.push({
                            role: "tool",
                            tool_call_id: toolCall.id,
                            content: JSON.stringify({ error: "参数校验或执行失败" }),
                        });
                    }
                    break;
                }

                case "getEpisodeDetail": {
                    const sort = args.episodeSort as number;

                    // 去重：相同集数编号不重复查询
                    if (exploredEpisodeSorts.has(sort)) {
                        messages.push({
                            role: "tool",
                            tool_call_id: toolCall.id,
                            content: JSON.stringify({
                                _notice: `集数 ${sort} 已查过，跳过重复调用`,
                                data: null,
                            }),
                        });
                        continue;
                    }
                    exploredEpisodeSorts.add(sort);

                    try {
                        const parsed = GetEpisodeDetailSchema.parse(args);
                        const result = await getEpisodeDetail(parsed.subjectId, parsed.episodeSort);
                        messages.push({
                            role: "tool",
                            tool_call_id: toolCall.id,
                            content: JSON.stringify(result),
                        });
                    } catch (error: unknown) {
                        if (error instanceof z.ZodError) {
                            messages.push({
                                role: "tool",
                                tool_call_id: toolCall.id,
                                content: JSON.stringify({
                                    error: `参数校验失败: ${error.message}`,
                                    issues: error.issues,
                                }),
                            });
                        } else {
                            messages.push({
                                role: "tool",
                                tool_call_id: toolCall.id,
                                content: JSON.stringify({ error: "工具执行异常" }),
                            });
                        }
                    }
                    break;
                }
            }
        }
    }

    return null;
}

// ─── LLM Tool Calling Loop ───────────────────────────────────────────────────

/**
 * 使用 LLM Tool Calling Loop 进行最终决策。
 *
 * 传入当前候选列表，LLM 可以通过工具调用进一步探索，
 * 最终返回匹配结果。
 */
async function llmDecision(
    anime: animeItem,
    candidates: AnimeCandidate[],
): Promise<MatchResult> {
    setActiveAiScene("bangumi_match");
    const userPrompt = buildLLMPrompt(anime, candidates);

    // messages 从 system 开始，后续逐步追加 user / assistant / tool
    const messages: ChatMessage[] = [
        { role: "system", content: SYSTEM_PROMPT },
        { role: "user", content: userPrompt },
    ];

    const processLog: string[] = [];
    processLog.push(`LLM 决策开始，候选数: ${candidates.length}`);

    let finalRaw = "";

    // 最大迭代轮次防止无限循环
    const MAX_ITERATIONS = 15;
    let iteration = 0;

    // 追踪已搜索过的关键词和已查过关联的 subjectId，防止重复调用
    const searchedKeywords = new Set<string>();
    const exploredSubjectIds = new Set<number>();

    while (iteration < MAX_ITERATIONS) {
        iteration++;

        let response;
        try {
            response = await getClient().chat.completions.create({
                model: MODEL,
                messages,
                tools: [
                    {
                        type: "function",
                        function: {
                            name: "searchAnimeCandidates",
                            description:
                                "使用关键词搜索 Bangumi 番剧候选列表。返回 AnimeCandidate[]，包含 id、名称、中文名、放送日期、集数范围和简介。在搜索别名或模糊名称时调用此工具。",
                            parameters: {
                                type: "object",
                                properties: {
                                    keyword: {
                                        type: "string",
                                        description: "搜索关键词，可以是番剧原名、中文名、别名等",
                                    },
                                    limit: {
                                        type: "number",
                                        description: "返回结果数量上限（1-50），默认 10",
                                        default: 10,
                                    },
                                },
                                required: ["keyword"],
                            },
                        },
                    },
                    {
                        type: "function",
                        function: {
                            name: "getRelatedSubjects",
                            description:
                                "获取指定 Bangumi 条目的关联作品列表（如前传、续集、番外篇等）。仅返回 type=2（动画）的关联条目。当需要区分多季番剧或查找关联作品时调用此工具。",
                            parameters: {
                                type: "object",
                                properties: {
                                    subjectId: {
                                        type: "number",
                                        description: "Bangumi 条目 ID",
                                    },
                                },
                                required: ["subjectId"],
                            },
                        },
                    },
                    {
                        type: "function",
                        function: {
                            name: "getEpisodeDetail",
                            description:
                                "获取指定 Bangumi 条目中某集的具体详情，包括章节 ID（id）、集数编号（sort）和放送日期。当确定了 subjectId 并且输入包含集数信息时，调用此工具获取精确的章节 ID。",
                            parameters: {
                                type: "object",
                                properties: {
                                    subjectId: {
                                        type: "number",
                                        description: "Bangumi 条目 ID",
                                    },
                                    episodeSort: {
                                        type: "number",
                                        description: "集数编号，如第 5 集则传入 5",
                                    },
                                },
                                required: ["subjectId", "episodeSort"],
                            },
                        },
                    },
                ],
                tool_choice: "auto" as const,
                temperature: 0.1,
            });
        } catch (error: unknown) {
            if (error instanceof Error) {
                return {
                    confidence: 0,
                    reason: `LLM 调用失败: ${error.message}`,
                };
            }
            return {
                confidence: 0,
                reason: "LLM 调用时发生未知错误",
            };
        }

        const choice = response.choices[0];
        if (!choice) {
            return { confidence: 0, reason: "LLM 未返回任何结果" };
        }

        const message = choice.message;
        const toolCalls = message.tool_calls;
        const content = message.content;

        // 只处理 type === 'function' 的 tool_calls（过滤掉 custom tool）
        const functionToolCalls = (toolCalls ?? []).filter(
            (tc): tc is OpenAI.Chat.Completions.ChatCompletionMessageFunctionToolCall =>
                tc.type === "function",
        );

        if (functionToolCalls.length === 0) {
            // 没有可执行的 function tool，视为最终回答
            finalRaw = content ?? "";
            processLog.push(`第 ${iteration} 轮迭代: LLM 给出最终决策`);
            break;
        }

        // 记录助手消息（保持与 OpenAI SDK 返回的类型一致）
        messages.push({
            role: "assistant",
            content: message.content,
            tool_calls: toolCalls,
        });

        // 执行所有工具调用（传入去重集合防止无限循环）
        for (const toolCall of functionToolCalls) {
            const result = await executeToolCall(toolCall, searchedKeywords, exploredSubjectIds);

            messages.push({
                role: "tool",
                tool_call_id: toolCall.id,
                content: result,
            });
        }

        // 记录本轮迭代的工具调用
        const toolNames = functionToolCalls.map(tc => tc.function.name);
        processLog.push(`第 ${iteration} 轮迭代: 调用工具 [${toolNames.join(', ')}]`);
    }

    if (iteration >= MAX_ITERATIONS) {
        return {
            confidence: 0,
            reason: `LLM 工具调用达到最大迭代次数，未能得出结果\n\n迭代流程:\n${processLog.join('\n')}`,
        };
    }

    const result = safeParseMatchResult(finalRaw);
    result.reason += `\n\n迭代流程:\n${processLog.join('\n')}`;
    return result;
}

// ─── Prompt 构建 ─────────────────────────────────────────────────────────────

/**
 * 构建 LLM 用户消息。
 * 将 animeItem 和候选列表格式化为文本。
 */
function buildLLMPrompt(anime: animeItem, candidates: AnimeCandidate[]): string {
    const lines: string[] = [];

    lines.push("请匹配以下番剧到 Bangumi 条目：");
    lines.push("");
    lines.push(`标题: ${anime.title}`);
    if (anime.names.length > 0) {
        lines.push(`别名: ${anime.names.join("、")}`);
    }
    if (anime.episode) {
        lines.push(`集数: ${anime.episode}`);
    }
    lines.push("");
    lines.push(`当前候选列表 (${candidates.length} 个):`);
    lines.push("");

    for (const c of candidates) {
        lines.push(`- ID: ${c.id}`);
        lines.push(`  原名: ${c.name}`);
        lines.push(`  中文名: ${c.name_cn ?? "(无)"}`);
        lines.push(`  放送日期: ${c.date ?? "(未知)"}`);
        lines.push(`  集数范围: ${c.episode_range ?? "(未知)"}`);
        lines.push(`  简介: ${truncate(c.summary, 120) ?? "(无)"}`);
        lines.push("");
    }

    lines.push("如果需要更多信息，请调用 searchAnimeCandidates、getRelatedSubjects 或 getEpisodeDetail 工具。");
    lines.push("当你能确定 subjectId 并且输入包含集数信息时，调用 getEpisodeDetail 获取精确的章节 ID（episodeId）。");
    lines.push("确定结果后，请直接返回 JSON，不要包含 markdown 代码块标记。");

    return lines.join("\n");
}

/**
 * 截断字符串到指定长度。
 */
function truncate(text: string | undefined, maxLen: number): string | undefined {
    if (!text) {
        return text;
    }
    if (text.length <= maxLen) {
        return text;
    }
    return `${text.slice(0, maxLen)}...`;
}
