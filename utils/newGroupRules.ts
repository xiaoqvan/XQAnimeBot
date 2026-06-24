/**
 * 名称优先级排序函数
 * @param names - 待排序的名称数组
 * @param title - 原始标题，用于检测检索用名称
 * @returns 按优先级排序的名称数组
 */
function sortNamesByPriority(names: string[], title = "") {
  const priority = {
    search: 1, // 检索用
    japanese: 2, // 日文名称
    chinese: 3, // 中文名称
    english: 4, // 英文名称
    other: 5, // 其他
  };

  return names
    .map((name) => ({
      name,
      type: getNameType(name, title),
      priority: priority[getNameType(name, title)],
    }))
    .sort((a, b) => a.priority - b.priority)
    .map((item) => item.name);
}

/**
 * 检查是否是检索用名称（支持多种格式）
 * @param name - 名称
 * @param title - 原始标题
 * @returns 名称类型
 */
function getNameType(name: string, title: string) {
  // 检查是否是检索用名称（支持多种格式）
  if (
    title.includes(`（检索用：${name}）`) ||
    title.includes(`(检索：${name})`) ||
    title.includes(`（${name}）`)
  ) {
    return "search";
  }

  // 检查是否包含日文字符（平假名、片假名、汉字在日文语境中）
  if (/[\u3040-\u309F\u30A0-\u30FF]/.test(name)) {
    return "japanese";
  }

  // 检查是否为中文（主要是简体中文常用字符和标点）
  if (/[\u4e00-\u9fff\u3400-\u4dbf]/.test(name) && !/[a-zA-Z]/.test(name)) {
    return "chinese";
  }

  // 检查是否主要是英文
  if (/^[a-zA-Z\s\-:'!?.]+$/.test(name)) {
    return "english";
  }

  // 混合或其他类型
  return "other";
}

/**
 * 字幕组解析规则对象
 * 每个字幕组对应一个函数，接收标题字符串，返回按优先级排序的名称数组
 */
export const groupRules: Record<
  string,
  (title: string) => { names: string[]; episode: string }
> = {
  // 三明治代餐部：[]后到-前所有名称，按 / _ ， , 分割
  三明治摆烂组: (title: string) => {
    let t = title.replace(/^\[[^\]]+\]\s*/, "");
    let episode = "";
    const episodeMatch = t.match(/(?:\s*-\s*(\d{1,3}))|(?:\s*\[(\d{1,3})\])/);
    if (episodeMatch) {
      episode = episodeMatch[1] || episodeMatch[2] || "";
    }
    const splitResult = t.split(/\s*-\s*\d{1,3}|\s*\[\d{1,3}\]/);
    t = (splitResult[0] ?? "").trim();
    const names = t
      .split(/\s*\/\s*|_|，|,|、/)
      .map((s) => s.trim())
      .filter(Boolean);
    return { names: sortNamesByPriority(names, title), episode };
  },
  // 轻之国度字幕组：通常为多个中/英名在第2个中括号中，后面有集数中括号或单独 - NN
  轻之国度字幕组: (title: string) => {
    // 去掉首个中括号（发布组标签）
    const t = title.replace(/^\[[^\]]+\]\s*/, "");
    let episode = "";

    // 优先匹配像 [07] 或 [01-24] 这样的中括号集数；也支持 " - 07 " 格式
    const episodeMatch = t.match(/\[(\d{1,3}(?:-\d{1,3})?)\]|\s*-\s*(\d{1,3})/);
    if (episodeMatch) {
      episode = episodeMatch[1] || episodeMatch[2] || "";
    }

    // 先尝试从第一个中括号中取出名称，如果没有则取到第一个中括号之前的内容
    const nameBracketMatch = t.match(/^\[([^\]]+)\]/);
    let nameStr = "";
    if (nameBracketMatch) {
      nameStr = nameBracketMatch[1] ?? "";
    } else {
      const splitResult = t.split(/\s*\[/);
      nameStr = (splitResult[0] ?? "").trim();
    }

    // 名称可能包含多个形式，用 / 、，、,、、分隔；有时中间带空格分割中英名
    const names = nameStr
      .split(/\s*\/\s*|_|，|,|、/)
      .map((s) => s.trim())
      .filter(Boolean);

    return { names: sortNamesByPriority(names, title), episode };
  },
  // 喵萌奶茶屋：支持标题中使用全角【】或中括号[]作为发布标签，番剧名称在第一个中括号内，集数通常为后续的中括号 (如 [04] 或 [01-24])
  喵萌奶茶屋: (title: string) => {
    // 去掉开头的发布组标签，支持【喵萌奶茶屋】和[喵萌奶茶屋]
    const t = title.replace(/^[【\[][^】\]]+[】\]]\s*/, "");
    let episode = "";

    // 提取所有中括号 [] 的内容
    const brackets = [...t.matchAll(/\[([^\]]+)\]/g)].map((m) => m[1] ?? "");

    // 找到第一个看起来像集数的中括号（纯数字或范围）
    const ep = brackets.find((b: string) => /^\d{1,3}(?:-\d{1,3})?$/.test(b));
    if (ep) episode = ep;

    // 找到第一个可能是番剧名称的中括号（排除技术标记、分辨率、编码、语言等）
    const nameCandidate = brackets.find(
      (b: string) =>
        !/^(?:\d{1,3}(?:-\d{1,3})?|END|GB|MP4|MKV|1080P|720P|WEBRip|WEB|HEVC|H264|H265|BDRip|先行版|简日双语|繁日双语|简日双语|简繁|繁日|简体|繁体)$/i.test(
          b
        ) && b.length > 2
    );

    // 退回策略：优先 nameCandidate，否则取第一个中括号内容，仍无则取第一个 [ 前的文本
    const splitResult = t.split(/\s*\[/);
    const nameStr = nameCandidate || brackets[0] || (splitResult[0] ?? "").trim();

    const names = nameStr
      .split(/\s*\/\s*|_|，|,|、/) // 按常见分隔符分割中英名
      .map((s) => s.trim())
      .filter(Boolean);

    return { names: sortNamesByPriority(names, title), episode };
  },
};
