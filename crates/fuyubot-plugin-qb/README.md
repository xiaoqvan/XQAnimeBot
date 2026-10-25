# fuyubot-plugin-qb

qBittorrent 插件（qbweb）：注册 `download.qbittorrent`。

**手写 WebUI v2 API，不用任何 qBittorrent 封装包。** 只依赖一个 HTTP 客户端，
把它当「带 cookie 的 JSON 通道」用。

```
业务插件 ──get_service("download.qbittorrent")──▶ 本插件 ──HTTP──▶ qBittorrent WebUI
```

## 启动配置

```json
{ "url": "http://127.0.0.1:8080", "username": "admin", "password": "***", "timeout_ms": 15000 }
```

`start()` 会登录一次验证账号密码；WebUI 如果开了 IP 白名单，记得把机器人所在机器加进去。

## 方法

| 方法 | 入参 | 说明 |
| --- | --- | --- |
| `ping` | – | 版本 + WebAPI 版本 + 地址 + `supported`（是否 ≥ 5.0） |
| `login` | – | 强制重登（一般不用手动调，403 会自动重登） |
| `version` | – | qB 版本号 + `supported` |
| `transfer` | – | 全局上传/下载速度、连接数 |
| `list` | `filter?, category?, tag?, hashes?, limit?, sort?, reverse?` | 原样返回 qB 字段；`filter` 取值见下表 |
| `files` | `hash` | 种子内文件列表（含进度） |
| `properties` | `hash` | 单个种子详情 |
| `add` | `urls?/file?, category?, savepath?, tags?, paused?, skip_checking?, auto_tmm?, sequential?, rename?` | 磁力/HTTP 或 .torrent；`paused` 对应 5.0 的 `stopped` |
| `pause` / `resume` | `hashes` | 支持 `a|b` 或数组，实际打到 `torrents/stop` / `torrents/start` |
| `delete` | `hashes, delete_files?` | |
| `set_category` | `hashes, category` | |

`filter` 取值（5.0 白名单）：`all` `downloading` `seeding` `completed` `stopped` `running`
`active` `inactive` `stalled` `stalled_uploading` `stalled_downloading` `checking` `moving` `errored`。
4.x 的 `paused` / `resumed` 会自动映射成 `stopped` / `running`；**其它未知取值直接报错**。 

示例：

```json
{"urls":"magnet:?xt=urn:btih:...","category":"anime","savepath":"D:/downloads","paused":false}
{"hashes":["abc","def"],"delete_files":true}
{"filter":"stopped","limit":20}
```

## 三个实现要点

1. **会话靠 cookie**：登录拿到 `SID` 后由 cookie jar 自动带上。jar 挂在 `Agent` 上，
   所以 Agent 必须复用——每次新建 Agent 等于每次都要重登。
2. **403 自动重登**：qB 重启、会话过期都会 403。这里对 403 做一次「重登 + 重试」，
   业务侧不用自己管登录态；重试仍失败才报错（多半是白名单/权限）。
3. **只做通道，不做业务判断**：`progress` 是 0~1 的小数、`state` 是 qB 的字符串枚举，
   原样透传。番剧算不算下载完、该不该删，由业务插件决定。

## 只适配 qBittorrent 5.0+

按 5.0 的接口写，**不做 4.x 回退**。三处改动都对着官方源码核对过
（`src/webui/api/torrentscontroller.cpp`、`src/base/torrentfilter.cpp`，本机 v5.1.2.10 / WebAPI 2.11.4 实测）：

| 项 | 4.x | 5.0+ | 这里怎么处理 |
| --- | --- | --- | --- |
| 暂停 / 继续 | `torrents/pause`、`torrents/resume`（在 5.x 上是 404） | `torrents/stop`、`torrents/start` | 只打新端点 |
| 添加后不启动 | `add` 的 `paused` | `add` 的 `stopped` | 只发 `stopped` |
| 列表过滤 | `filter=paused` / `resumed` | `filter=stopped` / `running` | 旧名自动映射 |
| 种子状态 | `pausedDL` / `pausedUP` | `stoppedDL` / `stoppedUP` | 原样透传，业务侧按 5.x 判断 |

两个必须自己兜住的坑：

1. **`add` 传不认识的参数不会报错**。5.0 只读 `stopped`，如果还发 `paused`，它不会拒绝，
   而是照常开始下载——「添加但不下载」会变成真的去下载几十 GB。所以只发 `stopped`，
   并且 `add` 前先校验版本。
2. **`filter` 传不认识的取值不会报错**。qB 的 `setTypeByName` 默认成 `All`，
   于是「查暂停的任务」会静默返回全部任务。所以本地按白名单校验，只给上游放行合法值。

低于 5.0 的服务器：读接口照常可用，`add` / `pause` / `resume` 会返回明确的错误说明
（而不是 404 或静默失效），`ping` 里的 `supported` 为 `false`。

写接口成功时 qB 返回空体或 `Ok.`。返回 `Fails.` 说明被拒（重复种子、保存路径无权限、
分类不存在），会被转成明确的错误信息，而不是静默成功。

## 联调

```bash
# 只读（版本/传输状态/列表/文件/属性）
cargo run -p fuyubot-plugin-qb --example live

# 连写接口一起测：临时添加一个暂停磁力 → pause/resume → 删除
QB_WRITE_TEST=1 cargo run -p fuyubot-plugin-qb --example live
```

账号密码从环境变量读（`QB_URL` / `QB_USER` / `QB_PASS`，默认 `admin` / `adminadmin`）。
写测试**无论中间哪步失败都会执行删除**，不会在你任务列表里留下东西。
注意 `add` 里的 `category` 会让 qB **自动建分类**，测试用的分类需要自己清。

示例会自己断言这几件事，任何一条不满足就以非 0 退出：

- 服务器版本 ≥ 5.0（否则写接口按 5.x 实现会走错语义）；
- `filter=paused`（4.x 旧名）能映射成 `stopped`，`filter=finished` 必须被拒绝；
- `add` 后 `state` 是 `stoppedDL`——即 `stopped` 参数真的生效了、没有偷偷开始下载；
- `pause` / `resume` 实际打到 `/torrents/stop`、`/torrents/start`。

最近一次实测（v5.1.2.10 / WebAPI 2.11.4）：

```
✓ ping: {"api_version":"2.11.4","supported":true,"version":"v5.1.2.10"}
✓ filter=stopped: []
✓ filter=paused（4.x 旧名 → stopped）: []
✓ 未知 filter 被拒绝：未知 filter：finished（qB 5.0 支持 all / downloading / …）
✓ add: {"accepted":true}
✓ 种子已入库：state=stoppedDL progress=0
✓ pause: /api/v2/torrents/stop
✓ resume: /api/v2/torrents/start
✓ 清理完成：临时种子已移除
```
