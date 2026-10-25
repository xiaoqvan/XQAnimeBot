# 插件拆分与跨插件服务

原来的 TS 版是一个大插件：数据库、qB、转码、出图全在 `plugins/XQAnimeBot` 里，
`database/query.ts` 804 行、`qBittorrent/client.ts` 629 行，业务逻辑和基础设施混在一起。

Rust 版把它们拆成**独立插件**，靠宿主提供的两样东西通信：

- **服务表**：`host.get_service("名字")` → 一组「方法名 → JSON 函数」；
- **事件总线**：`host.emit(type, payload)` / `host.on(type, filter, cb)`。

```
fuyubot-plugin-anime  ──┬─ storage.mongo         ──▶ MongoDB
（业务，骨架）           ├─ download.qbittorrent  ──▶ qBittorrent WebUI
                        ├─ media.ffmpeg          ──▶ ffmpeg 子进程
                        └─ image.render          ──▶ image-shot.exe ──▶ Ultralight
```

## 为什么这么拆

| 理由 | 具体收益 |
| --- | --- |
| 依赖隔离 | 只有 mongo 插件链接官方驱动，只有 ffmpeg 插件链接 sidecar；业务插件不背这些依赖 |
| 崩溃隔离 | 渲染器本来就必须独立进程（Ultralight 要主线程）；ffmpeg 也是子进程，崩了不影响机器人 |
| 可替换 | 想换 SQLite → 新写一个注册 `storage.*` 的插件，业务不用改（只要方法签名一致） |
| 可单独升级 | 换 ffmpeg 参数不用重编动漫插件 |
| 复用 | 出图、转码、存储跟动漫毫无关系，下一个插件直接调 |

代价是**多了一层 JSON 序列化**和**跨服务调用的错误处理**。这个代价是值的：
跨插件调用的边界天然强制了「接口先想清楚」。

## 服务清单

| 服务 | 插件 | 方法数 | 状态 |
| --- | --- | --- | --- |
| `storage.mongo` | `fuyubot-plugin-mongo` | 11 | ✅ 本机 MongoDB 全 CRUD 联调通过 |
| `download.qbittorrent` | `fuyubot-plugin-qb` | 12 | ✅ 本机 qB 5.1.2 读+写联调通过（只适配 5.0+） |
| `media.ffmpeg` | `fuyubot-plugin-ffmpeg` | 9 | 本机实测 copy / NVENC 两条路 |
| `image.render` | `ultralight-gen-image` | 2 | 本机实测出图（ultralightGenImage） |

每个插件的 README 里有完整的入参表，这里不重复。

## 公共外壳：`fuyubot-service`

ABI 层的服务只有三样东西：名字、方法表、JSON 进出。每个插件都要拼一遍，
很容易抄错指针生命周期。`fuyubot-service` 把它收成一处：

```rust
Service::new("storage.mongo", vec![
    ("ping", svc_ping as Handler),
    ("find", svc_find as Handler),
])
.register(&host)?;
```

它负责：泄漏 vtable 成 `'static`（宿主只存裸指针）、线程局部回复缓冲（调用方读完即弃）、
统一的 `ok()` / `error()` / `parse()`、以及配置槽 `Settings<T>`。

## 同步服务怎么扛住慢操作

`ServiceMethodFn` 只有同步形态，而 ABI 的回调线程不能堵。三类慢操作的处置：

| 插件 | 慢在哪 | 处置 |
| --- | --- | --- |
| mongo | 驱动是 async | 专用 tokio 运行时 + `Handle::block_on` 摊平，业务侧不沾 async |
| qb | HTTP 往返 | 同步 HTTP + 全局超时；客户端按需重建 |
| ffmpeg | 转码几分钟 | `transcode` 只起线程就返回 `job_id`，进度走事件，状态可轮询 |

**共同点**：没有任何一个服务方法会在回调线程上跑超过一次网络往返或一次进程启动。
转码这种长任务必须是「起任务 + 查状态」两段式，不能同步等。

## 事件约定

| 事件 | 来源 | 载荷 |
| --- | --- | --- |
| `ffmpeg.progress` | ffmpeg 插件 | `{id, percent, speed, fps, eta_ms, ...}` |
| `ffmpeg.done` / `ffmpeg.failed` / `ffmpeg.canceled` | ffmpeg 插件 | 同上，含 `error` |
| `mongo.error` | mongo 插件 | `{error}` |

订阅方按 `filter_json` 过滤，例如只关心某个任务：

```rust
host.on("ffmpeg.progress", Some(r#"{"id":"ff-18f3a1-3"}"#), callback, ctx);
```

## 现状与后续

四个基础设施插件已可独立编译、单测通过、本机跑过真实 ffmpeg 与 Ultralight。
**没做的**：

- mongo / qb 已在真机联调（见各 README 的「联调」一节，都带可复跑的 example）；
- 服务调用目前靠业务插件手写 `host::call`，还没有生成式的类型化客户端；
- 动漫插件壳（`fuyubot-plugin-anime`）仍是骨架，编排逻辑（RSS → 匹配 → 下载 → 转码 → 发频道）
  还没接上这些服务。
