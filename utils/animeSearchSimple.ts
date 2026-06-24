/**
 * 精简搜索结果格式化工具
 *
 * 提供将 Bangumi 搜索结果格式化为简洁文本的功能，
 * 适用于内联查询等需要快速展示的场景。
 */

/** 精简搜索结果的单条条目 */
export interface SearchSimpleResult {
    /** Bangumi subject ID */
    id: number;
    /** 动漫名称 */
    name: string;
    /** 中文名称（可选） */
    nameCN?: string;
    /** 类型 */
    type?: string;
    /** 评分 */
    score?: number | string;
    /** 集数 */
    episodes?: string;
    /** 放送状态 */
    airingStatus?: string;
    /** 简介（截断） */
    summary?: string;
}

/** 精简搜索选项 */
export interface SearchSimpleOptions {
    /** 最大返回条数 */
    limit?: number;
    /** 搜索关键词 */
    keyword: string;
}

/**
 * 将 Bangumi 搜索结果格式化为简洁文本
 *
 * @param results - 搜索结果列表
 * @returns 格式化后的纯文本字符串
 */
export function formatSearchSimple(results: SearchSimpleResult[]): string {
    if (!results || results.length === 0) {
        return "未找到相关番剧";
    }

    return results
        .map((item, index) => {
            const name = item.nameCN || item.name;
            const score = item.score ? ` ⭐${item.score}` : "";
            const ep = item.episodes ? ` 📺${item.episodes}话` : "";
            return `${index + 1}. **${name}**${score}${ep}\n   ID: \`${item.id}\``;
        })
        .join("\n\n");
}
