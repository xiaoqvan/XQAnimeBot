/* =========================
 * 📺 单集元数据
 * ========================= */

export type EpisodeMetaDoc = {
    /** 放送日期 */
    airdate?: string;
    /** 标题 */
    name?: string;
    /** 中文名 */
    name_cn?: string;
    /** 时长 */
    duration?: string;
    /** 简介 */
    desc?: string;
    /** 条目内的集数, 从1开始。非本篇剧集的此字段无意义 */
    ep: number;
    /** 同类条目的排序和集数 */
    sort: number;
    /** 章节id */
    id: number;
    /** 类型 (本篇 = 0, 特别篇 = 1, OP = 2, ED = 3, 预告/宣传/广告 = 4, MAD = 5, 其他 = 6)  */
    type?: number;
    /** 动漫 ID */
    subject_id: number;
    /** 回复数量 */
    comment?: number;
};