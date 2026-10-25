# XQAnimeBot-Rust

原 [XQAnimeBot](F:/Node.JS/XQAnimeBot)（TypeScript，105 文件 / 20.5k 行）的 Rust 版：
FuyuBot 插件形态，RSS → 标题解析 → 匹配 → qBittorrent → 转码 → 发频道。

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
| `fuyubot-plugin-anime` | cdylib | 动漫业务插件：插件壳 + 命令路由（**已在真宿主跑通**，业务链路待接） |

```
fuyubot-plugin-anime  ──┬─ storage.mongo         ──▶ MongoDB
                        ├─ download.qbittorrent  ──▶ qBittorrent WebUI
                        ├─ media.ffmpeg          ──▶ ffmpeg 子进程
                        └─ image.render          ──▶ image-shot ──▶ Ultralight
```

## 迁移进度

**没有完成。** 基础设施四个插件已落地并可独立运行；业务插件壳已在真宿主跑通，业务链路仍空。

| 模块 | TS 规模 | Rust 状态 |
| --- | --- | --- |
| 出图（Vue/satori/resvg → Ultralight） | ~1.1k | ✅ 独立插件，实测出图 |
| ffmpeg 转码 | ~0.5k | ✅ 独立插件，实测 copy/NVENC 两条路 |
| qBittorrent 客户端 | ~1.1k | ✅ 独立插件（手写 5.0 API，真机联调通过） |
| MongoDB 存储层 | ~2.5k | ✅ 独立插件（通用 CRUD，本机全链路联调通过） |
| 标题解析 / 字幕组规则 | ~1.3k | ✅ 完成 |
| 配置 / 数据模型 / 日志 | ~0.8k | ✅ 完成 |
| 番剧卡片数据 payload | ~0.3k | ✅ 完成 |
| RSS 三源解析（bangumi/dmhy/acgnx） | ~1.2k | ✅ 完成（三源 + 合并去重 + 条目构建，真网络验证） |
| 番剧匹配（season + 集数对齐） | ~0.6k | ✅ 完成（打分 + 阈值 + 放送日期窗口，真数据验证） |
| 插件壳 / 命令路由 | ~3.3k | 🚧 3 个命令 + 后台 RSS 轮询（已接存储去重与番剧/集数匹配，只记日志） |
| 业务存储层（db） | ~2.5k | 🚧 `anime-core::store` 就绪（Storage trait + 业务查询），插件侧已用 `storage.mongo` 实现 |
| TDLib 读接口 / inline | — | 🚧 `fuyubot-tdlib-ext`（getMessage/getChat/getMessageLink/answerInlineQuery 等 10 个） |
| 业务编排（RSS→匹配→下载→转码→发频道） | ~2k | ❌ 未开始 |
| Bangumi API + AI Agent | ~1.8k | ❌ 未开始 |
| 任务恢复 / 检查点 / 审核 | ~1.0k | ❌ 未开始 |
| Web 管理端（Fastify + Vue） | ~7k | ❌ 未开始（决策待定） |

## 构建与验证

```bash
cargo build --workspace       # 全部插件（0 warning）
cargo test --workspace        # 207 个单测（离线，不碰网络/数据库）
cargo run -p ultralight-gen-image --example render_card          # 出图（含渲染器）
cargo run -p fuyubot-plugin-ffmpeg --example plan_smoke         # 转码选路 + 真实转码
cargo run -p anime-core --example rss_probe                     # RSS 三源抓取 + 合并 + 标题解析
cargo run -p anime-core --example matching_probe                # 番剧匹配 + 集数对齐（--live 用真实 RSS）
```

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
