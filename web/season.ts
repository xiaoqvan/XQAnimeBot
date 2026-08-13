/**
 * 年季解析工具：从番剧的 airingStart（放送开始）推导"年份 + 季度"。
 *
 * airingStart 格式不统一，可能为：
 *   - ISO：`2024-04-01`、`2024-04-03 25:35`
 *   - 中文 infobox：`2024年4月`、`2024年04月01日`、`2024年4月1日 更`
 *   - 其他：空串 / undefined / 无法识别
 *
 * 季度划分（日本动画惯例）：春 3-5 月、夏 6-8 月、秋 9-11 月、冬 12-2 月。
 */

export type SeasonKey = `${number}-${number}`; // 如 "2024-1"（1春 2夏 3秋 4冬）
export type SeasonResult =
  | { ok: true; year: number; season: 1 | 2 | 3 | 4; seasonLabel: string; key: SeasonKey }
  | { ok: false; reason: "empty" | "invalid" };

const SEASON_NAME: Record<1 | 2 | 3 | 4, string> = {
  1: "春",
  2: "夏",
  3: "秋",
  4: "冬",
};

function monthToSeason(month: number): 1 | 2 | 3 | 4 {
  if (month >= 3 && month <= 5) return 1; // 春
  if (month >= 6 && month <= 8) return 2; // 夏
  if (month >= 9 && month <= 11) return 3; // 秋
  return 4; // 冬（12, 1, 2）
}

/**
 * 从 airingStart 推导年季。
 * @param raw 原始 airingStart 字符串
 */
export function deriveSeason(raw: string | undefined | null): SeasonResult {
  if (!raw) return { ok: false, reason: "empty" };
  const str = String(raw).trim();
  if (!str) return { ok: false, reason: "empty" };

  // 匹配 ISO 或中文年份-月份：2024-04-01 / 2024-04-03 25:35 / 2024年4月 / 2024年04月01日
  // 优先匹配 年 + 月 的多种写法
  let year: number | undefined;
  let month: number | undefined;

  // ISO: YYYY-MM
  let m = str.match(/^(\d{4})-(\d{1,2})/);
  if (m) {
    year = Number(m[1]);
    month = Number(m[2]);
  } else {
    // 中文: YYYY年MM月
    m = str.match(/(\d{4})年(\d{1,2})月/);
    if (m) {
      year = Number(m[1]);
      month = Number(m[2]);
    } else {
      // 尝试纯四位数年份后跟月份，如 "2024 4月"
      m = str.match(/(\d{4})[年.\-/]?\s*(\d{1,2})月/);
      if (m) {
        year = Number(m[1]);
        month = Number(m[2]);
      }
    }
  }

  if (year === undefined || month === undefined) {
    return { ok: false, reason: "invalid" };
  }

  const season = monthToSeason(month);
  return {
    ok: true,
    year,
    season,
    seasonLabel: `${year} ${SEASON_NAME[season]}番`,
    key: `${year}-${season}`,
  };
}

/**
 * 将 SeasonKey 转成展示标签（如 "2024-1" -> "2024 春番"）。
 */
export function seasonKeyToLabel(key: string): string {
  const [y, s] = key.split("-").map(Number);
  if (!y || !s || s < 1 || s > 4) return key;
  return `${y} ${SEASON_NAME[s as 1 | 2 | 3 | 4]}番`;
}
