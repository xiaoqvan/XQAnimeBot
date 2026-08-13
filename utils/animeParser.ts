import { groupRules } from "./groupRules.ts";
import { OpenAI } from "openai";
import { recordAiCall } from "../database/ai.ts";

type ParseInfoResult = {
  names: string[];
  source: string;
  episode: string;
};

const EPISODE_AI_MODEL = "deepseek-v4-flash";

function getEpisodeAiClient(): OpenAI {
  const apiKey = process.env.DEEPSEEK_API_KEY ?? process.env.OPENAI_API_KEY;
  if (!apiKey) {
    throw new Error("缺少 API Key：请设置 DEEPSEEK_API_KEY 或 OPENAI_API_KEY");
  }
  const client = new OpenAI({
    apiKey,
    baseURL: process.env.DEEPSEEK_BASE_URL ?? "https://api.deepseek.com",
  });

  // 包装 completions.create，记录 AI 集数提取调用（供 Web 展示）
  const completions = client.chat.completions as unknown as {
    create: (...args: any[]) => any;
  };
  const originalCreate = completions.create.bind(completions);
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
        scene: "episode_extract",
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

function normalizeAiEpisode(raw: string): string | null {
  const value = raw.trim();
  if (!value) return null;
  if (/^未知$/i.test(value)) return null;
  if (/^(?:null|none|n\/a)$/i.test(value)) return null;

  // 只接受常见的单集/特殊集格式，避免模型回传长句污染流程
  if (/^\d{1,3}(?:v\d+)?$/i.test(value)) return value;
  if (/^(?:SP|OVA|OAD|Extra|番外|特典)\d{0,3}$/i.test(value)) return value;
  if (/^(?:剧场版|剧场总集篇|电影|SP)$/i.test(value)) return value;
  if (/^\d{1,3}\.\d$/.test(value)) return value;
  return null;
}

export async function extractEpisodeByAI(
  title: string,
  names: string[]
): Promise<string | null> {
  try {
    const client = getEpisodeAiClient();
    const nameHints = names.filter(Boolean).slice(0, 5).join(" / ");

    const completion = await client.chat.completions.create({
      model: EPISODE_AI_MODEL,
      temperature: 0,
      messages: [
        {
          role: "system",
          content:
            "你是动漫资源标题的集数提取器。请从标题中提取单个集数字段，仅输出 JSON：{\"episode\":\"...\"}。如果无法确定则输出 {\"episode\":\"未知\"}。禁止输出解释。",
        },
        {
          role: "user",
          content:
            `标题: ${title}\n` +
            `候选番名: ${nameHints || "(空)"}\n` +
            "可接受 episode 示例：01, 12, 02v2, SP, SP1, OVA2, 12.5, 剧场版, 电影。",
        },
      ],
      response_format: {
        type: "json_schema",
        json_schema: {
          name: "episode_extract",
          schema: {
            type: "object",
            properties: {
              episode: { type: "string" },
            },
            required: ["episode"],
            additionalProperties: false,
          },
        },
      },
    });

    const content = completion.choices[0]?.message?.content ?? "";
    if (!content) return null;

    const parsed = JSON.parse(content) as { episode?: unknown };
    if (typeof parsed.episode !== "string") return null;
    return normalizeAiEpisode(parsed.episode);
  } catch {
    // AI 兜底失败时静默回退，不影响主解析流程
    return null;
  }
}

/**
 * 格式化番剧信息
 * @param title - 动漫标题
 * @param teamName - 动漫发布组名称
 * @returns
 */
export function parseInfo(
  title: string,
  teamName: string | null
): ParseInfoResult | undefined {
  let names = [];
  let source = "";
  let episode;

  // 查找对应的字幕组规则
  const teamKeys = Object.keys(groupRules);
  const teamKey = teamKeys.find(
    (key) => teamName && teamName.toLowerCase().includes(key.toLowerCase())
  );

  if (!teamKey || !groupRules[teamKey]) {
    return; // 明确返回空数组
  }

  // 使用字幕组规则解析标题
  try {
    names = groupRules[teamKey](title);

    // 集数提取（支持区间和版本号）
    // 先检查是否是多集区间（支持版本号）
    const multiEpMatch = title.match(
      /\[(\d{1,3}(?:v\d+)?)-(\d{1,3}(?:v\d+)?)\]|\((\d{1,3}(?:v\d+)?)-(\d{1,3}(?:v\d+)?)\)/
    );
    let epMatch = null;

    if (multiEpMatch) {
      // 多集区间，返回集数数组
      const startStr = multiEpMatch[1] || multiEpMatch[3];
      const endStr = multiEpMatch[2] || multiEpMatch[4];

      // 提取数字部分和版本号部分
      const startMatch = startStr?.match(/^(\d{1,3})(v\d+)?$/);
      const endMatch = endStr?.match(/^(\d{1,3})(v\d+)?$/);

      if (startMatch && endMatch) {
        const startNum = startMatch[1] ?? "0";
        const endNum = endMatch[1] ?? "0";
        const start = parseInt(startNum);
        const end = parseInt(endNum);
        const startVersion = startMatch[2] || "";
        const endVersion = endMatch[2] || "";

        episode = [];
        for (let i = start; i <= end; i++) {
          // 保持原有的格式（如果原来是01，保持01的格式）
          const padLength = startNum.length;
          const paddedNum = i.toString().padStart(padLength, "0");

          // 如果是起始集数且有版本号，或者是结束集数且有版本号，保留版本号
          if (i === start && startVersion) {
            episode.push(paddedNum + startVersion);
          } else if (i === end && endVersion) {
            episode.push(paddedNum + endVersion);
          } else {
            episode.push(paddedNum);
          }
        }
      }
    } else {
      // 单集处理
      // 优先匹配 [数字+版本号] 格式的集数（如 [02v2]）
      epMatch = title.match(/\[(\d{1,3}(?:v+)?)\](?![^[]*\[)/);
      if (!epMatch) {
        // 适配 [03_卢恩城] 这种格式
        epMatch = title.match(/\[(\d{1,3})_/);
      }
      if (!epMatch) {
        // 适配 [03 标题] 这种格式
        epMatch = title.match(/\[(\d{1,3}(?:v\d+)?)\s+[^\]]+\]/);
      }
      if (!epMatch) {
        // 适配 [03 - 总第13] 这种格式
        epMatch = title.match(/\[(\d{1,3})\s*-\s*总第\d+/);
      }
      if (!epMatch) {
        // 尝试匹配中文方括号 【数字+版本号】 格式（如 【14】）
        epMatch = title.match(/【(\d{1,3}(?:v\d+)?)】/);
      }
      if (!epMatch) {
        // 尝试匹配 - 数字+版本号 格式（如 - 02v2）
        epMatch = title.match(/\s-\s(\d{1,3}(?:v\d+)?)(?:\s|$|\(|\[|【)/);
      }
      if (!epMatch) {
        // 尝试匹配 [数字] 格式的集数（纯数字）
        epMatch = title.match(/\[(\d{1,3})\](?![^[]*\[)/);
      }
      if (!epMatch) {
        // 尝试匹配中文方括号 【数字】 格式（纯数字）
        epMatch = title.match(/【(\d{1,3})】/);
      }
      if (!epMatch) {
        // 尝试匹配 - 数字 格式（如黒ネズミたち）
        epMatch = title.match(/\s-\s(\d{1,3})(?:\s|$|\()/);
      }
      if (!epMatch) {
        // 如果没有以上格式，再尝试其他格式
        epMatch = title.match(/(?:第|EP|ep)(\d{1,3}(?:v\d+)?)(?:话|集|話|集)/i);
      }
      if (!epMatch) {
        // 最后尝试匹配独立的数字（带版本号），但排除在番剧名称中的数字
        const brackets = [...title.matchAll(/\[([^\]]+)\]/g)];
        for (let i = brackets.length - 1; i >= 0; i--) {
          const bracket = brackets[i];
          if (!bracket) continue;
          const content = bracket[1] ?? "";
          if (content && /^\d{1,3}(?:v\d+)?$/.test(content)) {
            epMatch = [null, content];
            break;
          }
        }
      }
      if (!epMatch) {
        // 尝试匹配中文方括号内的独立数字
        const chineseBrackets = [...title.matchAll(/【([^】]+)】/g)];
        for (let i = chineseBrackets.length - 1; i >= 0; i--) {
          const bracket = chineseBrackets[i];
          if (!bracket) continue;
          const content = bracket[1] ?? "";
          if (content && /^\d{1,3}(?:v\d+)?$/.test(content)) {
            epMatch = [null, content];
            break;
          }
        }
      }
      if (!epMatch) {
        // 最后尝试匹配独立的纯数字，但排除在番剧名称中的数字
        const brackets = [...title.matchAll(/\[([^\]]+)\]/g)];
        for (let i = brackets.length - 1; i >= 0; i--) {
          const bracket = brackets[i];
          if (!bracket) continue;
          const content = bracket[1] ?? "";
          if (content && /^\d{1,3}$/.test(content)) {
            epMatch = [null, content];
            break;
          }
        }
      }
    }
    episode = epMatch ? epMatch[1] : null;

    // 特殊集数提取（包含繁体变体）
    if (!episode) {
      // 剧场版、剧场总集篇等（支持简体/繁体与外文关键字）
      if (
        /(?:剧场版|劇場版|剧场总集篇|劇場總集篇|Gekijouban|Eiga|Movie|MOVIE)/i.test(
          title
        )
      ) {
        if (/(?:剧场总集篇|劇場總集篇)/i.test(title)) {
          episode = "剧场总集篇";
        } else if (/(?:剧场版|劇場版)/i.test(title)) {
          episode = "剧场版";
        } else if (/(?:Gekijouban|Eiga|Movie|MOVIE)/i.test(title)) {
          episode = "剧场版";
        }
      }
      // 电影（简体/繁体）
      else if (/(?:电影|電影)/i.test(title)) {
        episode = "电影";
      }
      // 特别篇 / 特別篇 / Special / SP
      else if (/(?:特别篇|特別篇|Special|SP)/i.test(title)) {
        episode = "SP";
      }
      // 番外、Extra、OVA、OAD、特典（带数字或不带数字）
      else {
        // OVA/OAD/SP/Extra/番外/特典 + 数字（如 OVA2、OAD03、SP1、Extra1、番外2），包含繁体/英文学写法
        const specialEp = title.match(
          /(?:OVA|OAD|SP|Extra|番外|特典)[\s\-:]?(\d{1,3})/i
        );
        if (specialEp) {
          episode = specialEp[0];
        } else {
          // [OVA03]、[OAD01]、[SP1]、[Extra2]、[番外2] 这种括号内
          const bracketSpecial = title.match(
            /\[(OVA|OAD|SP|Extra|番外|特典)[\s\-:]?(\d{1,3})\]/i
          );
          if (bracketSpecial) {
            episode = (bracketSpecial[1] ?? "") + (bracketSpecial[2] ?? "");
          } else {
            // 小数集数，如 48.5
            const decimalEp = title.match(/[\s\-\[]?(\d{1,3}\.\d)[\s\)\]]/);
            if (decimalEp) {
              episode = decimalEp[1];
            }
          }
        }
      }
    }

    // 来源提取 - 改进的规则
    const sourcePatterns = [
      /\[([A-Za-z]+)\s+WEB-DL[^\]]*\]/i, // [Bilibili WEB-DL 1080P AVC 8bit AAC MKV] -> Bilibili
      /\[([A-Za-z]+)\]\[WEB-DL\]/i, // [Baha][WEB-DL] -> Baha
      /\(([A-Za-z-]+)\s+\d+x\d+/i, // (CR 1920x1080) -> CR, (B-Global 1920x1080) -> B-Global
      /\(([A-Za-z]+)\s+/i, // (ABEMA 1920x1080) -> ABEMA
    ];

    for (const pattern of sourcePatterns) {
      const match = title.match(pattern);
      if (match && match[1]) {
        const potentialSource = match[1].trim();
        // 排除技术标记，但允许平台名称
        if (
          !/^(WebRip|WEB-DL|MP4|MKV|AVC|HEVC|AAC|1080P|720P|CHT|CHS|GB|BIG5|x264|x265|10bit|8bit)$/i.test(
            potentialSource
          )
        ) {
          source = potentialSource;
          break;
        }
      }
    }

    // 如果上面的模式没有匹配到，尝试单独的[]标记
    if (!source) {
      const allBrackets = [...title.matchAll(/\[([^\]]+)\]/g)];
      for (const bracket of allBrackets) {
        const content = (bracket[1] ?? "").trim();
        // 检查是否是常见的来源平台
        if (
          /^(Baha|CR|Bilibili|Netflix|Amazon|Hulu|Funimation|iQIYI|Youku|ABEMA|B-Global)$/i.test(
            content
          )
        ) {
          source = content;
          break;
        }
      }
    }
  } catch {
    return; // 出错时也返回空
  }

  // 确保 episode 不为空，若为空则填充 "未知"
  if (Array.isArray(episode) && episode.length === 0) {
    episode = "未知";
  }
  if (
    episode === null ||
    episode === undefined ||
    (typeof episode === "string" && episode.trim() === "")
  ) {
    episode = "未知";
  }

  return { names, source, episode: String(episode) };
}
