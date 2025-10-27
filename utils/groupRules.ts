// 字幕组解析规则配置
// 每个字幕组定义自己的标题解析逻辑，返回按优先级排序的名称数组

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
export const groupRules: Record<string, (title: string) => string[]> = {
  // 三明治代餐部：[]后到-前所有名称，按 / _ ， , 分割
  三明治摆烂组: (title: string) => {
    let t = title.replace(/^\[[^\]]+\]\s*/, "");
    // 去除集数及后缀
    t = t.split(/\s*-\s*\d{1,3}|\s*\[\d{1,3}\]/)[0].trim();
    const names = t
      .split(/\s*\/\s*|_|，|,|、/)
      .map((s) => s.trim())
      .filter(Boolean);
    return sortNamesByPriority(names, title);
  },
  // 轻之国度字幕组：番剧名在第二个[]，支持中英文名用 / 分割，过滤集数和技术标记
  轻之国度字幕组: (title: string) => {
    // 提取所有[]内的内容
    const brackets = [...title.matchAll(/\[([^\]]+)\]/g)].map((m) => m[1]);
    // 跳过字幕组名，取第二个[]作为番剧名称
    if (brackets.length >= 2) {
      let animeName = brackets[1];
      // 过滤集数和技术标记
      if (/^\d{1,3}$|END|GB|MP4|1080P|720P/i.test(animeName)) {
        return [];
      }
      // 按 / 分割中英文名
      const names = animeName
        .split(/\s*\/\s*/)
        .map((s) => s.trim())
        .filter(Boolean);
      return sortNamesByPriority(names, title);
    }
    return [];
  },
  // 喵萌奶茶屋：第一个[]内所有名称，按 / _ ， , 分割
  喵萌奶茶屋: (title: string) => {
    const match = title.match(/\[([^\]]+)\]/);
    if (match) {
      const names = match[1]
        .split(/\s*\/\s*|_|，|,|、/)
        .map((s) => s.trim())
        .filter(Boolean);
      return sortNamesByPriority(names, title);
    }
    return [];
  },
  // 例：漫猫字幕组，番剧名在第3个[]
  爱恋字幕社: (title: string) => {
    const brackets = [...title.matchAll(/\[([^\]]+)\]/g)].map((m) => m[1]);
    // 跳过所有字幕组名（含“字幕组/汉化组/社/组/汉化”等关键字）
    const rest = brackets.filter(
      (b) =>
        !/(字幕组|汉化组|字幕社|汉化社|字幕|汉化)/.test(b) &&
        !/新番|1080P|720P|MP4|GB&JP|BIG5|CHS|CHT|简繁|双语|无字幕|整理搬运/i.test(
          b
        ) &&
        !/^\d{1,3}$/.test(b) // 跳过纯数字（集数）
    );
    // 过滤掉剧场版、SP、OVA、OAD、01v2等技术标记
    const valid = rest.filter(
      (b) =>
        b.length > 2 &&
        !/^(剧场版|SP|OVA|OAD)$/i.test(b) &&
        !/^\d{1,3}(v\d+)?$/i.test(b)
    );
    const names = valid.map((b) => b.trim());
    return sortNamesByPriority(names, title);
  },
  // 例：黒ネズミたち，番剧名在字幕组后第一个/分割后，集数在 - 数字格式
  "Kirara Fantasia": (title: string) => {
    let t = title.replace(/^\[[^\]]+\]\s*/, "");
    const parts = t.split(/\s*\/\s*/);
    const names = [];

    for (const part of parts) {
      let name = part.split(/\s*-\s*\d+/)[0].trim();

      // 移除括号内容但保留主要名称
      name = name.replace(/\s*\([^)]*\)\s*$/, "").trim();

      // 处理年龄限制版等特殊标记
      name = name.replace(/\s*\[年齡限制版\]\s*/, "").trim();

      if (name.length > 2) {
        names.push(name);
      }
    }

    return sortNamesByPriority(names.length > 0 ? names : [], title);
  },
  // ANi组：优先取 / 后中文名，无 / 时取字幕组后到 - 或集数前内容
  ANi: (title: string) => {
    let t = title.replace(/^\[[^\]]+\]\s*/, "");

    const parts = t.split(/\s*\/\s*/);
    const names = [];
    for (const part of parts) {
      let name = part.split(/\s*-\s*\d{1,3}|\s*\[\d{1,3}\]/)[0].trim();

      // 移除各种标记和括号内容，但保留年龄限制版等重要信息
      name = name.replace(/\[.*?\]|\(.*?\)/g, "").trim();

      // 处理年龄限制版等特殊标记
      name = name.replace(/\s*\[年齡限制版\]\s*/, "").trim();

      if (name.length > 2) names.push(name);
    }
    return sortNamesByPriority(names, title);
  },
  // 动漫国字幕组：去掉字幕组标识和月新番标识，取第一个[]内的内容
  動漫國字幕組: (title: string) => {
    let t = title.replace(/^【[^】]+】\s*/, "").replace(/★\d{1,2}月新番/g, "");

    const match = t.match(/\[([^\]]+)\]/);
    if (match) {
      const name = match[1].trim();

      if (!/^\d{1,3}$|1080P|MP4|简体|繁体|GB|BIG5|CHS|CHT/i.test(name)) {
        return sortNamesByPriority([name], title);
      }
    }
    return [];
  },
  // 澄空学园：类似动漫国字幕组的处理
  澄空学园: (title: string) => {
    let t = title.replace(/^【[^】]+】\s*/, "").replace(/★\d{1,2}月新番/g, "");
    const match = t.match(/\[([^\]]+)\]/);
    if (match) {
      const name = match[1].trim();
      if (!/^\d{1,3}$|1080P|MP4|简体|繁体|GB|BIG5|CHS|CHT/i.test(name)) {
        return sortNamesByPriority([name], title);
      }
    }
    return [];
  },
  // 华盟字幕社：类似处理
  华盟字幕社: (title: string) => {
    let t = title.replace(/^【[^】]+】\s*/, "").replace(/★\d{1,2}月新番/g, "");
    const match = t.match(/\[([^\]]+)\]/);
    if (match) {
      const name = match[1].trim();
      if (!/^\d{1,3}$|1080P|MP4|简体|繁体|GB|BIG5|CHS|CHT/i.test(name)) {
        return sortNamesByPriority([name], title);
      }
    }
    return [];
  },
  // Billion Meta Lab：字幕组后到第一个集数或分辨率前的内容
  亿次研同好会: (title: string) => {
    // 匹配 [Billion Meta Lab] 番剧名 [集数][分辨率][其它]
    const match = title.match(
      /^\[Billion Meta Lab\]\s*(.+?)\s*\[\d{1,3}\](?=\[|$)/
    );
    if (match) {
      return sortNamesByPriority([match[1].trim()], title);
    }
    let t = title.replace(/^\[Billion Meta Lab\]\s*/, "");
    t = t.split(/\[\d{3,4}P\]|\[简日内嵌\]/)[0].trim();
    const names = t.length > 0 ? [t] : [];
    return sortNamesByPriority(names, title);
  },
  // SweetSub：提取字幕组后面的[]内容作为番剧名称，支持检索用标记
  SweetSub: (title: string) => {
    const searchMatch = title.match(/（检索用：([^）]+)）/);
    let searchName = searchMatch ? searchMatch[1].trim() : null;

    const names = [];

    if (searchName) {
      names.push(searchName);
    }

    // 提取所有[]内的内容，排除字幕组名和技术标记
    const brackets = [...title.matchAll(/\[([^\]]+)\]/g)].map((m) => m[1]);

    for (const bracket of brackets) {
      // 跳过字幕组名和技术相关标记
      if (
        bracket === "SweetSub" ||
        /^\d{1,3}(?:v\d+)?$/.test(bracket) || // 集数
        /^(WebRip|HEVC|AVC|MP4|MKV)/.test(bracket) || // 编码格式
        /^\d{3,4}P?$/.test(bracket) || // 分辨率
        /^(简日双语|简日内嵌|双语|内嵌|8bit|10bit)/.test(bracket) // 语言和编码
      ) {
        continue;
      }

      // 检查是否是有效的番剧名称（长度大于2且不是纯技术标记）
      if (bracket.length > 2) {
        names.push(bracket.trim());
      }
    }

    return sortNamesByPriority(names.length > 0 ? names : [], title);
  },

  // ❀拨雪寻春❀：字幕组后番剧名有多个名称用 / 分割
  拨雪寻春: (title: string) => {
    const searchMatch = title.match(/（检索用：([^）]+)）/);
    let searchName = searchMatch ? searchMatch[1].trim() : null;

    let t = title.replace(/^\[❀拨雪寻春❀\]\s*/, "");

    t = t.split(/\s*-\s*\d{1,3}/)[0].trim();

    t = t
      .replace(/\[WebRip\]|\[HEVC[^\]]*\]|\[\d{3,4}P?\]|\[简日内嵌\]/g, "")
      .trim();

    // 只移除检索用标记，保留季度信息等重要括号内容
    t = t.replace(/（检索用：[^）]*）/g, "").trim();

    const names = [];

    if (searchName) {
      names.push(searchName);
    }

    // 按 / 分割多个名称并添加到结果中
    const otherNames = t
      .split(/\s*\/\s*/)
      .map((s) => s.trim())
      .filter(Boolean);
    names.push(...otherNames);

    return sortNamesByPriority(names.length > 0 ? names : [], title);
  },
  // 幻樱字幕组：支持多种格式，正确分割名称
  幻樱字幕组: (title: string) => {
    // 提取所有【】内的内容
    const segments = [...title.matchAll(/【([^】]+)】/g)].map((m) => m[1]);

    // 过滤掉字幕组名、月新番、技术标记等
    const validSegments = segments.filter(
      (segment) =>
        segment !== "幻樱字幕组" &&
        !/^\d{1,2}月新番$/.test(segment) &&
        !/^(GB|BIG5|MP4|MKV|1920X1080|1280X720|720P|1080P)(_.*)?$/.test(
          segment
        ) &&
        !/^\d{1,3}$/.test(segment) // 排除纯数字（这些是集数）
    );

    const names = [];

    // 查找包含番剧名称的段落（通常是最长的非技术段落）
    for (const segment of validSegments) {
      if (segment.length > 5) {
        // 番剧名称通常比较长
        // 处理复合名称，按 / 分割
        if (segment.includes("/")) {
          const parts = segment.split("/");
          for (const part of parts) {
            const trimmed = part.trim();
            if (trimmed.length > 1) {
              // 进一步分割混合的中英文名称
              if (/[\u4e00-\u9fff]/.test(trimmed) && /[a-zA-Z]/.test(trimmed)) {
                // 包含中英文混合，尝试分离
                const chineseMatch = trimmed.match(
                  /([\u4e00-\u9fff\u3400-\u4dbf]+[^\sa-zA-Z]*)/
                );
                const englishMatch = trimmed.match(/([a-zA-Z][a-zA-Z\s]*)/);

                if (chineseMatch) names.push(chineseMatch[1].trim());
                if (englishMatch) names.push(englishMatch[1].trim());
              } else {
                names.push(trimmed);
              }
            }
          }
        } else {
          // 处理单个名称，可能包含中英文混合
          if (/[\u4e00-\u9fff]/.test(segment) && /[a-zA-Z]/.test(segment)) {
            // 尝试分离中英文
            const parts = segment.split(/\s+/);
            let chinesePart = "";
            let englishPart = "";

            for (const part of parts) {
              if (/[\u4e00-\u9fff]/.test(part)) {
                chinesePart += part;
              } else if (/[a-zA-Z]/.test(part)) {
                englishPart += (englishPart ? " " : "") + part;
              }
            }

            if (chinesePart) names.push(chinesePart);
            if (englishPart) names.push(englishPart);
          } else {
            names.push(segment);
          }
        }
      }
    }

    return sortNamesByPriority(names.filter(Boolean), title);
  },
  星空字幕组: (title: string) => {
    // 只取第二个[]内容作为番剧名
    const brackets = [...title.matchAll(/\[([^\]]+)\]/g)].map((m) => m[1]);
    if (brackets.length >= 2) {
      let animeName = brackets[1];
      // 过滤集数和技术标记
      if (
        /^\d{1,3}$|END|GB|MP4|1080P|720P|WEBrip|简日双语|双语/i.test(animeName)
      ) {
        return [];
      }
      // 按 / 分割中英文名
      const names = animeName
        .split(/\s*\/\s*/)
        .map((s) => s.trim())
        .filter(Boolean);
      return sortNamesByPriority(names, title);
    }
    return [];
  },

  // 悠哈璃羽字幕社：排除CHT繁体版本，只保留CHS简体版本
  悠哈璃羽字幕社: (title: string) => {
    // 提取番剧名称，在第一个[]内
    const match = title.match(/\[([^\]]+)\]/);
    if (match) {
      const name = match[1].trim();
      // 过滤掉集数、分辨率等技术标记（仅当整个段落为技术标记时视为无效）
      if (!/^(?:\d{1,3}|END|1080P|720P|MP4|CHS|CHT|简体|繁体)$/i.test(name)) {
        // 按 / 或 _ 分割多个名称
        const names = name
          .split(/\s*\/\s*|_/)
          .map((s) => s.trim())
          .filter(Boolean);
        return sortNamesByPriority(names, title);
      }
    }
    return [];
  },

  // 猎户压制部：字幕组后直接是番剧名，包含中文和日文，用 / 分隔
  猎户压制部: (title: string) => {
    let t = title.replace(/^\[[^\]]*猎户[^\]]*\]\s*/, "");

    // 提取到第一个 [ 之前的内容作为番剧名称
    t = t.split(/\s*\[/)[0].trim();

    // 按 / 分割多个名称
    const names = t
      .split(/\s*\/\s*/)
      .map((s) => s.trim())
      .filter(Boolean);

    return sortNamesByPriority(names, title);
  },

  // 猎户发布组：字幕组后直接是番剧名，中英文用 / 分割，支持联合字幕组
  猎户发布组: (title: string) => {
    // 支持 [猎户压制部]、[猎户手抄部]、[学院部＆不鸽] 等
    let t = title.replace(/^\[[^\]]*猎户[^\]]*\]\s*/, "");
    // 支持联合字幕组
    t = t.replace(/^\[[^\]]*＆[^\]]*\]\s*/, "");
    // 提取到第一个 [ 集数 或 - 集数 或 [分辨率] 前的内容作为番剧名称
    t = t
      .split(/\s*\[\d{1,3}(-\d{1,3})?\]|\s*-\s*\d{1,3}|\s*\[\d{3,4}p?\]/i)[0]
      .trim();
    // 按 / 分割多个名称
    const names = t
      .split(/\s*\/\s*/)
      .map((s) => s.trim())
      .filter(Boolean);
    return sortNamesByPriority(names, title);
  },
  // 奇怪机翻组：字幕组后直接是番剧名，用 / 分割，到 - 集数前，支持检索用标记
  奇怪机翻组: (title: string) => {
    // 检查是否有检索用标记
    const searchMatch = title.match(/\(检索[：:]([^)]+)\)/);
    let searchName = searchMatch ? searchMatch[1].trim() : null;

    // 移除字幕组标识
    let t = title.replace(/^\[奇怪机翻组\]\s*/, "");

    // 提取到 - 集数前的内容作为番剧名称
    t = t.split(/\s*-\s*\d{1,3}/)[0].trim();

    // 按 / 分割多个名称
    const names = t
      .split(/\s*\/\s*/)
      .map((s) => s.trim())
      .filter(Boolean);

    // 如果有检索用名称，添加到数组前面
    if (searchName) {
      names.unshift(searchName);
    }

    return sortNamesByPriority(names, title);
  },

  // 晚街与灯：番剧名称在第二个[]中，使用下划线分隔中日文名称
  晚街与灯: (title: string) => {
    // 提取所有[]内的内容
    const brackets = [...title.matchAll(/\[([^\]]+)\]/g)].map((m) => m[1]);

    // 跳过字幕组名，取第二个[]作为番剧名称
    if (brackets.length >= 2) {
      const animeName = brackets[1]; // 第二个[]中的内容

      // 按 _ 分割中日文名称
      const names = animeName
        .split(/\s*_\s*/)
        .map((s) => s.trim())
        .filter(Boolean);

      return sortNamesByPriority(names, title);
    }

    return [];
  },

  // 云光字幕组：使用空格分隔中英文名称，到第一个[]前
  云光字幕组: (title: string) => {
    // 移除字幕组标识
    let t = title.replace(/^\[云光字幕组\]\s*/, "");

    // 提取到第一个 [ 之前的内容作为番剧名称
    t = t.split(/\s*\[/)[0].trim();

    // 先按空格分割，然后识别中英文部分
    const parts = t.split(/\s+/);
    const names = [];
    let currentChineseName = [];
    let currentEnglishName = [];

    for (const part of parts) {
      // 检查是否包含中文字符
      if (/[\u4e00-\u9fff\u3400-\u4dbf]/.test(part)) {
        // 如果有积累的英文名称，先添加
        if (currentEnglishName.length > 0) {
          names.push(currentEnglishName.join(" "));
          currentEnglishName = [];
        }
        currentChineseName.push(part);
      } else if (/^[a-zA-Z]+$/.test(part)) {
        // 如果有积累的中文名称，先添加
        if (currentChineseName.length > 0) {
          names.push(currentChineseName.join(""));
          currentChineseName = [];
        }
        currentEnglishName.push(part);
      }
    }

    // 添加剩余的名称
    if (currentChineseName.length > 0) {
      names.push(currentChineseName.join(""));
    }
    if (currentEnglishName.length > 0) {
      names.push(currentEnglishName.join(" "));
    }

    return sortNamesByPriority(names.filter(Boolean), title);
  },

  // MingY：支持检索用标记，用 / 分割名称
  MingYSub: (title: string) => {
    // 检查是否有检索用标记（括号形式）
    const searchMatch = title.match(/（([^）]+)）/);
    let searchName = searchMatch ? searchMatch[1].trim() : null;

    // 移除字幕组标识 - 修改正则表达式匹配包含其他字幕组的情况
    let t = title.replace(/^\[MingY[^[\]]*\]\s*/, "");

    // 提取到第一个 [ 之前的内容作为番剧名称
    t = t.split(/\s*\[/)[0].trim();

    // 移除检索用标记
    t = t.replace(/（[^）]*）/, "").trim();

    // 按 / 分割多个名称
    const names = t
      .split(/\s*\/\s*/)
      .map((s) => s.trim())
      .filter(Boolean);

    // 如果有检索用名称，添加到数组前面
    if (searchName) {
      names.unshift(searchName);
    }

    return sortNamesByPriority(names, title);
  },

  // Prejudice-Studio：字幕组后直接是番剧名（中英文混合），到 - 集数前，支持Bilibili来源
  "Prejudice-Studio": (title: string) => {
    // 移除字幕组标识
    let t = title.replace(/^\[Prejudice-Studio\]\s*/, "");

    // 提取到 - 集数前的内容作为番剧名称
    t = t.split(/\s*-\s*\d{1,3}/)[0].trim();

    const names = [];

    // 尝试分离中英文名称
    // 匹配模式1：中文名 英文名 (如: 坂本日常 SAKAMOTO DAYS)
    let match = t.match(
      /^([\u4e00-\u9fff\u3400-\u4dbf]+(?:\s*[\u4e00-\u9fff\u3400-\u4dbf]+)*)\s+([A-Z][A-Z\s]+(?:[A-Z]+)?)$/
    );

    if (match) {
      const chineseName = match[1].trim();
      const englishName = match[2].trim();

      if (chineseName) names.push(chineseName);
      if (englishName) names.push(englishName);
    } else {
      // 匹配模式2：中文名 英文名称 THE ANIMATION (如: 小城日常 CITY THE ANIMATION)
      match = t.match(
        /^([\u4e00-\u9fff\u3400-\u4dbf]+(?:\s*[\u4e00-\u9fff\u3400-\u4dbf]+)*)\s+([A-Z]+(?:\s+[A-Z]+)*)$/
      );

      if (match) {
        const chineseName = match[1].trim();
        const englishName = match[2].trim();

        if (chineseName) names.push(chineseName);
        if (englishName) names.push(englishName);
      } else {
        // 匹配模式3：纯中文名称 (如: 直至魔女消逝)
        if (/^[\u4e00-\u9fff\u3400-\u4dbf\s]+$/.test(t)) {
          names.push(t);
        } else if (t.length > 0) {
          // 如果无法精确分离，则作为整体名称
          names.push(t);
        }
      }
    }

    return sortNamesByPriority(names, title);
  },

  // 北宇治字幕组：字幕组后面是番剧名（多个中文名/英文名用/分割），到[集数]前
  北宇治字幕组: (title: string) => {
    // 移除字幕组标识
    let t = title.replace(/^\[北宇治字幕组\]\s*/, "");

    // 提取到第一个 [集数] 之前的内容作为番剧名称
    t = t.split(/\s*\[\d{1,3}\]/)[0].trim();

    // 按 / 分割多个名称
    const names = t
      .split(/\s*\/\s*/)
      .map((s) => s.trim())
      .filter(Boolean);

    return sortNamesByPriority(names, title);
  },

  // 桜都字幕组：字幕组后面是番剧名（中文名/英文名用/分割），到[集数]前
  桜都字幕组: (title: string) => {
    // 移除字幕组标识
    let t = title.replace(/^\[桜都字幕组\]\s*/, "");

    // 提取到第一个 [集数] 之前的内容作为番剧名称
    t = t.split(/\s*\[\d{1,3}\]/)[0].trim();

    // 按 / 分割多个名称
    const names = t
      .split(/\s*\/\s*/)
      .map((s) => s.trim())
      .filter(Boolean);

    return sortNamesByPriority(names, title);
  },

  // 云歌字幕组：字幕组后可能有月新番标记，番剧名在第二个或第三个[]中
  云歌字幕组: (title: string) => {
    // 提取所有[]内的内容
    const brackets = [...title.matchAll(/\[([^\]]+)\]/g)].map((m) => m[1]);

    // 过滤掉字幕组名、联合字幕组名、月新番、技术标记、集数等
    const validSegments = brackets.filter(
      (segment) =>
        segment !== "云歌字幕组" &&
        !/[&｜|]/.test(segment) && // 过滤联合字幕组名称
        !/^\d{1,2}月新番$/.test(segment) && // 过滤月新番标记
        !/^(HEVC|x265|x264|10bit|8bit|1080p|720P|MP4|MKV)/.test(segment) && // 过滤技术标记
        !/^(简日双语|双语|内嵌|招募校对|招募|校对)/.test(segment) && // 过滤其他标记
        !/^\d{1,3}$/.test(segment) && // 过滤集数
        !/^\d{1,3}-\d{1,3}$/.test(segment) // 过滤集数区间（合集）
    );

    // 查找番剧名称（通常是最长的有效段落）
    const animeNameSegment = validSegments.find(
      (segment) => segment.length > 2
    );

    const names = animeNameSegment ? [animeNameSegment.trim()] : [];
    return sortNamesByPriority(names, title);
  },

  // 樱桃花字幕组：字幕组后番剧名可能包含中文名和英文名，用/分割，到-集数前
  樱桃花字幕组: (title: string) => {
    // 移除字幕组标识（包括联合字幕组的情况）
    let t = title.replace(/^\[[^[\]]*樱桃花字幕组[^[\]]*\]\s*/, "");

    t = t.split(/\s*-\s*\d{1,3}/)[0].trim();

    t = t.replace(/\s*\[[^\]]*\]\s*$/, "").trim();

    const names = t
      .split(/\s*\/\s*/)
      .map((s) => s.trim())
      .filter(Boolean);

    return sortNamesByPriority(names, title);
  },
  // S1百综字幕组：字幕组后番剧名可能包含中文名和英文名，用/分割，到[集数]前
  S1百综字幕组: (title: string) => {
    // 移除字幕组标识（包括联合字幕组的情况）
    let t = title.replace(/^\[[^[\]]*S1百综字幕组[^[\]]*\]\s*/, "");

    t = t.split(/\s*\[\d{1,3}\]/)[0].trim();

    const names = t
      .split(/\s*\/\s*/)
      .map((s) => s.trim())
      .filter(Boolean);

    return sortNamesByPriority(names, title);
  },
};
