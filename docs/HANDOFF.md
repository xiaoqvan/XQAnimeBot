# 交接文档（HANDOFF）

> 面向下一个接手的人/AI。读完这份，你应该能在 10 分钟内跑起来，并知道下一步该写哪个文件。
> **最后更新：2026-10-27 08:00｜分支 `rust`｜HEAD `af32ac0`｜`cargo test --workspace` = 440 passed**
>
> **参考实现只有一个**：`github.com/xiaoqvan/XQAnimeBot` 的 **`newdbtest`** 分支，
> 本地检出 `d:/GitHub/_ts-newdbtest`。
> ⚠️ `F:/Node.JS/XQAnimeBot` 是 `main` 分支的旧副本（插件本体 10,617 行），
> **不能当参考**——早期文档照它写过，结论错了三处，见 [gap-audit](gap-audit-2026-10-27.md) §0。
>
> 三份文档的分工：
> - 本文：怎么跑、代码在哪、坑在哪；
> - [gap-audit-2026-10-27.md](gap-audit-2026-10-27.md)：与 `newdbtest` 的**剩余差异**（唯一权威）；
> - [plan-parity-newdbtest.md](plan-parity-newdbtest.md)：9 个阶段的计划、验收标准与执行记录。

---

## 0. 三分钟上手

```bash
cd D:/GitHub/XQAnimeBot-Rust

cargo build --workspace     # 8 个 crate，应 0 warning
cargo test --workspace      # 440 个单测全绿（离线，不碰网络/数据库/TDLib）

# 出图端到端（会拉起渲染子进程，需要 runtime/ultralight 已就位）
cargo run -p ultralight-gen-image --example render_card

# RSS 三源端到端（真网络，无需账号/数据库）
cargo run -p anime-core --example rss_probe

# 匹配端到端（默认内置 fixture；加 --live 换成真实 RSS）
cargo run -p anime-core --example matching_probe

# LLM Agent 端到端（需要 AI Key，会真调 Bangumi 工具）
cargo run -p anime-core --example agent_live

# 插件在真宿主里跑一遍（回放模式，不接 TDLib、不需要账号）
cargo build -p fuyubot-plugin-anime
mkdir -p target/abi-probe/plugins
cp target/debug/fuyubot_plugin_anime.dll target/abi-probe/plugins/
<FuyuBot>/target/debug/fuyubot.exe run \
  --replay examples/replay/commands.json \
  --plugin-dir target/abi-probe/plugins --replay-wait 500
```

跑通这几条，说明环境和运行时都没问题。

---

## 1. 这个项目是什么

把 TS 版动漫机器人（XQAnimeBot，跑在自研 tsbot 框架上，参考 `newdbtest` 分支）
迁移成 **FuyuBot 插件**（Rust，cdylib 热加载）。

业务链路：

```
RSS 三源（bangumi / dmhy / acgnx）→ 标题解析（字幕组规则）→ 去重
    → 老番：规则匹配（AI 补集数）/ 新番：LLM Agent 匹配
    → qBittorrent 下载 → ffmpeg 转码/烧字幕/分段 → 导航正文 → 发频道/缓冲频道
    → 审核提示与回调 → resources 落库 → 导航刷新
    → 后续：/jz 单集纠正、内联查询
```

迁移时定下的三条边界（**改动前请先确认，这是刻意的**）：

1. **基础设施拆成独立插件**：数据库、qB、转码、出图都与动漫无关，注册成服务供任何插件调用；
2. **出图走真浏览器**：TS 是 `Vue → satori → SVG → resvg → sharp`，Rust 侧没有 satori 等价物，
   改成 Ultralight 2.0（WebKit）跑真 HTML 再截图；模板是纯 HTML（数据 Rust 侧绑定）；
3. **业务逻辑放 `anime-core`，插件壳只做翻译**：领域层不碰宿主、不碰 TDLib，能被 `cargo test` 直接跑。

---

## 2. 工程结构

```
crates/
├── fuyubot-service/          355 行   服务外壳：JSON 服务表、回复缓冲、配置槽、跨插件调用
├── fuyubot-plugin-mongo/     862 行   storage.mongo        （11 方法）
├── fuyubot-plugin-qb/      1,079 行   download.qbittorrent （12 方法）
├── fuyubot-plugin-ffmpeg/  2,694 行   media.ffmpeg         （10 方法，含 thumbnail）
├── fuyubot-tdlib-ext/        583 行   TDLib 读接口（getMessage/getChat/getMessageLink/answerInlineQuery…）
├── ultralight-gen-image/   2,463 行   image.render         （2 方法）+ image-shot.exe
├── anime-core/            12,407 行   领域层：配置/模型/标题解析/RSS/匹配/Agent/卡片/存储 trait
└── fuyubot-plugin-anime/  12,483 行   业务插件：轮询/链路/审核/导航/纠正/inline/命令
```

累计约 33k 行 Rust。依赖关系：

```
fuyubot-plugin-anime ──┬─ storage.mongo         ──▶ MongoDB
                       ├─ download.qbittorrent  ──▶ qBittorrent WebUI (5.0+)
                       ├─ media.ffmpeg          ──▶ ffmpeg 子进程
                       └─ image.render          ──▶ image-shot.exe ──▶ Ultralight
                       └─ fuyubot-tdlib-ext     ──▶ HostApi td_invoke/td_execute
```

### 2.1 `fuyubot-plugin-anime` 模块表

| 模块 | 行数 | 职责 |
| --- | --- | --- |
| `bot.rs` | 1,475 | 22 条命令的静态表 + 路由 + `/help`（表即清单，不会和实现漂移） |
| `callback.rs` | 1,447 | 内联键盘回调：6 种 `Y/F/N_anime/N_ep/colorep/chgcol` 分发、多轮纠正 |
| `chain.rs` | 1,318 | 下载 → 预检 → 转码/烧字幕 → 分段 → 导航 → 发频道 → 落库（`Plan` 决定发哪、发完干什么） |
| `correct.rs` | 1,016 | `/jz` 单集纠正：按章节 id 迁移资源、重写集数、刷导航、改已发文案；`publish_cached` 供审核回调复用 |
| `inline.rs` | 860 | `@机器人 关键词`：打分制选择器、导航卡/单集视频两种应答 |
| `text.rs` | 735 | 导航正文（1024/4096 双限、分页链接、集数排序）、视频文案、标签过滤、集数解析 |
| `tasks.rs` | 584 | 并发管理器：任务表、普通/MKV 队列、重型槽、进度、取消 |
| `poll.rs` | 539 | RSS 轮询循环、跨轮去重、匹配决策、按结论分派 |
| `td.rs` | 480 | TDLib 出口：`Target`、发送/编辑/相册、Markdown 渲染 |
| `newanime.rs` | 400 | 新番两条路：置信度达标先发后审 / 否则缓冲频道 |
| `reply.rs` | 342 | 「等管理员下一条回复」状态机（30 分钟超时） |
| `review.rs` | 339 | 审核记录：`record_send` / `prompt_after_send` / 通过落库 |
| `web.rs` | 310 | 5 个只读 GET（`/api/health|stats|anime|reviews|tasks`） |
| `prompts.rs` | 303 | 4 种审核提示 + 候选列表 |
| `fetch.rs` | 301 | 轮询外围：抓取、过滤、seen 去重 |
| `nav.rs` | 290 | 导航卡首发、正文分页维护（`navVideoMessage`）、链接 |
| `runtime.rs` | 251 | 配置定位（`config_toml` → `config_path` → `discover`）+ `Arc<Runtime>` |
| `media.rs` | 207 | ffmpeg 服务封装：probe/plan/transcode/thumbnail/split |
| `deeplink.rs` | 187 | `/start` 深链解析（`bgm_cb-…`、收藏、标记看过） |
| `bgmuser.rs` | 183 | Bangumi 凭证：提前 60 秒刷新并写回 |
| `store.rs` | 140 | `Storage` trait 在 `storage.mongo` 上的实现 |
| `perm.rs` | 127 | 管理员判定：白名单或管理群 `getChatMember` |
| `qb.rs` | 110 | qB 服务封装 + btih 提取 |

---

## 3. 现在能跑什么

**业务主链已全程打通**（离线单测覆盖到每个决策点，真机联调见 §10）：

| 环节 | 实现 | 关键行为 |
| --- | --- | --- |
| RSS 三源 | `anime-core/src/rss/` | 并行抓取、优先级合并（bangumi > dmhy > acgnx）、`enrich_bangumi` 补磁力与多语言名 |
| 标题解析 | `anime-core/src/title/` | 34 条字幕组规则 + 8 条别名（**超集**于 TS 的 31/5） |
| 老番匹配 | `matching/` + `poll.rs` | 规则打分（760 阈值、歧义转复核、放送窗口）；规则没对上时 AI 补集数 |
| 新番匹配 | `ai/agent.rs` + `newanime.rs` | Responses API 工具调用（3 个工具、≤8 轮），输出 `confidence`；≥0.9 先发后审，否则缓冲频道 |
| 下载 | `chain.rs` + `qb.rs` | btih 直用 / `.torrent` 地址则加种后从列表反查；MKV 预检分队列 |
| 转码 | `chain.rs` + `media.rs` | 先烧字幕转一次，滤镜挂了去掉字幕再转；失败回退原文件，不让整条链失败 |
| 发送 | `chain.rs` + `td.rs` | 真实宽高时长、第 10 秒封面、>2GB 分段相册（仅首段带文案）、`has_spoiler=r18`、失败重试 |
| 导航 | `text.rs` + `nav.rs` | 正文双限测量 + summary 压缩 + `[第N页](link)`；分页消息 edit/新发 |
| 审核 | `prompts.rs` + `review.rs` + `reply.rs` + `callback.rs` | 4 提示 / 6 回调 / 多轮回复 / 通过后落 `resources` + 刷导航 |
| 纠正 | `correct.rs` | `/jz <源章节> <目标章节> [字幕组]`；`N_ep`/`F_anime` 走同一套（正片库 `run`，缓冲库 `publish_cached`） |
| 并发 | `tasks.rs` | 普通/MKV 队列、转码独占槽、`/progress`、`/canceltask`（等槽/预检/下载/转码都可中断） |
| 内联 | `inline.rs` | 章节 id / bgm 链接 → 视频卡；关键词 → 导航卡；选中发原文件不重传 |
| 权限 | `perm.rs` | `super_admin`/`owner_id` 或管理群管理员；`0` 一律当没设 |
| OAuth | `oauth.rs` + `bgmuser.rs` + `deeplink.rs` | 授权 → 回调 → 存 token → 自动刷新；`/bindbangumi` 也支持直接粘 token |

---

## 4. 阶段进度（[plan-parity-newdbtest.md](plan-parity-newdbtest.md)）

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| 1 | 配置迁入全局（三层配置、`timeout_ms`、`workers` 生效） | ✅ |
| 2 | 数据模型修正（`bangumi_users` / `pendingReviews` / 三张表 / 序列 / `navVideoMessage`） | ✅ |
| 3 | 新番链路 + 缓冲频道 + 审核（4 提示 / 6 回调 / 多轮） | ✅ |
| 4 | 导航正文 + 视频文案 + 媒体细节 + 状态机 + 标签过滤 | ✅ |
| 5 | 并发管理器 + MKV 队列 + 进度/取消 | ✅ |
| 6 | 命令 / 权限 / OAuth / `getEpisodeById` | ✅ |
| 7 | LLM Agent（Responses API 工具调用） | ✅ |
| 8 | inline 模式 | ✅ |
| 9 | 文档 + 分支交付 | ✅（本文即交付物） |

**剩余差异**只有 [gap-audit §2](gap-audit-2026-10-27.md) 那 10 条：5 条 P1（`ai_calls`、索引、
ErrorHandler、`tagExcludeList`、库级种子去重）+ 5 条 P2（导航图不重生成、状态只写 5 态、
进度文案简化、web 只读、收藏卡模板）。

---

## 5. 下一步建议（按性价比）

1. **建索引**（半天）：在 `lib.rs` 启动时对 `resources` 的 `{anime_id, episodeId}`、
   `{anime_id, groups, episode}`、`cache_resources` 唯一索引调 `ensure_index`。
   TS `initDb.ts` 里有现成的键定义，照抄即可。库越大收益越明显。
2. **`ai_calls` 落库**（半天）：Agent 每次调用写一条（模型、耗时、token、结果摘要），
   顺便做「同一条目 24 小时内不重复问」。
3. **ErrorHandler**（半天）：`error_group_thread_id` 接上，未捕获异常发管理群 ERROR 话题。
   现在出错只有宿主日志，值班的人看不到。
4. **库级种子去重**（1 小时）：`chain::spawn` 前调 `has_torrent_by_magnet`，补上重启后的窗口。
5. **导航图重生成**（1–2 天）：`navImageHash` 写库 + `editMessageMedia`。
   要先确认宿主 `td_execute` 支持 `editMessageMedia`（当前没用过）。

---

## 6. 架构决策：为什么这么做（**别改回去**）

| 决策 | 原因 | 如果硬要改会怎样 |
| --- | --- | --- |
| 出图必须走子进程 | Ultralight 的 Renderer 必须在**主线程**创建并驱动，宿主主线程归 tokio，cdylib 抢不到 | 某些平台直接崩，或渲染卡死拖垮机器人 |
| 出图一个 crate 两个产物 | 合并了原来的三个 crate，但进程分离不能取消 | — |
| `RenderRequest` 只有一份（`src/request.rs`） | 以前插件侧/渲染器侧各一份，字段靠手抄，错一个只在运行时炸 | — |
| 服务用「点号名」而不是插件名 | 服务是契约，插件是可替换实现 | 换实现就要改所有调用方 |
| 服务方法同步 + 调用方自己开线程 | `ServiceMethodFn` 只有同步签名（ABI 定死） | 一次渲染 0.2~1s，塞进 TDLib 回调线程会堵死事件流 |
| 业务逻辑放 `anime-core` | 可以纯 `cargo test`，不需要 Telegram/Mongo | 全部要起宿主才能测 |
| Mongo 用 relaxed Extended JSON | 与 TS 写入的 BSON 二进制兼容 | 老数据读回来类型错位 |
| qB 只适配 5.0+ | 用户明确要求，且本机就是 5.1.2 | 见 §7.3，4.x 语义不同 |
| 槽位/重型槽用 RAII guard | `run()` 错误分支多，手动 release 漏一条就永久少一个 worker | 跑几天后并发掉到 0，只能重启 |

---

## 7. 踩过的坑（照着抄，别再踩）

### 7.1 Ultralight：exception 出参是「空 ULString」不是空指针

`ulViewEvaluateScript` 的 exception 出参在没有异常时是**空 ULString**（非 null）。
只判 `is_null()` 会让页面永远「不就绪」，最后超时截白图。判断要用 `ulStringIsEmpty` / 长度。

### 7.2 Ultralight：求值结果不能 `ulDestroyString`

返回的字符串归 SDK 所有，销毁它会导致**双释放，进程直接消失**（无 panic、无日志）。
只释放自己 `ulCreateString` 出来的那些。

### 7.3 qBittorrent 5.0 改了四处，其中两处「不报错」

| 项 | 4.x | 5.0+ | 我们怎么写 |
| --- | --- | --- | --- |
| 暂停/继续 | `torrents/pause` `resume`（5.x 上 404） | `torrents/stop` `start` | 只打新端点 |
| 添加后不启动 | `add` 的 `paused` | `add` 的 `stopped` | 只发 `stopped` |
| 列表过滤 | `filter=paused` `resumed` | `filter=stopped` `running` | 旧名映射 + 白名单校验 |
| 状态串 | `pausedDL/UP` | `stoppedDL/UP` | 原样透传 |

**两个静默坑**（qB 都不报错）：`add` 传不认识的参数会被忽略（「不下载」变成真下载几十 GB）；
`filter` 传不认识的取值会默认成 `All`（返回全部种子）。所以 `add` 前校验版本，`filter` 本地白名单校验。

### 7.4 mongodb 3.x 的 bson 版本必须对齐

```toml
bson = { version = "3", features = ["chrono-0_4", "serde", "serde_json-1"] }
mongodb = { version = "3", features = ["bson-3"] }
```

`serde_json-1` 是 `into_relaxed_extjson()` 的开关；`mongodb` 的 `bson-3` 让两者用同一个 bson 实例，
否则编译期报「expected bson::Document, found bson::Document」。

### 7.5 `define_plugin!` 只收字面量

```rust
fuyubot_abi::define_plugin!(MyPlugin, "ultralightGenImage");  // ✅
fuyubot_abi::define_plugin!(MyPlugin, PLUGIN_NAME);            // ❌ 宏匹配 $name:literal 失败
```

同一个 crate 里**只能展开一次**（四个导出符号名是 ABI 定死的）。

### 7.6 没有宿主时怎么测插件

把业务逻辑放 `pub mod api` + `pub fn call(method, Value)`，`unsafe extern "C"` 只做薄壳，
然后写 `examples/live.rs` 直连真机。四个基础设施插件都是这么测的。

### 7.7 Windows 构建噪音

`error deleting lock file for incremental compilation session directory ... (os error 5)` 是并发 cargo /
杀毒软件占用导致的，**不是代码警告**。判断有没有真警告时先 `grep -v "lock file"`。

### 7.8 上游 crate 正在开发时，path 依赖会「突然」编译不过

`fuyubot-abi` / `fuyubot-simplify` 走 path 依赖，上游正在开发时可能处在半个状态。
判断：`cd <上游> && git status --short` 看 `??` / `M` 与 mtime。**别在上游改到一半时排查本仓。**

### 7.9 回放验证只能证明「请求组得对」

`--replay` 不接 TDLib，插件的 `sendMessage` 只是留档。它能验：插件加载、事件订阅、命令路由、
JSON 字段路径、服务发现；**不能**验：Telegram 真的收到、`parseTextEntities` 解析对不对、
`answerInlineQuery` 是否被服务端接受。这些要真账号联调。

### 7.10 回调里别同步抓网络

`on_command` / 回调都跑在宿主投递线程上。在里面 `rss::fetch_all` 会把线程按住十几秒，
期间所有更新都堵着。一律 `std::thread::spawn` 出去跑，结果再 `td.reply_text`。
同理别在回调里用 `send_message_wait_success`。

### 7.11 改 `bot.rs` / `td.rs` 要保留 CRLF

这两个文件在工作区是 **CRLF**。用脚本改写时如果写成 LF，`git diff` 会把整个文件标成改动。
Python 里用 `io.open(..., newline='')` 并显式保留 `\r\n`。

### 7.12 `callback_data` 上限 64 字节

中文一个字 3 字节。所以 `/jz` 不做确认按钮（字幕组名塞不下），审核回调只带数字 id。

---

## 8. 宿主 ABI 速查（`fuyubot-abi`）

路径依赖：`fuyubot-abi = { path = "F:/Rust/FuyuBot/crates/fuyubot-abi" }`（见根 `Cargo.toml`）。
SDK 发版后换成版本号。

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
| `emit` / `on(event, filter, cb)` / `off(h)` | 事件总线。**更新按 `@type` 单独发一遍**，所以 `on("updateNewInlineQuery")` 能收到内联查询 |
| `get_service(name)` | 跨插件调用；业务侧用 `fuyubot_service::call_service` 包一层 |
| `td_invoke(json)` / `td_execute(json)` | TDLib 请求 |
| `register_tool` / `call_tool` / `list_tools` | AI 工具调用 |
| `wait_for_update` / `cancel_wait` | 同步等某个 update |

TDLib 便捷封装在 **`F:/Rust/FuyuBot/crates/fuyubot-simplify`**（不在本仓库）：
`Text` / `ParseMode` / `Media` / `InputFile` / `ReplyButton` / `send_params` / `album_params` /
`edit_text_params` / `edit_caption_params` / `on_command` / `command_of` / `UpdateFilter`。
读接口与 inline 应答在 `crates/fuyubot-tdlib-ext`（本仓库），见 [tdlib-gaps.md](tdlib-gaps.md)。

---

## 9. 环境与运行时依赖

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
cargo run -p fuyubot-plugin-mongo --example live
QB_WRITE_TEST=1 cargo run -p fuyubot-plugin-qb --example live
cargo run -p fuyubot-plugin-ffmpeg --example plan_smoke
cargo run -p anime-core --example bangumi_payload
cargo run -p anime-core --example agent_live
```

---

## 10. 风险与未验证项

| 项 | 状态 |
| --- | --- |
| 插件在真宿主的加载/服务发现/事件订阅/请求组装 | ✅ 已验证（2026-10-25） |
| 业务主链「RSS → 匹配 → 下载 → 转码 → 发频道」 | ⚠️ **单测全绿但没做过一次完整真机联调**。集成问题只会在这里暴露 |
| `answerInlineQuery` | ⚠️ 离线只能看请求体；结果类型是否被服务端接受未验 |
| 相册发送 + 封面 | ⚠️ 需要真 TDLib 上传 |
| 高频 `editMessageText` 风控 | ⚠️ TS 会先 `getMessageLinkInfo` 比旧值，Rust 未做；真机可能被限流 |
| OAuth 四步 | ⚠️ 需要真 Bangumi 应用与回调地址 |
| Linux 部署 | ⚠️ ffmpeg 硬件编码器、Ultralight 运行时只在 Windows 验过 |
| `fuyubot-abi` / `fuyubot-simplify` 是 path 依赖 | ⚠️ 换机器会构建失败；上游开发中可能编译不过（§7.8） |
| web 管理端 | 只读 5 个 GET，按约定不迁写操作 |

---

## 11. 文档索引

| 文件 | 内容 |
| --- | --- |
| [README](../README.md) | 工程结构、迁移进度总表、命令清单 |
| [gap-audit-2026-10-27.md](gap-audit-2026-10-27.md) | 与 `newdbtest` 的**剩余差异**（唯一权威） |
| [plan-parity-newdbtest.md](plan-parity-newdbtest.md) | 9 个阶段的计划、验收标准、执行记录 |
| [tdlib-gaps.md](tdlib-gaps.md) | TDLib 读接口覆盖、inline 结果类型的坑 |
| [plugin-architecture.md](plugin-architecture.md) | 为什么拆插件、服务清单、同步服务怎么扛慢操作 |
| [image-render.md](image-render.md) | 出图架构、模板契约、Ultralight 两个坑 |
| [audit-2026-10-26.md](audit-2026-10-26.md) | **历史存档**（迁移早期的对齐记录，结论已被上文取代） |
| `crates/*/README.md` | 各插件的服务名、方法、配置、联调命令 |

---

## 12. 一句话总结

**9 个阶段全部落地：数据模型与 `newdbtest` 对齐，业务主链（RSS → 匹配 → 下载 → 转码 → 发频道 →
审核 → 导航）完整，命令/权限/OAuth/单集纠正/并发管理/内联都接上了，440 个单测全绿。
剩下的不是「没写完」，而是 10 条细节差异（[gap-audit §2](gap-audit-2026-10-27.md)）和一次真机联调。**

---

## 13. 附录：三个高频操作（照抄即可）

### 13.1 给已有插件加一个服务方法

```rust
// 1) api 模块里写业务函数（可单测、可被 example 直调）
pub mod api {
    pub fn stats(input: &Value) -> Result<Value, String> {
        let collection = require_str(input, "collection")?;
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

然后在 `api::call` 的 `match` 里加一行分支。**逻辑不要写进 `unsafe extern "C"` 函数**。

### 13.2 新增一张业务卡片模板

1. 在业务插件 `assets/templates/` 放 `xxx.html`；
2. 纯 HTML + 占位符（`{{ 字段 }}`、`{{#if}}`、`{{#each}}`），不用带 JS；
3. 数据侧写 `to_html_payload(&self) -> serde_json::Value`（参考 `anime_core::card::bangumi`）；
4. 调 `image.render` 的 `render`，`template_dir` 传业务自己的目录。

调试不用起机器人：`cargo run -p anime-core --example bangumi_payload` 导出 payload，
再用 `image-shot --once req.json` 直接出图。

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

慢调用（转码、出图）**必须**放自己的线程或 `spawn_blocking`：服务方法本身是同步的，
占着的是 TDLib 回调线程。
