# 补齐计划：对齐 newdbtest + 配置迁入全局

> 目标仓库：`D:/GitHub/XQAnimeBot-Rust`（分支 `rust`，HEAD `87aadab`）
> 对齐对象：`github.com/xiaoqvan/XQAnimeBot` 的 **`newdbtest`** 分支（已克隆到 `d:/GitHub/_ts-newdbtest`）
> 范围：**不含 web 端**
> 制定时间：2026-10-27
> 前置：`cargo test --workspace` = 306 passed / 0 failed

## 0. 本次计划要解决的两件事

1. **参考实现搞错了**：之前对照的是 `main`（`F:/Node.JS/XQAnimeBot`，插件副本 10,617 行），
   而当前代码在 `newdbtest`（18,289 行）。三处判断因此出错（`btdata` / `eps` /
   `navMessageLink`），已在对照过程中更正并实测确认。
2. **配置要放进全局配置**：宿主 ABI v3 有 `get_config` / `set_config`，配置统一落在
   `fuyubot.toml` 的 `[plugin.xq.anime]` 段。这是本次的**第一项改动**，因为它决定了
   后面所有阶段怎么读配置。

---

## 1. 配置迁移（先做，其它阶段都依赖它）

### 1.1 现状

[runtime.rs](crates/fuyubot-plugin-anime/src/runtime.rs) 的 `Bootstrap` 现在有三条来源：
内联 `config_toml` → 显式 `config_path` → `Config::discover`（扫 `XQ_ANIME_CONFIG`
环境变量和当前目录的 `xq-anime.toml`）。业务字段全在这份**插件私有** TOML 里。

### 1.2 改成什么样

来源收敛成三层，顺序固定：

| 优先级 | 来源 | 用途 |
| --- | --- | --- |
| 1 | `plugin_start` 的 `config_toml` 内联 | **测试 / 回放**专用，保证单测确定性，不受宿主环境影响 |
| 2 | 宿主全局配置的 `[plugin.xq.anime]` 段 | **生产唯一真相**，经 `host.get_config("xq.anime", "")` 读整段 |
| 3 | 内置默认值 | 没配也能起来，日志里说清「用的默认值」 |

**删除** `Config::discover`（不再扫当前目录）和 `config_path`（全局配置已覆盖这个需求）。

**保留** `apply_env_overrides`：凭据一律允许走环境变量（`XQ_MONGO_URI` / `XQ_QB_*` /
`DEEPSEEK_API_KEY`），不写进配置文件。

### 1.3 具体改动

- `Config` 增加 `from_json(&Value)` —— 全局配置给的是 **JSON** 不是 TOML，
  现在只有 `parse(&str TOML)` 和 `load(path)`，缺这条入口。
  `deny_unknown_fields` 对 JSON 同样生效，字段对齐的约束不变。
- `Bootstrap::resolve()` 改成：内联 → `host.get_config("xq.anime", "")` → `Config::default()`。
  返回里带上「配置来自哪一层」，启动日志照旧打出来。
- **顺带修掉一个现成的 bug**：`xq-anime.toml.example` 里 `[mongodb] timeout_ms`
  现在会被 `deny_unknown_fields` 拒掉（照抄样例起不来，已实测）。
  处理办法是**把 `timeout_ms` 加进 `MongoConfig`**（而不是从样例里删）——
  [lib.rs:163](crates/fuyubot-plugin-anime/src/lib.rs#L163) 下发 `storage.mongo` 时
  本来就写死 `10_000`，迁到全局后它应该变成可配值，下发时用配置值。
- 新增 `set_config` 写入口，供 `/animeblacklist` 真正增删黑名单（现在只能列，
  [bot.rs:522](crates/fuyubot-plugin-anime/src/bot.rs#L522)）。写 `rss.blacklist` 点路径。
- 交付一份可直接粘进 `fuyubot.toml` 的段样例，**取代** `xq-anime.toml.example`。

### 1.4 验收

- `cargo test --workspace` 全绿（回放/单测走内联通道，不受影响）。
- 在 `F:/Rust/FuyuBot/fuyubot.toml` 里加 `[plugin.xq.anime]` 段跑真机回放：插件读到、
  启动日志打印来源、改一个值生效。
- `fuyubot config get` 能看到该段；`set_config` 写入后 `fuyubot.toml` 被回写。
- 样例段粘进干净的 `fuyubot.toml` 后插件能起来（现在粘 `xq-anime.toml.example` **起不来**）。

### 1.5 如实标注的边界（宿主侧已知限制）

- `set_config` 落盘是**全量重写** `fuyubot.toml`，用户手写的注释会被丢掉
  （[decisions.md 变更 11](f:/Rust/FuyuBot/docs/decisions.md) 已留账）。
  所以：`set_config` 只用于「程序化改自己那几个键」（黑名单之类），
  日常配置仍建议手改 `fuyubot.toml`。
- `set_config` 不广播事件，别的插件不会感知到变更。

---

## 2. 数据模型修正（第二优先，越晚改越贵）

### 2.1 `bangumi_users` —— 现在硬不兼容，必须改

TS 用 camelCase：`accessToken` / `refreshToken` / `expiresIn` / `tokenType` /
`bgmUserId` / `scope` / `authTime`；`id` 是**内部自增序列**，Bangumi 用户 id 放 `bgmUserId`。

Rust 的 [BangumiUserDoc](crates/anime-core/src/models.rs#L388-L405) 是 `access_token`
（**snake_case，没写 rename**），`id` 存的是 Bangumi 用户 id，还**必填** `username` / `nickname`。

实测：拿 newdbtest 真实形状的文档反序列化 → `missing field 'username'`。
即使补齐 `username`，`accessToken` 也进不到 `access_token`（有 `#[serde(default)]`，静默变空串）。

后果：接老库时 `/collect` 和收藏回调一律报「请先 /bindbangumi 绑定」。

**改法**：字段全部对齐 camelCase + `#[serde(default)]`，`id` 改成内部自增、`bgmUserId` 存 Bangumi 的。
同时把「自增序列」抽出来（见 2.3）。

### 2.2 `pendingReviews` —— 主键和载荷都不一样

| | TS | Rust 现状 |
| --- | --- | --- |
| 主键 | 自增 `id` | 字符串 `key`（`rss:<cleanTitle>`） |
| 载荷 | `item` / `anime` / `episodeId` / `episodeSort` / `sentMessages[]` / `primaryMessage{}` / `matchDetail` / `episodeMatch` | 一个 `payload` blob |
| 回调数据 | `Y_anime?r=<id>` / `F_anime?r=<id>` | `review:approve:<key>` |

[store.rs:423](crates/anime-core/src/store.rs#L423) 的注释写着「宿主接口没有自增 id」——
不是宿主没有，是当初照 `main` 抄的。改成结构化文档 + 自增 id，回调格式随之定下来
（这一步不定，阶段 3 的审核流程没法写）。

### 2.3 自增序列：跟 TS 一致用 `config` 集合

TS 的 `getNextSequence(name)` 是 `config` 集合里 `{type, seq}` 配 `$inc`。
三个序列：`cacheItemId` / `bangumi_user_id` / `pendingReviewId`。

Rust 的 `cache_item_add` 现在是 `max(id)+1`（[store.rs:221](crates/anime-core/src/store.rs#L221)
注明了单写者等价）。功能够用，但不推进 `config` 里的序列文档，两边交替写会撞号。
统一改成 `config` 序列，与 TS 互通。

### 2.4 补三张表：`resources` / `cache_resources` / `cacheAnime`

这三张是 `btdata` 的**替代品**，Rust 现在一张都没有（`"resources"` 唯一的命中是
ultralight 渲染器的静态资源目录）。

- `resources` / `cache_resources`：`EpisodeResourceDoc`（`anime_id` / `episode` /
  `episodeId` / `groups[]` / `title` / `message` / `messages[]` / `videoid` /
  `unique_id` / `cache_id` / `source` / `createdAt`）。导航正文、内联取视频、
  集数纠正全靠它。
- `cacheAnime`：审核中的番剧（TS 的 `saveAnime(anime, cache=true)`）。

### 2.5 顺带修正 `navVideoMessage` 的语义

newdbtest 里 `navVideoMessage` 是**导航分页消息**（`page` = 第几页，指向导航频道里的
文本消息），集数资源不在这里。Rust 的
[push_episode](crates/fuyubot-plugin-anime/src/nav.rs) 却把每集视频消息连同一个 TS 没有的
`episode` 字段写进去——两边会互相污染。改成只写导航分页消息，集数资源进 `resources`。

### 2.6 验收

- 用 TS 真实形状的文档（含 camelCase、`airdate: null`、`score` 为字符串）跑反序列化单测全绿。
- FakeStorage 单测覆盖三张新表的 upsert / 查询 / 迁移。
- 与 newdbtest 双向互通：Rust 写的 pendingReview TS 能读，反之亦然。

---

## 3. 业务主链：新番 + 缓冲频道 + 审核

### 3.1 现状：新番只打一行日志

[poll.rs:331-334](crates/fuyubot-plugin-anime/src/poll.rs#L331-L334) 的 `SeasonDecision::New`
只做 `new_anime += 1`。

newdbtest 的 `handleNewAnime` 走两条路：

- **置信度 ≥ 0.9（先发后审）**：建档 → 匹配章节 → 下载 → 发导航消息 → 发视频到正式频道 →
  `saveAnimeResource` → 再次更新导航 → `createPendingReview` → 带候选列表的审核提示。
- **< 0.9（回退）**：`animeinfo` 搜索 → `buildAndSaveAnimeFromInfo(cache=true)` → 下载 →
  发**缓冲频道** → `saveAnimeResource(cache=true)` → 提示确认。

### 3.2 缓冲频道（现在完全不存在）

`sendMegToCache` 发到 `ADMIN_GROUP_ID` 的 `ANIME_GROUP_THREAD_ID` 话题，标题只用
`item.title`，发完把种子状态置 `等待纠正`。

「命中了但集数没对齐」是最常走的一条路，Rust 现在只是
`log!(Info, "命中但集数没对齐")`（[poll.rs:310-317](crates/fuyubot-plugin-anime/src/poll.rs#L310-L317)），
视频既没发出去也没给管理员纠正入口。

### 3.3 审核提示：1 个 → 4 个

newdbtest 有 `promptAdminProvideAnimeInfo` / `promptAdminConfirmAnime` /
`promptAdminConfirmAnimeEpisodes` / `promptAdminConfirmAnimeWithCandidates`。
Rust 只有 [review.rs](crates/fuyubot-plugin-anime/src/review.rs) 一个通用提示（纯文本、
无候选、无置信度）。

### 3.4 回调：2 种 → 6 种

`Y_anime` / `F_anime` / `N_anime` / `N_ep` / `colorep` / `chgcol`。

其中 `N_anime` / `N_ep` 是**多轮交互**：改完提示语后用 `client.iterUpdates()` 等管理员
回复这条消息的 reply，解析出章节 id 或 bgm 链接再继续。Rust 没有「等下一条回复」这种
状态机，需要新增（宿主侧 `on("update")` 订阅 + reply 匹配）。

### 3.5 验收

- 构造一条库里没有的新番 RSS 条目 → 走完「建档 → 下载 → 发频道（或缓冲频道）→
  生成待审核 → 管理群收到带候选的提示」。
- 点「正确」/「错误」/「提供」三个按钮各自走通，包括多轮回复纠正集数。
- 集数没对齐的条目落到缓冲频道，种子状态是 `等待纠正`。

---

## 4. 用户可见部分：导航正文 + 视频文案 + 媒体细节

- `navmegtext`：1024/4096 双限测量、`summary` 300→100 逐级压缩、按 `compareEpisode` 排序、
  `[第N页](link)` 页码导航、按字幕组分组的资源区。Rust 现在只有一行 caption。
- `AnimeText`：原名/中文名/发布组/发布时间/追踪标签 + 导航·收藏·标记看过·吐槽箱四个链接。
  Rust 现在是 `名字｜第 N 集`（[chain.rs:725](crates/fuyubot-plugin-anime/src/chain.rs#L725)）。
- 视频发送：真实 `width`/`height`/`duration`、封面、`has_spoiler = r18`、
  分段走 `sendMessageAlbum`、`timeout: 1800`、失败重试一次。
  Rust 现在 `duration/width/height` 写死 0、`has_spoiler` 写死 false
  （[td.rs:123](crates/fuyubot-plugin-anime/src/td.rs#L123)）、无封面、无相册、无重试。
  **封面抽取需要 ffmpeg 插件新增一个 thumbnail 方法**（现有方法只有
  `ping`/`capabilities`/`probe`/`plan`/`transcode`/`extract_subtitle`/`split`/`job_*`/`prune`）。
- 种子状态机：`等待下载 → 等待元数据 → 下载中 → 下载完成 → 上传中 → 完成 / 等待纠正 / 失败`。
  现在只写 `Downloading` 和 `Done`。
- 标签过滤：`extractFilteredTagNames`（剔除 `^\d{4}年`、纯数字、`tagExcludeList`）。
  Rust 现在原样搬运。

**验收**：发一条真实视频，导航消息是完整正文、视频 caption 带全部标签和四个链接、
`r18` 番剧的视频带 spoiler、分段发成相册。

---

## 5. 并发管理器 + MKV 队列 + 进度/取消

newdbtest 的 `AnimeProcessorManager`：普通 worker 3 + **MKV 独立队列 1** +
重型转换独占槽 1 + `progressMap`（阶段/百分比/标签）+ `cancelActiveByIndex` /
`cancelActiveByTitle` / `cancelAllActive`。

Rust 现在是一个 `const WORKERS: usize = 3` 的 cvar 计数，`/canceltask` 直接回
「暂不支持单独取消」，`/progress` 只打 4 个计数器。

还要补 `checkTorrentFormat`：下载前借 qB 的 metadata 通道（paused 态加入、轮询
`has_metadata` 最多 60 秒）判断容器，纯 MKV 进独立队列。现在没有预检，也没有 MKV 队列。

**验收**：`mkv_workers` / `workers` 两个配置真的生效；`/progress` 列出每条任务的
阶段和百分比；`/canceltask` 能真的取消一条并释放槽位。

---

## 6. 命令 / 权限 / Bangumi OAuth

- **补齐命令**：`start`（深链 `bgm_cb-` / `collection-` / `eplook-`）、
  `ConAnimeInfo`(`jz`)、`addnewanime`。
- **`addanime` 语义现在是错的**：TS 是 `/addanime <章节ID> <磁力URL>`（拉 torrent 详情 →
  解析 → 下载 → `saveAnimeResource` → 更新导航 → `editMessageCaption` 补全文案）；
  Rust 的 `/addanime <条目id>` 只是拉 Bangumi 条目建档。
- **权限**：TS 是 `isUserAdmin(ADMIN_GROUP_ID, sender)`（真调 `getChatMember`）+
  `config` 里的 `super_admin`。Rust 只有 `owner_id` 等值白名单，且 `owner_id == 0`
  时**完全不校验**。`super_admin` 正好可以放进全局配置的 `[plugin.xq.anime]` 段（阶段 1 铺好的路）。
- **OAuth**：现在只有 [oauth.rs](crates/anime-core/src/bangumi/oauth.rs) 的 `refresh_token`，
  且从没被调用。TS 有四步：`getAuthUrl` / `getAccessToken` / `refreshAccessToken` /
  `getTokenStatus`，外加 `getBgmToken` 按 `authTime + expiresIn` 提前一分钟自动刷新。
- **接口**：补 `getEpisodeById`(`GET /v0/episodes/{id}`)——`/addanime`、`/addnewanime`、
  `/jz`、`N_anime`、`nullEp` 全依赖它。另补 `retryRequest`（3 次重试、间隔 10 秒）。

---

## 7. LLM Agent：建议延后（待你拍板）

newdbtest 的 `bangumiAgent.ts` 有 1102 行：OpenAI Responses API + **工具调用**
（`searchAnimeCandidates` / `getRelatedSubjects` / `getEpisodeDetail`）、
`MAX_ITERATIONS=8` 的迭代循环、输出带 `confidence` 和 `reason`。
Rust 的 [ai.rs](crates/anime-core/src/ai.rs) 只有 213 行，两次单发 chat/completions。

差距是**数量级**的，而且依赖 OpenAI Responses API 的工具调用协议。建议：

- 阶段 7 之前先用「规则匹配 + 现有简单 AI 兜底」跑通主链（阶段 3 的 `≥0.9 先发后审`
  可以先用规则置信度代替 LLM 置信度）。
- 工具调用版单独作为一个阶段，等你确认要照搬 Responses API 还是换成别家。

---

## 8. 单独挂起：inline 模式

newdbtest 注册了两个 inline handler（`animeNavigation` / `episodeVideo`）。
**但这条现在做不了**：宿主 `fuyubot-abi` / `fuyubot-core` 里 `inline` 是 0 命中，
ABI 只有 `UPDATE_EVENT` / `RESPONSE_EVENT` 和 `on(...)`，没有 inline 注册入口。
`fuyubot-tdlib-ext` 的 `answer_inline_query` 只能发、收不到查询。

这是**宿主侧限制**，得先在 FuyuBot 加 inline 事件分发，不是本仓能单独补的。
建议先挂着，等宿主侧有入口再说。

---

## 9. 文档与交付

- **先删掉/重写** [gap-audit-2026-10-27.md](docs/gap-audit-2026-10-27.md)——那份是照
  `main` 写的，结论有三处错误，留着会误导下一个接手的人。
- [HANDOFF.md](docs/HANDOFF.md) §3/§4 自相矛盾、测试数写 281（实际 306）；
  [audit-2026-10-26.md](docs/audit-2026-10-26.md) 是「15 命令 → 3」的旧口径。
- 所有文档里「TS 版」的引用统一改指 `newdbtest`，并写明**参考分支**——
  这次的错就是没写分支导致的。
- 最后按你的要求，作为**新分支**提交到 `github.com/xiaoqvan/XQAnimeBot`。
  分支名待定（本地现有 `rust` / `rust-migration` / `main`，`origin/rust` 已存在）。

---

## 执行顺序

| 阶段 | 内容 | 依赖 |
| --- | --- | --- |
| 1 | **配置迁入全局** | 无（先做） |
| 2 | 数据模型修正（`bangumi_users` / `pendingReviews` / 三张表 / 序列 / navVideoMessage） | 1 |
| 3 | 新番链路 + 缓冲频道 + 审核（4 提示 / 6 回调） | 2 |
| 4 | 导航正文 + 视频文案 + 媒体细节 + 状态机 + 标签过滤 | 2、3 |
| 5 | 并发管理器 + MKV 队列 + 进度/取消 | 4 |
| 6 | 命令 / 权限 / OAuth / `getEpisodeById` | 2 |
| 7 | LLM Agent（**待定**） | 3 |
| 8 | 文档 + 分支交付 | 全部 |

阶段 1、2 是「越晚改越贵」的，建议先做完再动 3。

## 需要你拍板的三件事（已定案）

1. **LLM Agent（阶段 7）**：~~延后~~ → **现在就照搬 Responses API 工具调用**。
   阶段 7 不再挂起，按计划照搬 newdbtest 的 `bangumiAgent.ts`（OpenAI Responses API +
   工具调用 + 迭代循环 + confidence/reason）。
2. **`deny_unknown_fields`**：**保留严格**。字段拼错直接报错，不静默忽略。
3. **分支名**：**继续推 `rust`**（`origin/rust` 已存在且等于当前 HEAD）。

## 已定案：黑名单位置

计划 §1.3 原写「`set_config` 写 `rss.blacklist` 点路径（全局 TOML）」，但 TS 版和当前
Rust 都把黑名单存在 MongoDB 的 `config` 集合 `{key:"animeBlacklist", list:[...]}`。
**已定案沿用 MongoDB config 集合**（与 TS 完全对齐，接老库互通），`set_config` 不再
用于黑名单。实现：`anime-core::store` 新增 `config_list_add` / `config_list_remove` /
`config_list_clear`，`/animeblacklist` 支持 list/add/remove/clear 子命令。

## 执行进度（2026-10-27）

- **阶段 1（配置迁入全局）已完成**：`Config::from_json` + `MongoConfig.timeout_ms`（修复
  `xq-anime.toml.example` 起不来的 bug）；`Bootstrap::resolve` 三层来源（内联 → 宿主全局
  `[plugin.xq.anime]` → 默认值）+ `ConfigSource` 枚举；删除 `config_path`/`discover`；保留
  `apply_env_overrides`；`configure_storage` 用配置的 timeout_ms；样例重写为 `[plugin.xq.anime]`
  段；黑名单增删走 store 层（MongoDB config 集合）。
- **阶段 2（数据模型修正）已完成**：`BangumiUserDoc` 对齐 camelCase + id 自增 + bgmUserId；
  `PendingReviewDoc` 结构化 + 自增 id；`next_sequence` 用 config 集合 `{type, seq}`（三个序列
  cacheItemId/bangumi_user_id/pendingReviewId 对齐 TS）；新增 `EpisodeResourceDoc`（resources/
  cache_resources）+ `save_cached_anime`（cacheAnime）；`navVideoMessage` 删 `episode` 字段、
  只存导航分页消息；回调格式 `review:approve:<key>` → `review:approve:<id>`。
  测试 306 → 313 全绿（含 TS 真实 camelCase 形状的反序列化用例）。
- **阶段 7（LLM Agent）已完成**：`ai/agent.rs` 照搬 Responses API 工具调用
  （`searchAnimeCandidates` / `getRelatedSubjects` / `getEpisodeDetail`、迭代上限、
  去重、`confidence`/`reason`）；`bangumi/tools.rs` 提供工具后端；`Http::with_user_agent`
  + `post_json_auth_statused`（502/429/5xx 才重试）；`/aimatch` 与 `agent_live` 可实测。
  AI 凭据以**配置文件为准**，环境变量仅在配置为空时兜底；默认模型 `deepseek-flash`。
- **阶段 3（新番链路 + 缓冲频道 + 审核）已完成**：
  - 3.1 缓冲频道：`chain::Plan`（发到哪 + 发完干什么）+ `cache_target`
    （`cache_channel`，为 0 时退回管理群 + 番剧话题，与 TS `sendMegToCache` 一致）；
    缓冲路线发完种子状态置 `等待纠正`，正片路线才记导航。
  - 3.2 新番主链：`newanime.rs` 照搬 `handleNewAnime` 两条路
    （Agent 置信度 ≥0.9 先发后审；否则搜索 → 建档(cache) → 缓冲频道）。
    轮询里「命中但集数没对齐」也改走缓冲频道（TS `handleExistingAnime`）。
  - 3.3 四个提示：`prompts.rs`（提供条目 / 确认集数 / 确认番剧 / 带候选先发后审）；
    `review::after_send` 负责发完收尾（记 `resources` 或 `cache_resources`、
    建待审核、按分支发提示）。
  - 3.4 六种回调：`callback.rs` 先判 TS 格式（`Y_anime?r=` / `F_anime?r=` /
    `N_anime?c=` / `N_ep?c=&id=`）再回落旧 `review:approve:<id>`；多轮纠正靠新增的
    `reply.rs`（进程级订阅 `updateNewMessage` + 等回复 + 30 分钟超时 + 回复解析）。
    `N_anime` 纠正完重新下载发送（提示发在下载前），`N_ep`/`F_anime` 只迁移资源归属。
  - 配套：`BgmClient::episode(id)`（`GET /v0/episodes/{id}`）+ `Episode.subject_id`。
  测试 336 → 351 全绿。

## 阶段 3 之后的已知缺口（如实记录）

- `updateAnimeLinks` / 双条目导航刷新只做了一半：纠正后目前只刷新导航标题
  （`nav::refresh_caption`），**没有**重建导航正文（那属于阶段 4 的 `navmegtext`）。
- 纠正流程用「回复提示消息」说话，而不是 TS 的 `editMessageText`：宿主侧目前只有
  `edit_caption`，没有改消息文本的封装。
- 分段上传写进 `resources` 的仍是第一段（`messages[]` / `videoid[]` 未填），
  与阶段 4 的相册发送一起补。
- **阶段 4（导航正文 + 视频文案 + 媒体细节）主体已完成**（commit 3f589e6）：
  - `text.rs`（新增，全部纯函数 + 单测）：`safe_tag` / `format_tags` / 标签过滤
    （`^\d{4}年` 与纯数字，TS `extractFilteredTagNames`）、`anime_date`（只留年月）、
    `summary_trim`（300→100 逐级压缩 + `[...详细]`）、集数解析与排序（**SP/特别篇按
    自己的编号和正片混排**，与 TS 一致）、按字幕组分组的资源区、`navmegtext`
    （1024 上限 / 分页 4096 / 页码导航 / 资源外移）、`AnimeText` 四链接文案、
    `measure`（无 TDLib 时用「可见长度 + 链接数」近似）。
  - `nav.rs`：正文接进导航——`publish` 用正文首段做图注；新增 `refresh`（改首条
    caption + 分页消息 `editMessageText`／新发并记 `navVideoMessage`，page 从 1 起）。
  - `chain`：视频 caption 换 `AnimeText`；`duration/width/height` 用 probe 的真实值；
    `has_spoiler = r18`；发送超时 1800s；种子状态补「上传中」；**每次发送都记资源**。
  - `review`：`after_send` 拆成 `record_send`（每次都记）+ `prompt_after_send`（按路线提示）。
  - `td.rs`：`VideoMeta` / `send_text_wait` / `edit_text`。
- **阶段 4 剩余（宿主侧限制，非本仓能补）**：
  - ~~相册发送~~ **已补齐**：宿主 simplify 加了 `send_message_album`（返回各条最终
    消息）与 `Media::Video.cover` / `start_timestamp`，本仓照 TS 接上——分段 2–10 段
    走相册（只有首项带文案），超过 10 段（TG 相册上限）退化逐条发；主消息 = 首段。
  - ~~视频封面~~ **已补齐**：ffmpeg 插件新增 `thumbnail` 动作（`-ss` 在 `-i` 前做输入
    定位，第 10 秒抽一帧，`-q:v 2`），封面作为 `cover` 随视频发出。
  - `tagExcludeList`（TS 存在库里的标签黑名单）未实现。
  测试 351 → 374 全绿。
