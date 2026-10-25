# anime-core

动漫插件的**领域层**：纯逻辑，不认识宿主、不认识 TDLib、不碰动态库加载。
所有能脱离 Telegram 验证的东西都放这里，所以这一层可以 `cargo test` 直接跑。

```text
config     九组配置（telegram / qb / mongo / rss / bangumi / media / ai / http）
models     MongoDB 文档模型（字段名与 TS 版逐字对齐，老库直接接）
logger     最小日志 trait（插件壳转发到宿主 log）
title      标题解析：规则表、集数、片源、通用过滤
rss        RSS 三源抓取 → 过滤 → 合并去重 → 构建 AnimeItem
matching   番剧匹配（属于哪部番）+ 集数对齐（第几集、放送日期对不对）
card       出图模板的 payload（图片本身由 ultralightGenImage 插件渲染）
```

## rss：一条命令看到真数据

```bash
cargo run -p anime-core --example rss_probe                    # 抓三源 + 合并 + 标题解析
cargo run -p anime-core --example rss_probe -- --top 20        # 多看几条
cargo run -p anime-core --example rss_probe -- --build 0       # 只看抓取/合并
cargo run -p anime-core --example rss_probe -- --bangumi 2     # 再打 bangumi.moe API 补磁力
cargo run -p anime-core --example rss_probe -- --json          # 合并结果的 JSON
```

| 模块 | 职责 |
| --- | --- |
| `rss/http.rs` | `ureq` 客户端：固定浏览器 UA、15s 超时、GET 文本 / POST JSON |
| `rss/xml.rs` | RSS 2.0 → `Entry`（CDATA、实体、自闭合 `enclosure` 都要管） |
| `rss/common.rs` | 中文时间戳格式化、`Fin`/合集/字幕形态过滤 |
| `rss/bangumi.rs` | bangumi.moe 站内 RSS（无作者白名单，`.torrent` 地址） |
| `rss/dmhy.rs` | 动漫花园（**账号 → 字幕组映射表即白名单**，LoliHouse 的「内封」改写） |
| `rss/acgnx.rs` | 末日动漫（作者映射表即白名单） |
| `rss/bangumi_moe.rs` | bangumi.moe 的 `torrent/team/tag fetch`（磁力、发布组、多语言番名） |
| `rss/mod.rs` | 三源并行抓取、优先级合并（bangumi > dmhy > acgnx）、`build_item`、`enrich_bangumi` |

两条刻意的设计：

1. **抓取与合并分开**：`merge()` / `build_item()` 是纯函数，优先级、拉黑、合集过滤都能用
   固定 fixture 单测，不需要网络；`fetch_all()` 只负责「并行 + 把失败变成一句人话」。
2. **`build_item` 不做 AI**：集数只看正则。AI 兜底（`extractEpisodeByAI`）属于可选增强，
   在插件壳里做，领域层不依赖外部模型。

## matching：两层匹配，都能离线单测

```bash
cargo run -p anime-core --example matching_probe           # 内置 fixture（不联网）
cargo run -p anime-core --example matching_probe -- --live  # 抓真实 RSS 再匹配
```

```text
season   这条种子属于库里的哪部番    ← TS 的 database/query.ts::hasAnimeSend
episode  属于第几集、放送日期对不对  ← TS 的 utils/matcher.ts::matchBangumiEpisode
```

| 模块 | 职责 |
| --- | --- |
| `matching/normalize.rs` | 全角转半角、空白合并、小写；再去空白的 `compact` 比较 |
| `matching/season.rs` | 番名 × 库内番剧打分（1000 / 950 / 600+比例）、阈值、歧义判定、集数越界信号 |
| `matching/episode.rs` | `v2`/`END` 去尾、按 `sort` 找正片、放送日期窗口（-8 天 ~ +30 天） |
| `matching/mod.rs` | `match_item()`：串起两层，产出「可投 / 复核 / 新番」结论 |

打分规则（分数越高越可信）：

| 情况 | 分数 |
| --- | --- |
| 归一化后完全相等 | 1000 |
| 只差空白 | 950 |
| 包含关系 | `600 + 300 × 短/长`，前缀再 +40 |
| 默认阈值 | 760（约等于短名覆盖长名 40%） |
| 歧义判定 | 前两名都是不同番剧且分差 ≤ 40 → 复核 |

**两处刻意偏离 TS**（都写在 `season.rs` 模块文档里，也有对应单测）：

1. 包含关系要求较短一侧至少 3 个字符。TS 里 `死神` 会命中 `死神 千年血战篇`，
   把新一季投到上一季频道；两字名走精确匹配仍然可用。
2. 纯 ASCII 名的包含关系要求词边界，否则 `ani` 会命中 `anime`。

偏离只影响「自动投放」：分数不够落到 `NeedsReview`，交给人或 AI 复核，不会丢条目。

AI 不在这一层：`ai_episode_search` 需要 LLM 凭据，属于插件壳；本层只接受一个
`Option<AiEpisode>`，有值就优先采信——和 TS 的「AI 优先，自动匹配兜底」同序。

## 和其他 crate 的关系

```text
fuyubot-plugin-anime (cdylib)  ──▶  anime-core（本 crate）
                                     └─ 只返回数据结构，不调用任何服务
```

下载 / 转码 / 存储 / 出图**都不在这里**：那是四个基础设施插件的服务（`storage.mongo`、
`download.qbittorrent`、`media.ffmpeg`、`image.render`），由插件壳去调。

## 测试

```bash
cargo test -p anime-core        # 94 个单测，全部离线
```

单测全部用内联 fixture，不联网、不连库：

- RSS 层：CDATA、`&amp;` 实体、自闭合 `enclosure`、作者白名单、分类过滤、LoliHouse 改写、
  时间格式化（含 0 点 → `12:05AM`）、优先级去重、合集/黑名单过滤、字幕组切分。
- 匹配层：全角/大小写归一、空格差异、季名后缀、两字短名不误命中、ASCII 词边界、
  同名两部番转复核、同分选最近更新、集数越界信号、`05v2` / `SP01` / 未来放送日期、
  AI 结果优先、非正片跳过。
