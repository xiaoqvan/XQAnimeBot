# 交接文档（HANDOFF）

> 面向下一个接手的人/AI。读完这份，你应该能在 10 分钟内跑起来，并知道下一步该写哪个文件。
> 最后更新：2026-10-25 23:55（本地时间）
>
> **⚠️ 2026-10-26 现状覆盖**：本文写于迁移中途，第 3/4 节多处已过时。当前事实以
> [README](../README.md) 与 [audit-2026-10-26.md](audit-2026-10-26.md) 为准：
> 业务主链（RSS → 匹配 → 下载 → 转码/烧字幕/分段 → 发频道）、审核按钮、导航消息、
> 检查点恢复、AI 兜底、只读管理 API 都已实现，**281 个测试**；仓库已在 `rust` 分支
> （github.com/xiaoqvan/XQAnimeBot）。仍未验证的只有「真机联调」（需要 Mongo/qB/TDLib 账号）。

---

## 0. 三分钟上手

```bash
cd D:/github/XQAnimeBot-Rust

cargo build --workspace     # 全部 7 个 crate，应 0 warning
cargo test --workspace      # 281 个单测全绿

# 出图端到端（会拉起渲染子进程，需要 runtime/ultralight 已就位）
cargo run -p ultralight-gen-image --example render_card

# RSS 三源端到端（真网络，无需账号/数据库）
cargo run -p anime-core --example rss_probe

# 匹配端到端（默认内置 fixture；加 --live 换成真实 RSS）
cargo run -p anime-core --example matching_probe

# 插件在真宿主里跑一遍（回放模式，不接 TDLib、不需要账号）
cargo build -p fuyubot-plugin-anime
mkdir -p target/abi-probe/plugins
cp target/debug/fuyubot_plugin_anime.dll target/abi-probe/plugins/
<FuyuBot>/target/debug/fuyubot.exe run \
  --replay examples/replay/commands.json \
  --plugin-dir target/abi-probe/plugins --replay-wait 500
```

跑通这几条，说明环境和运行时都没问题。**当前状态见文首的「2026-10-26 现状覆盖」。**

项目路径：`D:/github/XQAnimeBot-Rust`（版本控制已在 `rust` 分支，远端 `github.com/xiaoqvan/XQAnimeBot`）。
上游 TS 参考实现：`F:/Node.JS/XQAnimeBot/plugins/XQAnimeBot`（105 文件 / 20,564 行）。

---

## 1. 这个项目是什么

把 TS 版动漫机器人（XQAnimeBot，跑在自研 tsbot 框架上）迁移成 **FuyuBot 插件**（Rust，cdylib 热加载）。

原业务链路：

```
RSS 三源（bangumi / dmhy / acgnx）→ 标题解析（字幕组规则）→ 番剧匹配（Bangumi API）
    → qBittorrent 下载 → ffmpeg 转码 → 生成番剧信息卡 → 发 Telegram 频道 → 审核/回调/命令
```

迁移时定下的三条边界（**改动前请先确认，这是刻意的**）：

1. **基础设施拆成独立插件**：数据库、qB、转码、出图都与动漫无关，注册成服务供任何插件调用；
2. **出图走真浏览器**：TS 是 `Vue → satori → SVG → resvg → sharp`，Rust 侧没有 satori 等价物，
   改成 Ultralight 2.0（WebKit）跑真 HTML 再截图；模板是纯 HTML（数据 Rust 侧绑定），
   不再往页面里塞 Vue 运行时；
3. **业务逻辑全部放 `anime-core`**：不碰宿主、不碰 TDLib，能被 `cargo test` 直接跑；
   插件壳只做「宿主事件 ↔ 领域函数」的翻译。

---

## 2. 工程结构

```
crates/
├── fuyubot-service/        355 行   服务外壳：JSON 服务表、回复缓冲、配置槽、跨插件调用
├── fuyubot-plugin-mongo/   693 行   storage.mongo        （11 方法）
├── fuyubot-plugin-qb/     1072 行   download.qbittorrent （12 方法）
├── fuyubot-plugin-ffmpeg/ 1993 行   media.ffmpeg         （9 方法）
├── ultralight-gen-image/  1795 行   image.render         （2 方法）+ image-shot.exe
├── anime-core/            6085 行   领域层：配置/模型/标题解析/RSS 三源/番剧匹配/卡片 payload
└── fuyubot-plugin-anime/   544 行   业务插件壳：Plugin 三件套 + 命令路由 + TDLib 出口（骨架，已跑通）
```

总计约 12.5k 行 Rust。依赖关系：

```
fuyubot-plugin-anime ──┬─ storage.mongo         ──▶ MongoDB
                       ├─ download.qbittorrent  ──▶ qBittorrent WebUI (5.0+)
                       ├─ media.ffmpeg          ──▶ ffmpeg 子进程
                       └─ image.render          ──▶ image-shot.exe ──▶ Ultralight
```

---

## 3. 已完成并且**真机验证过**的能力

每个都能独立跑，验证命令都在各 crate 的 README 里。

### 3.1 `storage.mongo`（11 方法）

通用 CRUD，**不认识番剧**。BSON 走 relaxed Extended JSON（`{"$oid":...}`），业务侧只认 JSON。

| 方法 | 入参 |
| --- | --- |
| `ping` | – |
| `find` | `collection, filter?, projection?, sort?, limit?, skip?`（上限 10 000） |
| `find_one` | `collection, filter?, projection?` |
| `insert` | `collection, docs`（对象或数组） |
| `update` | `collection, filter, update?/patch?, upsert?, many?` |
| `delete` | `collection, filter, many?`（空 filter 默认拒绝，需 `allow_empty:true`） |
| `count` | `collection, filter?` |
| `aggregate` | `collection, pipeline` |
| `distinct` | `collection, field, filter?` |
| `ensure_index` | `collection, keys, unique?, sparse?, name?, ttl_seconds?` |
| `collections` | `db?` |

配置：`{"uri":"mongodb://127.0.0.1:27017","db":"xqanime"}`
验证：`cargo run -p fuyubot-plugin-mongo --example live`（在独立测试库 `xqanime_it` 里跑全套，不碰正式库）
实测：ping / 唯一索引拦截 E11000 / insert / find / update / upsert / aggregate / distinct / delete 全通过。

### 3.2 `download.qbittorrent`（12 方法）

**手写 WebUI v2 API，不用任何封装包**；**只适配 qBittorrent 5.0+**（详见 §6.3）。
方法：`ping` `login` `version` `transfer` `list` `files` `properties` `add` `pause` `resume` `delete` `set_category`
配置：`{"url":"http://127.0.0.1:8080","username":"admin","password":"***","timeout_ms":15000}`
验证：`QB_WRITE_TEST=1 cargo run -p fuyubot-plugin-qb --example live`
实测（v5.1.2.10 / WebAPI 2.11.4）：写接口往返通过，`add paused=true` 后 `state=stoppedDL`（确认没偷偷下载），
`pause→/torrents/stop`、`resume→/torrents/start`。

### 3.3 `media.ffmpeg`（9 方法）

方法：`ping` `capabilities` `probe` `plan` `transcode` `job_status` `job_list` `job_cancel` `prune`
`intent` 四档：`remux`（只换容器）/ `compat`（默认，h264 优先硬件）/ `small`（hevc）/ `fast`（只用硬件）
进度事件：`ffmpeg.progress`（`percent/speed/fps/frame/eta_ms`，400ms 节流）、`ffmpeg.done`、`ffmpeg.failed`、`ffmpeg.canceled`
配置：`{"ffmpeg":"D:/ffmpeg/bin/ffmpeg.exe","probe_on_start":true}`

「高度优化」的三层：
1. 能 copy 就不转（h264+aac → mp4，2 秒素材 252ms）；
2. **硬件编码器要真编一帧才算可用**（本机实测 `h264_nvenc`/`hevc_nvenc`/`h264_amf`/`h264_mf` 可用，
   `*_qsv`/`*_vaapi` 名单里有但跑不通——只按名单选会一路失败重试）；
3. 软件兜底 `libx264`/`libx265`。

验证：`cargo run -p fuyubot-plugin-ffmpeg --example plan_smoke`（真实转码 + 真实进度）

### 3.4 `image.render` —— ultralightGenImage 插件

一个 crate、两个产物：`ultralight_gen_image.dll`（插件）+ `image-shot.exe`（渲染器，独立进程）。
方法：`render`（`template`/`template_dir`/`data`/`out` 或 `html`/`out`，加 `width/height/scale/format/quality/ready_expr`）、`ping`
配置：`{"binary":"D:/.../image-shot.exe","templates_dir":"D:/.../templates"}`

模板契约：纯 HTML + mustache 子集占位符（`{{ 字段 }}` / `{{#if}}` / `{{#each}}`），
数据渲染前在 Rust 侧绑进去（缺字段直接报错）；`html`+`data` 同样绑定，页面零 JS。

验证：`cargo run -p ultralight-gen-image --example render_card`
实测（debug）：demo 卡 2000×1100 / 173KB / 653ms；动漫卡 2000×1100 / 190KB / 758ms。

辅助工具：`cargo run -p anime-core --example bangumi_payload` 导出番剧卡数据 JSON（调模板不用起机器人）。

### 3.5 RSS 三源（`anime-core/src/rss/`）

TS 版五个文件的合并体，**真网络验证过**（2026-10-25 一轮：bangumi 18 / dmhy 15 / acgnx 6，
合并去重后 25 条，其中 17 条能被规则表解析出番名）。

| 模块 | 行数 | 说明 |
| --- | --- | --- |
| `rss/http.rs` | 116 | `ureq` 客户端：固定浏览器 UA、15s 超时、GET 文本 / POST JSON |
| `rss/xml.rs` | 248 | RSS 2.0 → `Entry`：CDATA、`&amp;` 实体（独立事件）、自闭合 `enclosure` |
| `rss/common.rs` | 277 | 中文时间戳（含 0 点 → `12:05AM`）、Fin/合集/字幕形态过滤 |
| `rss/bangumi.rs` | 111 | bangumi.moe 站内 RSS（无作者白名单） |
| `rss/dmhy.rs` | 211 | 动漫花园（38 条账号→字幕组映射 = 白名单，LoliHouse 内封改写） |
| `rss/acgnx.rs` | 134 | 末日动漫（19 条映射 = 白名单） |
| `rss/bangumi_moe.rs` | 178 | bangumi.moe `torrent/team/tag fetch`：磁力、发布组、多语言番名 |
| `rss/mod.rs` | 485 | 三源并行抓取、优先级合并、`build_item`、`enrich_bangumi` |

关键行为（都照抄 TS）：
- 三源并行，任一源失败只影响自己（`thread::scope` + `catch_unwind`，等价 `Promise.allSettled`）；
- 合并去掉重的优先级 bangumi > dmhy > acgnx，键是**归一化后的原标题**；
- dmhy / acgnx 的 `max_items` 语义是「先截断再过滤」，所以 50 条里通常只剩十几条，这是 TS 行为；
- bangumi 条目的磁力要去 API 取；API 挂了降级用 RSS 里的 `.torrent` 地址。

验证命令：`cargo run -p anime-core --example rss_probe`（`--top/--build/--bangumi/--json`）。

### 3.6 番剧匹配（`anime-core/src/matching/`）

两层，纯函数，不查库不联网。TS 里这两件事分别埋在 `database/query.ts::hasAnimeSend`
和 `utils/matcher.ts` 里，中间隔着数据库查询与 LLM 调用，**导致没法单测**；这里拆出来就能固定住。

| 模块 | 行数 | 说明 |
| --- | --- | --- |
| `matching/normalize.rs` | 86 | 全角转半角 / 空白合并 / 小写；再去空白的 `compact_for_match` |
| `matching/season.rs` | 563 | 番名 × 库内番剧打分、阈值、歧义判定、集数越界信号 |
| `matching/episode.rs` | 537 | `v2`/`END` 去尾、按 `sort` 找正片、放送日期窗口 |
| `matching/mod.rs` | 273 | `match_item()` 串起两层，产出「可投 / 复核 / 新番」 |

打分（越高越可信）：完全相同 1000 / 只差空白 950 / 包含关系 `600+300×短/长`（前缀 +40）。
默认阈值 760（≈短名覆盖长名 40%），前两名是不同番剧且分差 ≤ 40 直接转复核。

**两处刻意偏离 TS**（`season.rs` 模块文档有完整理由，也有对应单测）：

1. 包含关系要求较短一侧 ≥ 3 字符。TS 里 `死神` 会命中 `死神 千年血战篇`，
   把新一季投到上一季频道；两字名走精确匹配仍然可用。
2. 纯 ASCII 名的包含关系要求词边界，否则 `ani` 会命中 `anime`。

放送日期窗口照抄 TS：`起始放送日 - 8 天` ~ `最后一集 + 30 天`；拿不到起止日期时退化成
「不能超过未来 8 天」。时间统一用 `NaiveDate`（只比日期），TS 那套 `new Date()` 的
时区/凌晨边界会制造假阳性。

真数据验证（`matching_probe -- --live`）：真实 RSS 里 `[桜都字幕组] 死神 千年血战篇-祸进谭-`
精确命中库内条目；`[ANi] 死神 - 05` 按预期落到「新番」（不做跨季误配）。

### 3.7 `anime-core`（领域层，94 单测）

| 模块 | 行数 | 状态 |
| --- | --- | --- |
| `title/rules.rs` 字幕组规则 | 611 | ✅ 37 条规则 + 别名表（TS 版约 35 组 + 别名） |
| `title/mod.rs` 解析入口 | 252 | ✅ `parse_info` / `clean_title` / `normalize_title` / `is_title_allowed` / `extract_source` / `brackets` |
| `title/episode.rs` 集数 | 233 | ✅ `01` / `02v2` / `01-12` 区间 / SP / 剧场版 / 未知 |
| `models.rs` 数据模型 | 444 | ✅ Anime / TorrentDoc / EpisodeMeta / RssItem / AnimeItem / TaskCheckpoint / CacheItem / NavMessage，字段对齐 TS |
| `config.rs` 配置 | 293 | ✅ Telegram / Qb / Mongo / Rss / Bangumi / Media / Ai / Http 九组 |
| `card/bangumi.rs` 卡片 payload | 332 | ✅ `to_html_payload()` + `cache_key()` |
| `card/image.rs` 素材处理 | 78 | ✅ 海报裁切 / data URI |
| `rss/` 三源 + 合并 | 1760 | ✅ 见 §3.5 |
| `matching/` 番剧 + 集数匹配 | 1459 | ✅ 见 §3.6 |

### 3.8 插件壳（`fuyubot-plugin-anime`）—— **第一次在真宿主里跑通**

之前这条一直是「最大风险」：12k 行代码全建立在「`HostApiVTable` 调用方式是对的」这个假设上。
2026-10-25 深夜补上了验证，结论是**假设成立**。

| 模块 | 行数 | 职责 |
| --- | --- | --- |
| `lib.rs` | 94 | `impl Plugin`：`init` 记日志、`start` 装配置+订阅、`stop` 按序清理 |
| `runtime.rs` | 193 | 配置定位（`config_toml` → `config_path` → `discover`）+ `Arc<Runtime>` + 运行时长 |
| `bot.rs` | 177 | 命令路由：一张静态表 = 命令清单 + `/help` 内容（不会和实现漂移） |
| `td.rs` | 80 | `Target`（发给谁/回哪条）→ `Simplify::send_message` |

**验证方法**（不需要账号，离线）：`fuyubot run --replay` + 自建样本
`examples/replay/commands.json`（`/ping` `/status` `/help` / 外来命令 / 普通聊天）。

验证到的四件事：

1. **插件能被加载**：`已加载插件 xq.anime`（此前是 `缺少符号 plugin_init`，因为只有 11 行占位）；
2. **事件订阅生效**：`UpdateOutcome { listeners: 1 }`，之前是 `0`；
3. **TDLib 请求组得对**：`sendMessage` 带 `inputMessageText` + `formattedText` +
   `reply_to: inputMessageReplyToMessage`，字段路径与真 `td_api.tl` 一致；
4. **跨插件服务发现可用**：`/status` 里四个服务全部 `✓`
   （`storage.mongo` / `download.qbittorrent` / `media.ffmpeg` / `image.render`）——
   这条最有价值，说明「业务插件按名字取服务」的架构假设也是对的。

顺带确认的两条行为：**外来命令与普通聊天必须沉默**（不能抢 `/start`、`/plugins`），
`stop` 被正常调用（日志里有 `[anime] stop`）。

---

## 4. 未完成清单（按建议顺序）
带 TS 参考路径，可以直接对照着抄逻辑。

### 4.1 RSS 三源抓取 → `anime-core/src/rss/` ✅ **已完成**（见 §3.5）

下面这张表留着当「TS 参考在哪里」的索引：

| TS 源 | 行数 | 说明 |
| --- | --- | --- |
| `anime/rss/index.ts` | 285 | 调度：轮询、去重、分页 |
| `anime/rss/bangumi.ts` | 70 | Bangumi 站内 RSS |
| `anime/rss/dmhy.ts` | 139 | 动漫花园 |
| `anime/rss/acgnx.ts` | 104 | 末日动漫资源库 |
| `anime/rss/common.ts` | 57 | 通用 XML→条目映射 |
| `anime/rssItemHandler.ts` | 258 | 单条处理：解析标题→白名单→入库 |

Rust 已有 `quick-xml` 依赖、`RssItem`/`RssKind` 模型和 `Error::Http/Parse` 变体，直接复用。

### 4.2 番剧匹配 → `anime-core/src/matching/` ✅ **已完成**（见 §3.6）

TS 参考：`database/query.ts::hasAnimeSend`（番名归一化 + 包含判断）
+ `utils/matcher.ts::matchBangumiEpisode`（AI 优先 + `sort` 对齐 + 放送日期窗口）。
没做的只有「发布组偏好」——TS 侧也没有这层逻辑，字幕组白名单在 RSS 抓取时就做完了。

### 4.3 业务存储层 → `anime-core/src/db/` ⬅️ **下一个该做的**

TS 参考：`database/query.ts`（804 行）+ `database/create.ts`
Rust 侧只需要把「业务查询」翻成 `storage.mongo` 的 JSON 调用（**不要**再引一次 mongodb 驱动）。

**做完匹配之后，这里的顺序很关键**：`matching` 只吃 `&[AnimeCandidate]` / `&[EpisodeMeta]`，
谁把库里的文档取出来、取多少条、怎么投影，是 `db` 层的事。先把这层定下来，
插件壳才不会在命令/回调里手搓查询。

具体要写的：

| 函数 | TS 对应 | 说明 |
| --- | --- | --- |
| `anime_candidates()` | `hasAnimeSend` 的全表扫描 | 投影成 `matching::AnimeCandidate`（只取 id/name/name_cn/names/episode/airing_start/updated_at） |
| `episodes_by_subject(id)` | `getEpisodeMetasBySubjectId` | 按 `sort` 排序，喂给 `match_episode` |
| `has_torrent_title` / `has_torrent_by_magnet` | 同名 | 去重键是 `clean_title` + `infoHash` |
| `add_torrent` | `database/create.ts` | 解析成功立刻写，堵住重复入队窗口 |
| `get_anime_by_id` / `search_anime` | 同名 | 命令与回顾要用 |
| `cache_item` 增删查 | `cacheItem` 集合 | 新番待审核 |

### 4.4 转码/下载编排 → `anime-core/src/media/`、`qb/`

TS 参考：`utils/mkvtomp4.ts`、`utils/cuttingvideo.ts`、`utils/checkTorrentFormat.ts`、`qBittorrent/client.ts`
要做：下载完成后挑文件、判断是否已兼容、调 `media.ffmpeg` 的 `plan`/`transcode`、
进度事件转发到 Telegram。

### 4.5 插件壳 → `fuyubot-plugin-anime/src/` 🚧 **骨架完成**（见 §3.8）

| 模块 | 状态 | 说明 |
| --- | --- | --- |
| `lib.rs` | ✅ | `init` / `start` / `stop` 三件套，回放验证过 |
| `runtime.rs` | ✅ | 配置定位三层 + `Arc<Runtime>` |
| `bot.rs` | ✅ | 命令路由表 + 3 个命令（`/ping` `/status` `/help`） |
| `td.rs` | ✅ | 纯文本出口 |
| 定时轮询任务 | ❌ | RSS 拉取循环 + 去重窗口 |
| 下载/转码编排 | ❌ | 见 §4.4 |
| 回调路由 | ❌ | 内联键盘、审核按钮（`CallbackQuery/` 1122 行） |
| 命令全集 | ❌ | 还有 11 个命令没写（TS `cmd/*.ts` 2193 行） |

**下一步最有价值的一条**：`/rss` 手动抓一次——把已经写好的 `rss::fetch_all` +
`matching::match_item` 接进壳里（开线程跑，别堵回调），就能第一次看到
「真实种子 → 匹配结论」在宿主进程里流动。这一步之后再做 `db` 才有意义。

**注意**：宿主只暴露 `HostApi`（见 §7），TDLib 请求走 `fuyubot-simplify`，别自己手拼 JSON。

### 4.6 后续

- Bangumi API 客户端 + AI Agent（TS `bangumi/` 1769 行）
- 用户收藏卡模板（TS `img/generateUserCollectionImage.ts` 423 行）
- Web 管理端（TS `web/` 2115 行）——**迁移与否还没决策**

---

## 5. 架构决策：为什么这么做（**别改回去**）

| 决策 | 原因 | 如果硬要改会怎样 |
| --- | --- | --- |
| 出图必须走子进程 | Ultralight 的 Renderer 必须在**主线程**创建并驱动，宿主主线程归 tokio，cdylib 抢不到 | 某些平台直接崩，或渲染卡死拖垮机器人 |
| 出图一个 crate 两个产物 | 合并了原来的三个 crate，但进程分离不能取消 | — |
| `RenderRequest` 只有一份（`src/request.rs`） | 以前插件侧/渲染器侧各一份，字段靠手抄，错一个只在运行时炸 | — |
| 服务用「点号名」而不是插件名 | 服务是契约，插件是可替换实现。`image.render` 不叫 `ultralightGenImage.render` | 换实现就要改所有调用方 |
| 服务方法同步 + 调用方自己开线程 | `ServiceMethodFn` 只有同步签名（ABI 定死） | 一次渲染 0.2~1s，塞进 TDLib 回调线程会堵死事件流 |
| 出图插件**零业务字段** | 模板目录由调用方传，渲染器只认 `template_dir/<name>.html` | 出图插件会变成第二个「大泥球」 |
| 业务逻辑放 `anime-core` | 可以纯 `cargo test`，不需要 Telegram/Mongo | 全部要起宿主才能测 |
| Mongo 用 relaxed Extended JSON | 与 TS 版写入的 BSON 二进制兼容 | 老数据读回来类型错位 |
| qB 只适配 5.0+ | 用户明确要求，且本机就是 5.1.2 | 见 §6.3，4.x 语义不同 |

---

## 6. 踩过的坑（照着抄，别再踩）

### 6.1 Ultralight：exception 出参是「空 ULString」不是空指针

`ulViewEvaluateScript` 的 exception 出参在没有异常时是**空 ULString**（非 null）。
只判 `is_null()` 会让页面永远「不就绪」，最后超时截白图。判断要用 `ulStringIsEmpty` / 长度。

### 6.2 Ultralight：求值结果不能 `ulDestroyString`

返回的字符串归 SDK 所有，销毁它会导致**双释放，进程直接消失**（无 panic、无日志）。
只释放自己 `ulCreateString` 出来的那些。

### 6.3 qBittorrent 5.0 改了四处，其中两处「不报错」

| 项 | 4.x | 5.0+ | 我们怎么写 |
| --- | --- | --- | --- |
| 暂停/继续 | `torrents/pause` `resume`（5.x 上 404） | `torrents/stop` `start` | 只打新端点 |
| 添加后不启动 | `add` 的 `paused` | `add` 的 `stopped` | 只发 `stopped` |
| 列表过滤 | `filter=paused` `resumed` | `filter=stopped` `running` | 旧名映射 + 白名单校验 |
| 状态串 | `pausedDL/UP` | `stoppedDL/UP` | 原样透传 |

**两个静默坑**（qB 都不报错）：
- `add` 传不认识的参数会被忽略 → 「添加但不下载」变成真下载几十 GB；
- `filter` 传不认识的取值 → `TorrentFilter::setTypeByName` 默认成 `All`，**返回全部种子**。

所以：`add` 前校验版本（<5.0 直接报错），`filter` 在本地按白名单校验（14 个合法值）。

### 6.4 mongodb 3.x 的 bson 版本必须对齐

```toml
bson = { version = "3", features = ["chrono-0_4", "serde", "serde_json-1"] }
mongodb = { version = "3", features = ["bson-3"] }
```

`serde_json-1` 是 `into_relaxed_extjson()` 的开关；`mongodb` 的 `bson-3` 特性是让两者用同一个 bson 实例，
否则 `Document` 类型对不上，编译期报一堆「expected bson::Document, found bson::Document」。

### 6.5 `define_plugin!` 只收字面量

```rust
fuyubot_abi::define_plugin!(MyPlugin, "ultralightGenImage");  // ✅
fuyubot_abi::define_plugin!(MyPlugin, PLUGIN_NAME);            // ❌ macro 匹配 $name:literal 失败
```

同一个 crate 里**只能展开一次**（四个导出符号名是 ABI 定死的）。

### 6.6 没有宿主时怎么测插件

把业务逻辑放 `pub mod api` + `pub fn call(method, Value)`，`unsafe extern "C"` 只做薄壳。
然后写 `examples/live.rs` 直连真机。四个基础设施插件都是这么测的，**不要**把逻辑写进 `extern "C"` 函数里。

### 6.7 Windows 构建噪音

`error deleting lock file for incremental compilation session directory ... (os error 5)` 是并发 cargo /
杀毒软件占用导致的，**不是代码警告**。判断有没有真警告时先 `grep -v "lock file"`。

### 6.8 上游 crate 正在开发时，path 依赖会「突然」编译不过

`fuyubot-abi` / `fuyubot-simplify` 走 path 依赖指向上游检出目录，而那边**正在开发**
（`crates/fuyubot-simplify/src/rich.rs` 目前是未提交的新文件）。遇到过一次
`cannot find type RichButtonAlign in this scope`——不是本仓的错，是那一刻上游文件正被保存。

判断方法：`cd <上游> && git status --short` 看 `??` / `M`，以及文件 mtime 是不是刚刚。
**别在上游改到一半时排查本仓**，等它稳定后重编一次即可。长期解法是等 SDK 发版后换成版本号。

### 6.9 回放验证只能证明「请求组得对」

`--replay` 不接 TDLib，插件的 `sendMessage` 只是留档。所以它能验证：插件加载、事件订阅、
命令路由、JSON 字段路径、服务发现；**不能**验证 Telegram 那边真的收到、`parseTextEntities`
真的把 Markdown 解析对了。那两类要真账号联调。

### 6.10 回调里别同步抓网络

`on_command` 的回调跑在宿主投递线程上。在里面 `rss::fetch_all`（三源并行 HTTP）会把这个线程
按住十几秒，期间所有更新都堵着。正确做法：`std::thread::spawn` 出去跑，结果再 `td.reply_text`。
同理别用 `send_message_wait_success`——那是等最终消息，一样阻塞。

---

## 7. 宿主 ABI 速查（`fuyubot-abi`）

路径依赖：`fuyubot-abi = { path = "F:/Rust/FuyuBot/crates/fuyubot-abi" }`（见根 `Cargo.toml`）。
**上游检出目录变了要改这里**；SDK 发版后换成版本号。

```rust
pub trait Plugin: Sized + Send + 'static {
    fn init(host: HostApi) -> Result<Self, String>;   // 拿 HostApi，注册服务
    fn start(&mut self, config_json: &str) -> Result<(), String>;
    fn stop(&mut self) {}                              // 默认空
}
```

`HostApi` 关键方法：

| 方法 | 用途 |
| --- | --- |
| `log(level, msg)` | 日志 |
| `emit(event_type, json)` / `on(event, filter, cb)` / `off(h)` | 事件总线（ffmpeg 进度就是靠它） |
| `get_service(name) -> *const ServiceVTable` | 跨插件调用；**业务侧用 `fuyubot_service::call_service` 包一层** |
| `td_invoke(json)` / `td_execute(json)` | TDLib 请求 |
| `register_tool` / `call_tool` / `list_tools` | AI 工具调用 |
| `wait_for_update` / `cancel_wait` | 同步等某个 update |

TDLib 便捷封装在 **`F:/Rust/FuyuBot/crates/fuyubot-simplify`**（不在本仓库）：
`Text` / `ParseMode` / `Media` / `InputFile` / `ReplyButton` / `send_params` / `album_params` /
`edit_text_params` / `edit_caption_params` / `on_command` / `command_of` / `UpdateFilter`。

---

## 8. 环境与运行时依赖

| 依赖 | 说明 |
| --- | --- |
| Rust 1.99+ / edition 2024 | `rust-version = "1.99"` |
| Ultralight 2.0 SDK | `powershell -File scripts/fetch-ultralight.ps1` 下载到 `runtime/ultralight/`（约 56MB，已 gitignore） |
| ffmpeg / ffprobe | 放 `FFMPEG_DIR`、PATH 或插件同级 |
| MongoDB | 本机 27017 已验证 |
| qBittorrent 5.0+ | 本机 8080 / v5.1.2.10 已验证 |

常用命令：

```bash
cargo build --workspace
cargo test --workspace
cargo run -p ultralight-gen-image --example render_card
cargo run -p ultralight-gen-image --bin image-shot -- --check
cargo run -p fuyubot-plugin-mongo --example live
QB_WRITE_TEST=1 cargo run -p fuyubot-plugin-qb --example live
cargo run -p fuyubot-plugin-ffmpeg --example plan_smoke
cargo run -p anime-core --example bangumi_payload
```

---

## 9. 建议的接手顺序（含验收标准）

**第一步：把领域层接进壳里**（宿主通路已通，见 §3.8，不用再验）

现在最该做的是让已经写好的东西**在同一进程里流动起来**：

1. 加 `/rss` 命令：`rss::fetch_all` → `build_item` → `matching::match_item`，用
   `std::thread::spawn` 跑（回调里同步抓网络会堵住宿主投递线程），结果用 `td.reply_text` 汇总回来；
2. 验收：在群里发 `/rss`，10 秒内回一条「抓到 N 条，其中 M 条匹配到番剧，前 3 条是 …」。

这一步把 `rss` / `matching` / `config` / `td` 四条线第一次串起来，集成层面的坑都会在这暴露。

**第二步：打通最小业务链路**（RSS → 解析 → qb → ffmpeg → 发频道）

1. ~~RSS 抓取~~、~~标题解析~~、~~番剧匹配~~ 都已完成，直接用 `rss::fetch_all` + `build_item` + `matching::match_item`；
2. 缺的是 `db` 那一层：把 `anime` 文档投影成 `AnimeCandidate`、把章节读成 `Vec<EpisodeMeta>`；
3. 手写一个 `/addanime` 的极简版，把匹配结果落进 `storage.mongo`；
4. 下载：`download.qbittorrent.add` → 轮询 `list` 到 `progress==1.0`；
5. 转码：`media.ffmpeg.plan`（`intent=compat`）→ `transcode` → 订阅 `ffmpeg.progress`；
6. 发送：`image.render` 出卡 + TDLib `sendPhoto`，文案用 `fuyubot-simplify` 的 `Text`。

验收：从一条真实 RSS 条目到频道里出现带卡片的视频，全程不手动干预。

**第三步**：命令全集 → 回调/审核 → 检查点恢复 → Bangumi/AI（匹配已经写完，不用再排）。

---

## 10. 风险与未验证项（接手时心里要有数）

| 项 | 状态 |
| --- | --- |
| ~~所有插件都没在真宿主里加载过~~ | ✅ **已排除**（2026-10-25，见 §3.8）：加载、服务发现、事件订阅、TDLib 请求组装全部验证通过 |
| 业务链路一次都没串起来 | ⚠️ **当前最高风险**。零件逐个都验证过，但从没在同一进程里跑完「RSS → 匹配 → 下载 → 转码 → 发频道」。集成 bug 只会在这里暴露 |
| 回放不接 TDLib | 回放只能验「请求组得对不对」，验不了「Telegram 那边收到什么」；真账号联调仍是空白 |
| 没有 git 仓库 | ⚠️ 建议立即 `git init` + 首次提交 |
| `fuyubot-abi` / `fuyubot-simplify` 是 path 依赖 | 换机器会构建失败；**且上游正在开发，`rich.rs` 处于未提交状态，改到一半时本仓会编译不过**（已遇到一次） |
| mongo / qb / ffmpeg 只在 Windows 本机验证 | Linux 部署（尤其 ffmpeg 硬件编码器、Ultralight 运行时）未验证 |
| 出图只有番剧卡模板 | 用户收藏卡未做 |
| Web 管理端 | 未决策是否迁移 |
| `anime-core` 的 `Error` 已定义 Http/Qb/Store 变体 | 但对应模块还没写，属于提前备好的骨架 |

---

## 11. 文档索引

| 文件 | 内容 |
| --- | --- |
| `README.md` | 工程结构、迁移进度总表 |
| `docs/plugin-architecture.md` | 为什么拆插件、服务清单、同步服务怎么扛慢操作 |
| `docs/image-render.md` | 出图架构、模板契约、Ultralight 两个坑 |
| `crates/*/README.md` | 各插件的服务名、方法、配置、联调命令 |
| `docs/HANDOFF.md` | 本文件 |

---

## 12. 一句话总结

**基础设施（存储/下载/转码/出图）已经能用了，而且都有真机验证；领域层的 RSS 三源与
番剧匹配也都打通了（真网络 + 真数据验证）；插件壳第一次在真宿主里跑通，ABI 与服务发现
的假设全部成立。剩下的主干是：把领域层接进壳里 → 业务存储 → 下载/转码编排 → 命令全集。**

**下一件事建议**：加一条 `/rss` 命令，让 `rss::fetch_all` → `matching::match_item` 在宿主进程里
真正跑一次（开线程，别堵回调）。这是第一次把四条线串起来，集成层面的坑都在这里。
之后再做 `db`——**顺序不能反**：`db` 的投影字段该有哪些，取决于壳里怎么用。

**当前最高风险不再是 ABI，而是「零件齐但没组装过」**：从没有一条真实种子在同进程里
跑完 RSS → 匹配 → 下载 → 转码 → 发频道。

---

## 13. 附录：三个高频操作（照抄即可）

### 13.1 给已有插件加一个服务方法

```rust
// 1) api 模块里写业务函数（可单测、可被 example 直调）
pub mod api {
    pub fn stats(input: &Value) -> Result<Value, String> {
        let collection = require_str(input, "collection")?;
        // ...
        Ok(json!({ "count": 42 }))
    }
}

// 2) 加一个薄壳
unsafe extern "C" fn svc_stats(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("stats", input)
}

// 3) 注册进方法表
Service::new(SERVICE, vec![
    // ...已有方法
    ("stats", svc_stats as Handler),
])
```

然后在 `api::call` 的 `match` 里加一行分支。**逻辑不要写进 `unsafe extern "C"` 函数**，否则只能起宿主才能测。

### 13.2 新增一张业务卡片模板

1. 在业务插件的 `assets/templates/` 放 `xxx.html`；
2. 模板就是纯 HTML，字段写占位符（`{{ 字段 }}`、`{{#if}}`、`{{#each}}`），不用带任何 JS；
3. 数据侧写 `to_html_payload(&self) -> serde_json::Value`（参考 `anime_core::card::bangumi`）；
4. 调：

```rust
let result = call_service(&self.host, "image.render", "render", &json!({
    "template": "xxx",
    "template_dir": "plugins/anime/templates",   // 业务自己的目录
    "data": payload,
    "out": out_path,
    "width": 1000, "height": 550, "scale": 2.0
}))?;
```

调试时不用起机器人：`cargo run -p anime-core --example bangumi_payload` 导出 payload，
用 `image-shot --once req.json` 直接出图。

### 13.3 在业务插件里调别人的服务

```rust
use fuyubot_service::{call_service, service_available};

if !service_available(&self.host, "media.ffmpeg", "transcode") {
    // 插件没装：走降级（比如直接发未转码文件）
}
let plan = call_service(&self.host, "media.ffmpeg", "plan", &json!({
    "input": path, "output": target, "intent": "compat"
}))?;
```

慢调用（转码、出图）**必须**放在 `tokio::task::spawn_blocking` 或自己的线程里，
因为服务方法本身是同步的，占着的是 TDLib 回调线程。

