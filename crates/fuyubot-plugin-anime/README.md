# fuyubot-plugin-anime

XQAnimeBot 的业务插件：**RSS 订阅 → 标题解析 → 番剧匹配 → qB 下载 → 转码 → 发频道**。

它自己不实现任何基础设施：存储、下载、转码、出图都通过宿主注册的服务拿
（`storage.mongo` / `download.qbittorrent` / `media.ffmpeg` / `image.render`），
业务判断则全部下沉到 [`anime-core`](../anime-core/README.md)。

## 模块

```
lib.rs       impl Plugin：init 记日志、start 装配置+订阅、stop 退订并从运行时摘掉
runtime.rs   运行时状态：配置从哪来、`Arc<Runtime>`、运行时长
bot.rs       命令路由：一张静态表 = 有哪些命令 + /help 的内容
td.rs        TDLib 出口：Target（发给谁/回哪条）→ Simplify::send_message
```

**分层原则**：`lib.rs` 只做生命周期，`bot.rs` 只做「哪条命令归谁」，`td.rs` 只做
「怎么发出去」。业务规则一行都不许写在壳里——写在 `anime-core` 才能纯 `cargo test`。

## 命令

| 命令 | 作用 |
| --- | --- |
| `/ping` | 测活，回运行时长 |
| `/status` | 版本、运行时长、配置来源、RSS 参数、**四个依赖服务在不在** |
| `/help` | 命令列表（由 `bot::COMMANDS` 生成，不会和实现漂移） |

认不出的命令**静默忽略**：同一宿主里还装着 `/start`、`/help`、`/plugins` 等内置插件，
抢别人的命令比漏一条更糟。

## 配置怎么来

宿主传给 `plugin_start` 的 JSON **只用来定位业务配置**，不承载业务字段：

```json
{ "config_path": "D:/bots/xq/xq-anime.toml" }
{ "config_toml": "[rss]\ninterval_secs = 42" }
{}
```

优先级：`config_toml` → `config_path` → `anime-core::config::Config::discover`
（环境变量 `XQ_ANIME_CONFIG` → 当前目录 `xq-anime.toml`）→ 内置默认值。
最后统一走 `apply_env_overrides()`，凭据可以只放环境变量、不落盘。

业务配置的字段定义在 [`anime-core/src/config.rs`](../anime-core/src/config.rs)，
不在这里抄第二份。

## 本地验证（不需要账号，不需要 TDLib）

回放模式离线跑，插件的 `sendMessage` 会被宿主记下来而不真的发出：

```bash
cargo build -p fuyubot-plugin-anime
mkdir -p target/abi-probe/plugins
cp target/debug/fuyubot_plugin_anime.dll target/abi-probe/plugins/

fuyubot run --replay examples/replay/commands.json \
  --plugin-dir target/abi-probe/plugins --replay-wait 500
```

期望看到：

- `已加载插件 xq.anime`
- 每个 `updateNewMessage` 的 `listeners: 1`
- 三条 `sendMessage`（`/ping` `/status` `/help`），第 4、5 条输入**没有**回复

样本 `examples/replay/commands.json` 里第 4 条是不属于本插件的命令、第 5 条是普通聊天，
它们必须沉默——这条断言和三条回复一样重要。

## 当前进度

已完成：插件骨架、命令路由、TDLib 出口、配置装载、运行时状态，**在真宿主里回放验证通过**。

还没做（按顺序）：

1. `/rss` 手动抓取一次（把 `rss::fetch_all` + `matching::match_item` 接进壳里，开线程跑）
2. `db` 层（业务查询，把库文档投影成 `AnimeCandidate` / `EpisodeMeta`）
3. 定时轮询任务 + 下载/转码编排
4. 回调路由（内联键盘、审核按钮）
5. 命令全集（TS 版 14 个）
