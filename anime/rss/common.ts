/**
 * 公共标题过滤器
 * 返回 true 表示允许该标题（即不应被过滤），返回 false 表示应被过滤掉
 */
export function isTitleAllowed(title: string): boolean {
  if (!title) return false;

  // 常见的直接排除项
  const directExcludes = [
    "内封",
    "繁",
    "合集",
    "無字幕",
    "粵語",
    "整理搬运",
    "無對白字幕",
    "BIG5",
    "[720p]",
  ];

  for (const s of directExcludes) {
    if (title.includes(s)) return false;
  }

  // 精确/正则匹配项
  if (/\bFin\b/i.test(title)) return false;
  if (/\bMKV\b/i.test(title)) return false;

  // 幻樱字幕组：只保留 GB 简体 且 1080P
  if (title.includes("【幻樱字幕组】")) {
    if (title.includes("【BIG5_MP4】")) return false; // 排除 BIG5
    if (title.includes("【1280X720】")) return false; // 排除 720P
    // 必须同时包含 GB 简体 和 1920X1080
    if (!title.includes("【GB_MP4】") || !title.includes("【1920X1080】"))
      return false;
  }

  // 悠哈璃羽字幕社：排除 CHT 繁体
  if (title.includes("【悠哈璃羽字幕社】") && title.includes("[CHT]")) {
    return false;
  }

  // 集数区间（例如 [1-12] 或 (1-12)）也排除
  if (/\[\d{1,3}-\d{1,3}\]|\(\d{1,3}-\d{1,3}\)/.test(title)) return false;

  return true;
}
