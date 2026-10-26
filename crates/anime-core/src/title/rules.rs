//! 字幕组标题解析规则表。
//!
//! 每个字幕组的命名习惯都不一样：有的把番名放第一个 `[]`，有的放第二个，
//! 有的直接跟在中括号后面用 `/` 分隔中英日三语。TS 版用一份 900 行的规则表逐组硬编码，
//! 这里照搬同样的策略，只把「怎么切一刀」抽象成几个共用函数，剩下的每组仍是显式规则——
//! **不做通用猜测**，猜错一个字母整部番就匹配不上。
//!
//! 加新组：写一个 `fn`，然后往 [`RULES`] 里加一行。没有规则时走 [`generic`] 兜底。

use std::sync::LazyLock;

use super::brackets;

/// 组名 → 解析函数。表本身就是查找表，查找时先精确、再「包含」。
pub type Rule = fn(&str) -> Vec<String>;

pub static RULES: &[(&str, Rule)] = &[
    ("三明治摆烂组", sandwich),
    ("雪飄工作室", flsnow),
    ("雪飄工作室(FLsnow)", flsnow),
    ("轻之国度字幕组", light_novel),
    ("喵萌奶茶屋", nekomoe),
    ("爱恋字幕社", ailian),
    ("Kirara Fantasia", kirara),
    ("ANi", ani),
    ("動漫國字幕組", cn_bracket_after_group),
    ("澄空学园", cn_bracket_after_group),
    ("华盟字幕社", cn_bracket_after_group),
    ("亿次研同好会", billion_meta),
    ("SweetSub", sweet_sub),
    ("拨雪寻春", boxing_spring),
    ("星空字幕组", second_bracket),
    ("隣天使字幕组", second_bracket_unfiltered),
    ("悠哈璃羽字幕社", yuha),
    ("猎户压制部", orion),
    ("猎户发布组", orion),
    ("北宇治字幕组", cut_at_episode_bracket),
    ("桜都字幕组", cut_at_episode_bracket),
    ("云歌字幕组", yunge),
    ("樱桃花字幕组", cherry),
    ("S1百综字幕组", s1),
    ("绿茶字幕组", green_tea),
    ("LoliHouse", lolihouse),
    ("云光字幕组", cut_at_episode_bracket),
    ("MingYSub", ming_y_sub),
    ("奇怪机翻组", cut_at_episode_bracket),
    ("晚街与灯", cut_at_episode_bracket),
    ("Prejudice-Studio", cut_at_episode_bracket),
    ("百冬练习组", hundred_winter),
    ("百冬練習組", hundred_winter),
    ("幻樱字幕组", cut_at_episode_bracket),
];

/// 组名别名：发布组自报的名字和规则 key 对不上时，先精确归到规则组。
pub static ALIASES: &[(&str, &str)] = &[
    // 悠哈系：同一个组在不同站点/不同时期用过简体、繁体、简称三种写法，
    // 少一条就会退到「包含匹配」，而「悠哈C9漢化組」里并不含「悠哈璃羽字幕社」。
    ("悠哈C9字幕社", "悠哈璃羽字幕社"),
    ("悠哈C9字幕组", "悠哈璃羽字幕社"),
    ("悠哈C9漢化組", "悠哈璃羽字幕社"),
    ("悠哈C9汉化组", "悠哈璃羽字幕社"),
    ("YUI-7&悠哈C9字幕组", "悠哈璃羽字幕社"),
    ("望月月", "悠哈璃羽字幕社"),
    ("萌樱字幕组", "云歌字幕组"),
    ("微雨恋樱", "樱桃花字幕组"),
];

/// 按发布组名找规则：先查别名（精确），再按「规则名是发布组名的子串」匹配。
///
/// TS 版就是这么两级，顺序不能反：`ANiTorrent` 里含 `ANi`，
/// 但别名表里 `ANiTorrent` 精确指向 `ANi`，得先走精确那条。
pub fn rule_for(team: &str) -> Option<Rule> {
    let lowered = team.to_lowercase();
    for (alias, target) in ALIASES {
        if alias.to_lowercase() == lowered {
            return RULES
                .iter()
                .find(|(name, _)| *name == *target)
                .map(|(_, f)| *f);
        }
    }
    RULES
        .iter()
        .find(|(name, _)| lowered.contains(&name.to_lowercase()))
        .map(|(_, f)| *f)
}

// ── 共用刀法 ──────────────────────────────────────────────────────

/// 去掉标题最前面那个字幕组标签（`[xx组] 「」【】`）。
fn strip_group(title: &str) -> String {
    static P: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^\[[^\[\]]*\]\s*|^【[^【】]*】\s*").expect("内置正则必须合法")
    });
    // 联合字幕组会有连续两个标签。
    let once = P.replace(title, "").to_string();
    if once.len() < title.len()
        && (once.trim_start().starts_with('[') || once.trim_start().starts_with('【'))
    {
        P.replace(&once, "").to_string()
    } else {
        once
    }
}

/// 切到第一个「集数括号」之前（`[01]`、`[01v2]`、`[1080P]`、`[简日内嵌]` 都算）。
fn cut_before_episode(t: &str) -> &str {
    static P: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"\s*\[\d{1,3}(?:v\d+)?(?:-\d{1,3})?\]|\s*\[\d{3,4}[pP]?\]|\s*\[简日内嵌\]|\s*\[(?:简日双语|双语|内嵌)\]",
        )
        .expect("内置正则必须合法")
    });
    match P.find(t) {
        Some(m) => &t[..m.start()],
        None => t,
    }
}

/// 按 `/`（含全角）切分多语言名。
fn split_slash(t: &str) -> Vec<String> {
    static P: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*/\s*|\s*／\s*|\s*／").expect("内置正则必须合法"));
    P.split(t)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 名称优先级：检索用 > 日文 > 中文 > 英文 > 其他。
///
/// 顺序就是匹配时的尝试顺序——检索用名是发布组专门给的「去查 Bangumi 用这个」，
/// 比标题里的展示名准得多。
pub fn sort_names_by_priority(names: Vec<String>, title: &str) -> Vec<String> {
    static JP: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"[\u{3040}-\u{309F}\u{30A0}-\u{30FF}]").expect("内置正则必须合法")
    });
    static CJK: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"[\u{4E00}-\u{9FFF}\u{3400}-\u{4DBF}]").expect("内置正则必须合法")
    });
    static LATIN: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^[a-zA-Z\s\-:'!?.]+$").expect("内置正则必须合法"));

    let kind_rank = |name: &str| -> u8 {
        if title.contains(&format!("（检索用：{name}）"))
            || title.contains(&format!("(检索：{name})"))
            || title.contains(&format!("（{name}）"))
        {
            return 1;
        }
        if JP.is_match(name) {
            return 2;
        }
        if CJK.is_match(name) && !name.chars().any(|c| c.is_ascii_alphabetic()) {
            return 3;
        }
        if LATIN.is_match(name) {
            return 4;
        }
        5
    };

    let mut ranked: Vec<(u8, String)> = names
        .into_iter()
        .map(|name| (kind_rank(&name), name))
        .collect();
    // 稳定排序：同类型保持原顺序（发布组自己排的优先级）。
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, name)| name).collect()
}

// ── 各组规则 ──────────────────────────────────────────────────────

/// 三明治摆烂组：`[组] A / B - 01 [tags]`。
fn sandwich(title: &str) -> Vec<String> {
    static CUT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\s*-\s*\d{1,3}|\s*\[\d{1,3}\]").expect("内置正则必须合法")
    });
    static TAIL: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*(\[[^\]]+\]\s*)+$").expect("内置正则必须合法"));
    let t = strip_group(title);
    let cut = match CUT.find(&t) {
        Some(m) => &t[..m.start()],
        None => t.as_str(),
    };
    let trimmed = TAIL.replace(cut, "").to_string();
    let names = split_multi(&trimmed);
    sort_names_by_priority(names, title)
}

/// 雪飄工作室：番名在第二个 `[]`，可能要剥技术标记。
fn flsnow(title: &str) -> Vec<String> {
    let search = search_name(title);
    let blocks = brackets(title);
    let mut names: Vec<String> = Vec::new();
    if blocks.len() >= 2 {
        static TECH: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(
                r"(?i)\b(?:1080P|720P|MP4|MKV|WEBRip|HEVC|x265|x264|简繁外挂|简体内嵌|简繁|简体|繁体|外挂|内嵌)\b",
            )
            .expect("内置正则必须合法")
        });
        let name_field = TECH.replace_all(&blocks[1], "").to_string();
        let mut parts: Vec<String> = regex::Regex::new(r"\s*/\s*|／|_|　")
            .expect("内置正则必须合法")
            .split(&name_field)
            .map(|s| s.trim().replace(['\u{3000}', '_'], ""))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        names.append(&mut parts);
    }
    if let Some(search) = search {
        names.insert(0, search);
    }
    static NOISE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(?:\d{1,3}|简体|繁体|外挂|内嵌)$").expect("内置正则必须合法")
    });
    names.retain(|n| n.chars().count() > 1 && !NOISE.is_match(n));
    sort_names_by_priority(names, title)
}

fn light_novel(title: &str) -> Vec<String> {
    second_bracket_filtered(title, r"^\d{1,3}$|END|GB|MP4|1080P|720P")
}

/// 喵萌奶茶屋。
///
/// ⚠️ **这里与 TS 当前在用的 `groupRules.ts` 不同**，采纳的是 TS 里已写好但还没接入
/// 的 `utils/newGroupRules.ts`。原因：旧规则直接取**第一个**中括号，而喵萌的真实
/// 标题是 `[喵萌奶茶屋] 葬送的芙莉莲 - 05 [...]`——第一个括号是**发布组自己**，
/// 于是番名被解析成「喵萌奶茶屋」。实测后果：Agent 拿这个错误关键词去搜 Bangumi，
/// 候选列表为空，只能靠模型自己从标题原文再搜一轮（多花一轮、且可能搜偏）。
///
/// 修正后的顺序：先剥掉开头的组标签 → 在剩下的括号里挑「不像技术参数的那个」→
/// 仍没有就取第一个 `[` 之前的文本（番名写在括号外的情形）。
fn nekomoe(title: &str) -> Vec<String> {
    // 组标签可能是 `[组]` 也可能是 `【组】`，两种都要剥。
    static GROUP_TAG: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^[【\[][^】\]]+[】\]]\s*").expect("内置正则必须合法")
    });
    // 集数写法：` - 05` 或 `[05]`。
    static CUT: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*-\s*\d+|\s*\[\d+\]").expect("内置正则必须合法"));
    // 剩下所有 `[...]` / `(...)` 都是技术参数，一并去掉。
    static MARKS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\[[^\]]*\]|\([^)]*\)").expect("内置正则必须合法"));
    // 番名写在括号里时（旧格式 `[组][番名][05]`），用它剔除纯技术参数。
    static TECH: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^(?:END|GB|MP4|MKV|1080P|720P|WEBRip|WEB|HEVC|H264|H265|BDRip|先行版|简日双语|繁日双语|简繁|繁日|简体|繁体)$",
        )
        .expect("内置正则必须合法")
    });

    let stripped = GROUP_TAG.replace(title, "").to_string();

    // ① 番名在括号外（现主流形态 `[组] 番名 - 05 [参数]`）：剥组标签后砍掉集数与
    //    所有括号，剩下的就是番名。与 `lolihouse` 同一套处理。
    let outside: Vec<String> = split_multi(&stripped)
        .into_iter()
        .map(|part| {
            let cut = CUT
                .find(&part)
                .map(|m| part[..m.start()].to_owned())
                .unwrap_or(part);
            MARKS.replace_all(&cut, "").trim().to_owned()
        })
        .filter(|name| name.chars().count() > 2)
        .collect();
    if !outside.is_empty() {
        return sort_names_by_priority(outside, title);
    }

    // ② 番名在括号里（旧形态 `[组][番名][05]`）：挑第一个不像技术参数的括号。
    let blocks = brackets(&stripped);
    let candidate = blocks
        .iter()
        .find(|block| !TECH.is_match(block) && block.chars().count() > 2)
        .or_else(|| blocks.first());
    match candidate {
        Some(name) => sort_names_by_priority(split_multi(name), title),
        None => Vec::new(),
    }
}

fn ailian(title: &str) -> Vec<String> {
    static SKIP: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)(字幕组|汉化组|字幕社|汉化社|字幕|汉化)|新番|1080P|720P|MP4|GB&JP|BIG5|CHS|CHT|简繁|双语|无字幕|整理搬运|^\d{1,3}$",
        )
        .expect("内置正则必须合法")
    });
    static NOISE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(?:剧场版|SP|OVA|OAD)$|^\d{1,3}(?:v\d+)?$")
            .expect("内置正则必须合法")
    });
    let names: Vec<String> = brackets(title)
        .into_iter()
        .filter(|b| !SKIP.is_match(b))
        .filter(|b| b.chars().count() > 2 && !NOISE.is_match(b))
        .map(|b| b.trim().to_owned())
        .collect();
    sort_names_by_priority(names, title)
}

fn kirara(title: &str) -> Vec<String> {
    static CUT: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*-\s*\d+").expect("内置正则必须合法"));
    static TAIL_PAREN: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*\([^)]*\)\s*$").expect("内置正则必须合法"));
    let t = strip_group(title);
    let names: Vec<String> = split_slash(&t)
        .into_iter()
        .map(|part| {
            let cut = match CUT.find(&part) {
                Some(m) => part[..m.start()].to_owned(),
                None => part,
            };
            TAIL_PAREN.replace(&cut, "").trim().to_owned()
        })
        .filter(|n| n.chars().count() > 2)
        .collect();
    sort_names_by_priority(names, title)
}

fn ani(title: &str) -> Vec<String> {
    static CUT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\s*-\s*\d{1,3}|\s*\[\d{1,3}\]").expect("内置正则必须合法")
    });
    static PARENS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\[.*?\]|\(.*?\)").expect("内置正则必须合法"));
    let t = strip_group(title);
    let names: Vec<String> = split_slash(&t)
        .into_iter()
        .map(|part| {
            let cut = match CUT.find(&part) {
                Some(m) => part[..m.start()].to_owned(),
                None => part,
            };
            PARENS.replace_all(&cut, "").trim().to_owned()
        })
        .filter(|n| n.chars().count() > 2)
        .collect();
    sort_names_by_priority(names, title)
}

/// 動漫國 / 澄空 / 华盟：去掉 `【组】` 与「★X月新番」后取第一个 `[]`。
fn cn_bracket_after_group(title: &str) -> Vec<String> {
    static PREFIX: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^【[^】]+】\s*").expect("内置正则必须合法"));
    static SEASON: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"★\d{1,2}月新番").expect("内置正则必须合法"));
    static NOISE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)^\d{1,3}$|1080P|MP4|简体|繁体|GB|BIG5|CHS|CHT")
            .expect("内置正则必须合法")
    });
    let t = SEASON
        .replace_all(&PREFIX.replace(title, ""), "")
        .to_string();
    match brackets(&t).first() {
        Some(name) if !NOISE.is_match(name) => {
            sort_names_by_priority(vec![name.trim().to_owned()], title)
        }
        _ => Vec::new(),
    }
}

fn billion_meta(title: &str) -> Vec<String> {
    let t = strip_group(title);
    let cut = cut_before_episode(&t);
    let name = cut.trim();
    if name.is_empty() {
        Vec::new()
    } else {
        sort_names_by_priority(vec![name.to_owned()], title)
    }
}

fn sweet_sub(title: &str) -> Vec<String> {
    static NOISE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^\d{1,3}(?:v\d+)?$|^(?:Movie|BDRip|BluRay|BD|WEB-?DL|WebRip|HEVC|AVC|MP4|MKV)|^\d{3,4}P?$|^(?:简日双语|简日内嵌|双语|内嵌|8bit|10bit)",
        )
        .expect("内置正则必须合法")
    });
    let mut names: Vec<String> = search_name(title).into_iter().collect();
    for block in brackets(title) {
        if block == "SweetSub" || NOISE.is_match(&block) || block.chars().count() <= 2 {
            continue;
        }
        names.push(block.trim().to_owned());
    }
    sort_names_by_priority(names, title)
}

fn boxing_spring(title: &str) -> Vec<String> {
    static PREFIX: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^\[❀?拨雪寻春❀?\]\s*").expect("内置正则必须合法"));
    static CUT: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*-\s*\d{1,3}").expect("内置正则必须合法"));
    static TECH: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\[WebRip\]|\[HEVC[^\]]*\]|\[\d{3,4}P?\]|\[简日内嵌\]")
            .expect("内置正则必须合法")
    });
    static SEARCH_TAG: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"（检索用：[^）]*）").expect("内置正则必须合法"));
    let t = PREFIX.replace(title, "").to_string();
    let t = match CUT.find(&t) {
        Some(m) => t[..m.start()].to_owned(),
        None => t,
    };
    let t = SEARCH_TAG
        .replace_all(&TECH.replace_all(&t, ""), "")
        .to_string();
    let mut names: Vec<String> = search_name(title).into_iter().collect();
    names.extend(split_slash(t.trim()));
    names.retain(|n| !n.is_empty());
    sort_names_by_priority(names, title)
}

fn second_bracket(title: &str) -> Vec<String> {
    second_bracket_filtered(
        title,
        r"(?i)^\d{1,3}$|END|GB|MP4|1080P|720P|WEBrip|简日双语|双语",
    )
}

fn second_bracket_filtered(title: &str, noise: &str) -> Vec<String> {
    let blocks = brackets(title);
    if blocks.len() < 2 {
        return Vec::new();
    }
    let noise = regex::Regex::new(noise).expect("内置正则必须合法");
    if noise.is_match(&blocks[1]) {
        return Vec::new();
    }
    sort_names_by_priority(split_slash(&blocks[1]), title)
}

fn second_bracket_unfiltered(title: &str) -> Vec<String> {
    let blocks = brackets(title);
    if blocks.len() < 2 {
        return Vec::new();
    }
    sort_names_by_priority(split_slash(&blocks[1]), title)
}

fn yuha(title: &str) -> Vec<String> {
    static NOISE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(?:\d{1,3}|END|1080P|720P|MP4|CHS|CHT|简体|繁体)$")
            .expect("内置正则必须合法")
    });
    match brackets(title).first() {
        Some(name) if !NOISE.is_match(name) => {
            sort_names_by_priority(split_underscore(name), title)
        }
        _ => Vec::new(),
    }
}

fn orion(title: &str) -> Vec<String> {
    static STRIP_HUNTER: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^\[[^\]]*猎户[^\]]*\]\s*").expect("内置正则必须合法"));
    static STRIP_JOINT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^\[[^\]]*[＆&][^\]]*\]\s*").expect("内置正则必须合法")
    });
    static TAIL_DOMAIN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)\s*(bgm\.tv|bilibili|b站)\s*$").expect("内置正则必须合法")
    });
    static TAIL_TAG: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*\[[^\]]*\]\s*$").expect("内置正则必须合法"));

    let stripped = STRIP_HUNTER.replace(title, "").to_string();
    let t = if stripped == title {
        STRIP_JOINT.replace(title, "").to_string()
    } else {
        stripped
    };
    let t = TAIL_DOMAIN.replace(&t, "").to_string();
    let cut = cut_before_episode(&t);
    let cut = TAIL_TAG.replace(cut, "").to_string();
    let mut names = split_slash(cut.trim());
    if names.is_empty() {
        let fallback = strip_group(title);
        let fallback = fallback.split('[').next().unwrap_or("").trim();
        let fallback = TAIL_DOMAIN.replace(fallback, "").to_string();
        names = split_slash(fallback.trim());
    }
    names.retain(|n| !n.is_empty());
    sort_names_by_priority(names, title)
}

/// 最常见的形态：`[组] 番名 / 别名 - 01 [tags]`，切到第一个集数括号前。
fn cut_at_episode_bracket(title: &str) -> Vec<String> {
    let t = strip_group(title);
    let cut = cut_before_episode(&t);
    let cut = cut.trim();
    let mut names = split_slash(cut);
    names.retain(|n| !n.is_empty() && !is_tech_token(n));
    sort_names_by_priority(names, title)
}

/// LoliHouse：`[LoliHouse] 中文名 / English Name - 04 [WebRip 1080p HEVC-10bit AAC][简体内嵌]`。
///
/// 不能复用 [`cut_at_episode_bracket`]：那个只认 `[04]` 这类「集数单独成括号」的写法，
/// 而 LoliHouse 把集数写成 ` - 04`、技术参数塞进后续 `[]` 里；漏切的话整段
/// `- 04 [WebRip …]` 会被当成英文名送去 Bangumi 检索，命中率直接归零。
/// 所以这里单独按 `/` 拆、按 ` - 数字` / `[数字]` 截断，再剥掉残留的 `[]`/`()` 标记。
fn lolihouse(title: &str) -> Vec<String> {
    static PREFIX: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^\[LoliHouse\]\s*").expect("内置正则必须合法"));
    static CUT: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*-\s*\d+|\s*\[\d+\]").expect("内置正则必须合法"));
    static MARKS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\[[^\]]*\]|\([^)]*\)").expect("内置正则必须合法"));

    let t = PREFIX.replace(title, "").to_string();
    let names: Vec<String> = split_slash(&t)
        .into_iter()
        .map(|part| {
            let cut = match CUT.find(&part) {
                Some(m) => part[..m.start()].to_owned(),
                None => part,
            };
            MARKS.replace_all(&cut, "").trim().to_owned()
        })
        // 两字以内基本是 `01`、`SP` 之类的残留；真正的番名不会这么短。
        .filter(|name| name.chars().count() > 2)
        .collect();
    sort_names_by_priority(names, title)
}

/// MingYSub：`[MingYSub] 中文名 / English Name（日文名）[01][1080P]`。
///
/// TS 版把「第一个全角括号」整段当检索用名——它的标题习惯直接写 `（日文名）`，
/// 不是 `（检索用：…）`，所以不能复用 [`search_name`]（那个只认「检索用」前缀）。
/// 也正因为这个名字被单独取走、又以 `（日文名）` 的形式参与
/// [`sort_names_by_priority`] 的检索用判定，日文名才会排在中文名前面。
fn ming_y_sub(title: &str) -> Vec<String> {
    static PREFIX: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^\[MingY[^\[\]]*\]\s*").expect("内置正则必须合法"));
    static PAREN: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"（([^）]+)）").expect("内置正则必须合法"));
    static PAREN_ANY: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"（[^）]*）").expect("内置正则必须合法"));

    let search = PAREN
        .captures(title)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_owned())
        .filter(|name| !name.is_empty());
    // 番名只到第一个 `[` 为止，后面都是 `[01][1080P]` 这类技术信息。
    let t = PREFIX.replace(title, "").to_string();
    let t = t.split('[').next().unwrap_or("").trim().to_owned();
    // 括号里的检索用名已经取出，正文里再留一份会被当成普通番名候选。
    let t = PAREN_ANY.replace(&t, "").trim().to_owned();

    let mut names = split_slash(&t);
    if let Some(search) = search {
        names.insert(0, search);
    }
    sort_names_by_priority(names, title)
}

fn yunge(title: &str) -> Vec<String> {
    static NOISE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"[&｜|]|^\d{1,2}月新番$|^(?:HEVC|x265|x264|10bit|8bit|1080p|720P|MP4|MKV)|^(?:简日双语|双语|内嵌|招募校对|招募|校对)|^\d{1,3}$|^\d{1,3}-\d{1,3}$",
        )
        .expect("内置正则必须合法")
    });
    let pick = brackets(title)
        .into_iter()
        .find(|b| b.chars().count() > 2 && !NOISE.is_match(b) && *b != "云歌字幕组");
    match pick {
        Some(name) => sort_names_by_priority(vec![name.trim().to_owned()], title),
        None => Vec::new(),
    }
}

fn cherry(title: &str) -> Vec<String> {
    static PREFIX: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^\[[^\[\]]*樱桃花字幕组[^\[\]]*\]\s*").expect("内置正则必须合法")
    });
    static CUT: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*-\s*\d{1,3}").expect("内置正则必须合法"));
    static TAIL_TAG: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*\[[^\]]*\]\s*$").expect("内置正则必须合法"));
    let t = PREFIX.replace(title, "").to_string();
    let t = match CUT.find(&t) {
        Some(m) => t[..m.start()].to_owned(),
        None => t,
    };
    let t = TAIL_TAG.replace(&t, "").to_string();
    sort_names_by_priority(split_slash(t.trim()), title)
}

fn s1(title: &str) -> Vec<String> {
    static PREFIX: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^\[[^\[\]]*S1百综字幕组[^\[\]]*\]\s*").expect("内置正则必须合法")
    });
    let t = PREFIX.replace(title, "").to_string();
    let cut = cut_before_episode(&t);
    sort_names_by_priority(split_slash(cut.trim()), title)
}

fn green_tea(title: &str) -> Vec<String> {
    let t = strip_group(title);
    let cut = t.split('[').next().unwrap_or("").trim();
    sort_names_by_priority(split_slash(cut), title)
}

/// 百冬练习组：番名在第二个 `【】`，用 `_` 切。
fn hundred_winter(title: &str) -> Vec<String> {
    let blocks: Vec<String> = cn_brackets(title);
    match blocks.get(1) {
        Some(name) => sort_names_by_priority(split_underscore(name), title),
        None => Vec::new(),
    }
}

/// 没有规则时的兜底：和 [`cut_at_episode_bracket`] 同策略，但更宽松。
pub fn generic(title: &str) -> Vec<String> {
    cut_at_episode_bracket(title)
}

// ── 小工具 ────────────────────────────────────────────────────────

/// 按 `/ _ ， , 、` 切多名称（三明治 / 喵萌 用的那套分隔符）。
fn split_multi(t: &str) -> Vec<String> {
    static P: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*/\s*|_|，|,|、").expect("内置正则必须合法"));
    static TAIL: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*(\[[^\]]+\]\s*)+$").expect("内置正则必须合法"));
    P.split(t)
        .map(|s| TAIL.replace(s, "").trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn split_underscore(t: &str) -> Vec<String> {
    static P: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s*/\s*|_").expect("内置正则必须合法"));
    P.split(t)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `【】` 块内容（百冬这种用中文括号的）。
fn cn_brackets(title: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = title;
    while let Some(start) = rest.find('【') {
        let after = &rest[start + '【'.len_utf8()..];
        match after.find('】') {
            Some(end) => {
                out.push(after[..end].to_owned());
                rest = &after[end + '】'.len_utf8()..];
            }
            None => break,
        }
    }
    out
}

/// 「（检索用：xxx）」里的那个名字。
fn search_name(title: &str) -> Option<String> {
    static P: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"[（(]\s*检索(?:用)?[:：]\s*([^）)]+)\s*[）)]")
            .expect("内置正则必须合法")
    });
    P.captures(title)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn is_tech_token(s: &str) -> bool {
    static P: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(?:\d{1,3}|END|1080P|720P|MP4|MKV|GB|BIG5|CHS|CHT|简体|繁体|简日内嵌|简日双语|双语|内嵌|WEBRip|HEVC|AVC|\d{3,4}P?)$")
            .expect("内置正则必须合法")
    });
    P.is_match(s.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_lookup_prefers_exact_alias_then_contains() {
        assert!(rule_for("悠哈C9字幕社").is_some(), "别名要能精确归组");
        assert!(
            rule_for("ANiTorrent").is_some(),
            "包含匹配要能兜住 ANiTorrent"
        );
        assert!(rule_for("不存在的组").is_none());
    }

    #[test]
    fn ani_style_title_splits_both_languages() {
        let names =
            ani("[ANi] 葬送的芙莉莲 / Sousou no Frieren - 01 [1080P][Baha][WEB-DL][AAC AVC][CHT]");
        assert_eq!(names.first().map(String::as_str), Some("葬送的芙莉莲"));
        assert!(names.iter().any(|n| n == "Sousou no Frieren"));
    }

    #[test]
    fn cut_rule_stops_at_episode_bracket() {
        let names =
            cut_at_episode_bracket("[北宇治字幕组] 吹响！上低音号 / Hibike! Euphonium [01][1080P]");
        assert_eq!(names.len(), 2, "{names:?}");
        assert_eq!(names[0], "吹响！上低音号");
    }

    #[test]
    fn search_name_has_top_priority() {
        let names =
            flsnow("[雪飄工作室] 葬送的芙莉莲 / Frieren（检索用：葬送のフリーレン）[01][1080P]");
        assert_eq!(names[0], "葬送のフリーレン", "检索用名要排第一：{names:?}");
    }

    #[test]
    fn noise_tokens_are_not_names() {
        assert!(is_tech_token("1080P"));
        assert!(is_tech_token("简体"));
        assert!(!is_tech_token("葬送的芙莉莲"));
    }

    #[test]
    fn nekomoe_does_not_mistake_the_group_tag_for_a_title() {
        // 这是修过的一个 bug：旧规则直接取第一个中括号，而喵萌的第一个括号是
        // 「喵萌奶茶屋」自己，于是番名被解析成组名，Agent 拿它去搜 Bangumi 空手而归。
        let names = nekomoe("[喵萌奶茶屋] 葬送的芙莉莲 - 05 [1080p][繁日双语]");
        assert_eq!(names, vec!["葬送的芙莉莲"], "{names:?}");
    }

    #[test]
    fn nekomoe_handles_both_outside_and_inside_bracket_titles() {
        // 现主流形态：番名在括号外，后面跟一整串技术参数。
        assert_eq!(
            nekomoe(
                "[喵萌奶茶屋&LoliHouse] 葬送的芙莉莲 - 05 [WebRip 1080p HEVC-10bit AAC][简繁内封字幕]"
            ),
            vec!["葬送的芙莉莲"]
        );
        // 旧形态：番名在第二个括号里。
        assert_eq!(
            nekomoe("[喵萌奶茶屋][葬送的芙莉莲][05][繁日双语][1080P]"),
            vec!["葬送的芙莉莲"]
        );
        // 全角组标签同样要剥。
        assert_eq!(
            nekomoe("【喵萌奶茶屋】 葬送的芙莉莲 - 05 [1080p]"),
            vec!["葬送的芙莉莲"]
        );
    }

    #[test]
    fn nekomoe_keeps_slash_names_and_season_suffix() {
        // `/` 分隔的中英名都要留（检索名优先排序在 `sort_names_by_priority` 里）。
        assert_eq!(
            nekomoe("[喵萌奶茶屋] 葬送的芙莉莲 / Sousou no Frieren - 05 [1080p]"),
            vec!["葬送的芙莉莲", "Sousou no Frieren"]
        );
        // 「第二季」这类后缀不能当成参数砍掉——它是区分多季的关键。
        assert_eq!(
            nekomoe("[喵萌奶茶屋] 葬送的芙莉莲 第二季 - 16 [简日双语][1080P]"),
            vec!["葬送的芙莉莲 第二季"]
        );
    }

    #[test]
    fn ming_y_sub_reads_slash_names_and_search_paren() {
        // TS 注释里的形态：组标签 + 中/英名用 `/` 分隔，日文检索名放全角括号。
        let names = ming_y_sub(
            "[MingYSub] 地缚少年花子君 / Toilet-bound Hanako-kun（地縛少年花子くん）[01][1080P][简日内嵌]",
        );
        assert_eq!(names.len(), 3, "{names:?}");
        assert_eq!(
            names[0], "地縛少年花子くん",
            "全角括号里的检索用名要排第一：{names:?}"
        );
        assert!(names.iter().any(|n| n == "地缚少年花子君"), "{names:?}");
        assert!(
            names.iter().any(|n| n == "Toilet-bound Hanako-kun"),
            "{names:?}"
        );
    }

    #[test]
    fn lolihouse_cuts_dash_episode_and_strips_tech_marks() {
        // 真实形态：集数是 ` - 04`，后面还跟着一整串技术参数，不能当番名。
        let names = lolihouse(
            "[LoliHouse] 才女的侍从 / Saijo no Osewa - 04 [WebRip 1080p HEVC-10bit AAC][简体内嵌]",
        );
        assert_eq!(names, vec!["才女的侍从", "Saijo no Osewa"], "{names:?}");
    }

    #[test]
    fn youha_aliases_route_to_yuha_rule() {
        // 用函数地址确认别名指向 `yuha` 本身，而不是某条碰巧输出相同的规则。
        let expected = rule_for("悠哈C9漢化組").expect("繁体别名必须命中");
        assert!(
            std::ptr::fn_addr_eq(expected, yuha as Rule),
            "悠哈C9漢化組 要归到悠哈璃羽的规则"
        );
        for alias in [
            "悠哈C9字幕社",
            "悠哈C9字幕组",
            "悠哈C9汉化组",
            "YUI-7&悠哈C9字幕组",
        ] {
            let rule = rule_for(alias).unwrap_or_else(|| panic!("{alias} 没归到任何规则"));
            assert!(
                std::ptr::fn_addr_eq(rule, yuha as Rule),
                "{alias} 没归到悠哈璃羽的规则"
            );
        }
        // 再走一遍真实标题，确认规则本身能解析（地址对但函数坏的情况也要挡住）。
        let names = expected("[葬送的芙莉莲][01][1080P]");
        assert_eq!(names.first().map(String::as_str), Some("葬送的芙莉莲"));
    }
}
