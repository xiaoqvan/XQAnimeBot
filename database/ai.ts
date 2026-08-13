import logger from "@log/index.ts";
import { getDatabase } from "@db/index.ts";
import { getErrorMessage } from "@utils/error.ts";

const db = await getDatabase();

/**
 * AI 调用记录文档结构。
 * 用于记录插件内所有 LLM（AI）调用的输入、输出、耗时与结果，供 Web 展示。
 */
export interface AiCallDoc {
  /** 调用场景：bangumi 匹配 / 集数匹配 / 集数提取 */
  scene: "bangumi_match" | "episode_match" | "episode_extract";
  /** 输入内容（标题/关键词等） */
  input: string;
  /** AI 输出内容（原始文本/结果摘要） */
  output?: string;
  /** 是否成功 */
  success: boolean;
  /** 模型名称 */
  model?: string;
  /** 耗时（毫秒） */
  durationMs?: number;
  /** 额外信息（JSON 字符串，如轮数、置信度等） */
  meta?: string;
  /** 创建时间 */
  createdAt: Date;
}

/**
 * 记录一次 AI 调用。
 * 写入 ai_calls 集合；自动裁剪 input/output/meta 避免文档过大。
 * 该函数不会抛出异常（写日志失败不应中断主流程）。
 */
export async function recordAiCall(
  data: Omit<AiCallDoc, "createdAt">
): Promise<void> {
  try {
    const doc: AiCallDoc = {
      ...data,
      input: String(data.input ?? "").slice(0, 2000),
      output: data.output ? String(data.output).slice(0, 4000) : undefined,
      meta: data.meta ? String(data.meta).slice(0, 2000) : undefined,
      createdAt: new Date(),
    };
    await db.collection<AiCallDoc>("ai_calls").insertOne(doc);
  } catch (error) {
    logger.warn(`记录 AI 调用失败: ${getErrorMessage(error)}`);
  }
}

/**
 * 分页查询 AI 调用记录（最新的在前）。
 * @param page - 页码，从 1 开始
 * @param pageSize - 每页数量，默认 20，最大 100
 * @param scene - 可选的场景过滤
 */
export async function listAiCalls(
  page: number = 1,
  pageSize: number = 20,
  scene?: AiCallDoc["scene"]
): Promise<{ items: AiCallDoc[]; total: number; page: number; pageSize: number }> {
  try {
    const safePage = Math.max(1, Math.floor(page) || 1);
    const safeSize = Math.min(100, Math.max(1, Math.floor(pageSize) || 20));
    const filter = scene ? { scene } : {};

    const collection = db.collection<AiCallDoc>("ai_calls");
    const total = await collection.countDocuments(filter);
    const items = await collection
      .find(filter)
      .sort({ createdAt: -1 })
      .skip((safePage - 1) * safeSize)
      .limit(safeSize)
      .toArray();

    return { items, total, page: safePage, pageSize: safeSize };
  } catch (error) {
    throw new Error(`查询 AI 调用记录失败: ${getErrorMessage(error)}`);
  }
}
