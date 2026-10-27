# XQAnimeBot-Rust

原 XQAnimeBot（TypeScript，105 文件 / 20.5k 行）的 Rust 版：
FuyuBot 插件形态，RSS → 标题解析 → 匹配 → qBittorrent → 转码 → 发频道。

> **参考实现 = [`xiaoqvan/XQAnimeBot` 的 `newdbtest` 分支](https://github.com/xiaoqvan/XQAnimeBot/tree/newdbtest)**。
> 本文所有「TS 版」均指该分支（本地副本 `d:/GitHub/_ts-newdbtest`）；
> `main` 分支的 `plugins/XQAnimeBot` 是旧副本（10.6k 行），不要拿它对照。

## 工程结构

基础设施全部**拆成独立插件**，业务插件按名字取服务；每个插件都能单独装、单独升级、单独替换。

| crate | 类型 | 职责 |
| --- | --- | --- |
| `fuyubot-plugin-mongo` | cdylib | MongoDB：注册 `storage.mongo`（find/insert/update/aggregate/index…） |
| `fuyubot-plugin-qb` | cdylib | qBittorrent：注册 `download.qbittorrent`（手写 WebUI v2 API） |
| `fuyubot-plugin-ffmpeg` | cdylib | 转码：注册 `media.ffmpeg`（能力探测 + 自动选路 + 实时进度） |
| `ultralight-gen-image` | cdylib + bin | **ultralightGenImage**：出图插件，注册 `image.render`；同包产出 `image-shot.exe`（Ultralight 2.0 渲染器，独立进程独占主线程） |
| `fuyubot-service` | lib | 服务外壳：JSON 服务表、统一回复缓冲、配置槽 |
| `anime-core` | lib | 领域逻辑：配置、数据模型、标题解析、**RSS 三源**、**番剧/集数匹配**、番剧卡片 payload |
| `fuyubot-tdlib-ext` | lib | TDLib 读接口（getMessage/getChat/getMessageLink/answerInlineQuery…），simplify 的补丁层 |
| `fuyubot-plugin-anime` | cdylib | 动漫业务插件：轮询、下载链路、审核、导航、单集纠正、内联、22 条命令 |

```
fuyubot-plugin-anime  ──┬─ storage.mongo         ──▶ MongoDB
                        ├─ download.qbittorrent  ──▶ qBittorrent WebUI
                        ├─ media.ffmpeg          ──▶ ffmpeg 子进程
                        └─ image.render          ──▶ image-shot ──▶ Ultralight
```

## 迁移进度

**9 个迁移阶段全部落地。** 基础设施四个插件可独立运行；业务插件的主链（RSS → 匹配 → 下载 →
转码 → 发频道/缓冲频道 → 审核 → 导航）已完整，命令、权限、OAuth、单集纠正、并发管理、内联都已接上。
与 `newdbtest` 的**剩余差异**见 [gap-audit](docs/gap-audit-2026-10-27.md)。

| 模块 | TS 规模 | Rust 状态 |
| --- | --- | --- |
| 出图（bangumi.vue/satori → 纯 HTML + Ultralight） | ~1.1k | ✅ 独立插件，实测出图 |
| ffmpeg 转码 | ~0.5k | ✅ 独立插件，实测 copy/NVENC 两条路；MKV 简体中文字幕烧录已实测 |
| qBittorrent 客户端 | ~1.1k | ✅ 独立插件（手写 5.0 API，真机联调通过） |
| MongoDB 存储层 | ~2.5k | ✅ 独立插件（通用 CRUD + `ensure_index`，本机全链路联调通过） |
| 标题解析 / 字幕组规则 | ~1.3k | ✅ 34 条规则 + 8 条别名（TS 的 31/5 的超集） |
| 配置 / 数据模型 / 日志 | ~0.8k | ✅ 九组配置、三层取值、`bangumi_users`/`pendingReviews` 与 TS 逐字对齐 |
| 番剧卡片数据 payload | ~0.3k | ✅ 完成 |
| RSS 三源解析（bangumi/dmhy/acgnx） | ~1.2k | ✅ 完成（三源 + 合并去重 + 条目构建，真网络验证） |
| 番剧匹配（season + 集数对齐） | ~0.6k | ✅ 完成（打分 + 阈值 + 放送日期窗口，真数据验证） |
| LLM Agent（Responses API 工具调用） | ~1.8k | ✅ 3 个工具、≤8 轮、`confidence`/`reason`；新番链路 Agent 优先 |
| 业务编排（RSS→匹配→下载→转码→发频道） | ~2k | ✅ 主链完整：缓冲频道、4 提示 / 6 回调、多轮纠正、导航分页 |
| 并发管理 / MKV 队列 / 进度 / 取消 | ~0.5k | ✅ `tasks.rs`：普通+MKV 队列、转码独占槽、`/progress`、`/canceltask` |
| 命令 / 权限 / OAuth / 深链 | ~3.3k | ✅ 22 条命令（另 `/s` 是 `/searchanime` 短名；TS 15 条全覆盖）、真 `getChatMember` 判定、OAuth 四步 + 自动刷新 |
| 单集纠正 `/jz` | — | ✅ 按两个章节 id 迁移资源，含 `N_ep`/`F_anime` 复用 |
| TDLib 读接口 / inline | — | ✅ `fuyubot-tdlib-ext`；inline 两个 handler（`@机器人 关键词`） |
| 任务恢复 / 检查点 | ~1.0k | ✅ 检查点（RSS/下载/转码/发送）+ 重启恢复 |
| Web 管理端（Fastify + Vue） | ~7k | 🚧 只读 HTTP API（health/stats/anime/reviews/tasks + Bearer）；前端不做 |

## 数据库（本地 / 云端）

`[mongodb]` 是唯一配置入口：插件启动时调 `storage.mongo.configure` 把 `uri`/`database`/`timeout_ms` 下发给 mongo 插件（云上用 `mongodb+srv://...` 即可，连接串原样透传）。也支持用 `MONGO_URI` / `MONGO_DB` / `MONGO_TIMEOUT_MS` 环境变量覆盖。

判断是否生效：宿主日志里会有 `[anime] storage.mongo 就绪：库 fuyubot（来自 [mongodb] 配置）`；若显示「连接串是 …，[mongodb].uri 不生效」，说明 mongo 插件已被宿主先配好，此时只会切换库名。

## 快速开始

1. 装好四个基础插件（mongo / qb / ffmpeg / 出图），并让宿主加载本插件；
2. 复制 [`xq-anime.toml.example`](xq-anime.toml.example) 成 `xq-anime.toml`，填频道 id、qB 账号、Mongo 地址；
3. 导入第一部番：`/addnewanime <章节 id> <bangumi.moe 或 dmhy 链接>`（建档 + 加种 + 发导航卡）；
   已有番补一集用 `/addanime <章节 id> <链接>`；
4. 之后 RSS 轮询命中就会自动下载 → 转码 → 发番剧频道；匹配不准时用
   `/jz <源章节 id> <目标章节 id> [字幕组]` 把单集改挂到正确的番。

常用命令：`/selftest`（一条命令自检服务/库/RSS/匹配）`/addanime` `/addnewanime` `/updateanime`
`/jz` `/testbgimg` `/searchanime` `/setanimer18` `/animeblacklist` `/progress` `/canceltask`
`/status` `/web` `/help`。完整 22 条见 `/help`。

内联：在任意聊天里打 `@机器人 <番剧关键词>` 取导航卡（点开就是完整导航正文），打 `@机器人 <章节 id 或 bgm.tv/ep/<id> 链接>` 取这一集的视频，选中即发送（复用频道里已上传的文件，不重传）。

## 构建与验证

```bash
cargo build --workspace       # 全部插件（0 warning）
cargo test --workspace        # 440 个单测（离线，不碰网络/数据库）
cargo run -p ultralight-gen-image --example render_card          # 出图（含渲染器）
cargo run -p fuyubot-plugin-ffmpeg --example plan_smoke         # 转码选路 + 真实转码
cargo run -p anime-core --example rss_probe                     # RSS 三源抓取 + 合并 + 标题解析
cargo run -p anime-core --example matching_probe                # 番剧匹配 + 集数对齐（--live 用真实 RSS）
```

### 真宿主回放（已实测）

`--replay` 不接 TDLib，但会真的加载插件、跑事件循环、把 `sendMessage` 记下来：

```bash
cargo build -p fuyubot-plugin-anime
mkdir -p target/abi-probe/plugins
cp target/debug/fuyubot_plugin_anime.dll target/abi-probe/plugins/
<FuyuBot>/target/debug/fuyubot.exe run \
  --replay examples/replay/commands.json \
  --plugin-dir target/abi-probe/plugins --replay-wait 500
```

2026-10-26 实测结果：插件加载成功、命令全部注册、`/ping` `/status` `/help` 各发出一条
字段正确的 `sendMessage`（含 `inputMessageText` + `reply_to`）、服务发现四项全 ✓、
后台 RSS 真的抓到三源（bangumi 23 / dmhy 23 / acgnx 11）、`stop` 正常调用。
Mongo / qB 没起时只打 warn，不影响插件运行。

### 在真宿主里验插件（不需要 Telegram 账号）

回放模式不接 TDLib，插件的 `sendMessage` 会被记下来而不真的发出，所以离线就能验证
「插件能不能加载、命令认不认得、请求组得对不对」：

```bash
cargo build -p fuyubot-plugin-anime
mkdir -p target/abi-probe/plugins
cp target/debug/fuyubot_plugin_anime.dll target/abi-probe/plugins/

# 需要上游 Fuyu-Bot 的 fuyubot 可执行文件
<FuyuBot>/target/debug/fuyubot.exe run \
  --replay examples/replay/commands.json \
  --plugin-dir target/abi-probe/plugins --replay-wait 500
```

期望：`已加载插件 xq.anime`、每轮 `listeners: 1`、三条 `sendMessage`（`/ping` `/status` `/help`），
第 4、5 条输入（外来命令 + 普通聊天）**没有**回复。

`fuyubot-abi` 目前指向上游检出目录（`[workspace.dependencies]`），发到 crates.io 后换成版本号即可。

运行时依赖：
- 渲染器需要 Ultralight：`powershell -File scripts/fetch-ultralight.ps1`
- 转码需要 ffmpeg/ffprobe：放 `FFMPEG_DIR` 或插件同级目录

## 文档

- [`docs/plugin-architecture.md`](docs/plugin-architecture.md)：插件拆分理由、服务清单、
  同步服务怎么扛慢操作、事件约定
- [`docs/image-render.md`](docs/image-render.md)：出图插件架构、模板契约、踩坑记录
- [`docs/HANDOFF.md`](docs/HANDOFF.md)：**交接文档**——现状、未完成清单、架构决策、踩坑记录、接手顺序
- `crates/*/README.md`：各插件的服务名、方法与配置
