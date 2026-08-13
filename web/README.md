# XQ Anime Web 管理界面

为 XQAnimeBot 提供的 Web 管理界面，**前后端分离**：后端使用 Fastify 提供 REST API，
前端使用 Vue3 + Vite（**独立运行/部署，Bot 不托管、不运行前端**）。

## 目录结构

```
web/
├── config.ts          # Web 服务配置（端口 / 主机 / Token / 跨域）
├── auth.ts            # Bearer Token 鉴权
├── server.ts          # Fastify API 服务启动/关闭（不托管前端）
├── routes/
│   ├── index.ts       # 路由注册入口
│   ├── anime.ts       # 番剧 分页列表 / 添加 / 搜索 / 详情
│   ├── torrents.ts    # BT 下载 列表 / 暂停 / 恢复 / 删除
│   ├── reviews.ts     # 待确认番剧 列表 / 确认 / 拒绝
│   ├── ai.ts          # AI 调用记录
│   ├── progress.ts    # 处理进度 / 取消任务
│   └── stats.ts       # 健康检查 / 概览统计
└── frontend/          # Vue3 + Vite 前端工程
    └── src/
        ├── api/client.ts   # 统一 API 客户端与类型
        ├── router.ts
        ├── App.vue
        └── views/          # Dashboard / AnimeList / AnimeDetail / Progress / Torrents / Reviews / AiLog

> AI 调用记录：插件内 3 处 LLM 调用（番剧匹配、集数匹配、集数提取）会自动写入
> MongoDB `ai_calls` 集合（见 `database/ai.ts`），Web 仅作展示。
```

## 配置（环境变量）

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `WEB_HOST` | `127.0.0.1` | 监听地址；如需局域网访问改为 `0.0.0.0` |
| `WEB_PORT` | `3780` | 监听端口 |
| `WEB_API_TOKEN` | 空 | 可选：不过期的主令牌（兼容旧鉴权） |
| `WEB_KEY_SECRET` | 自动 | 密钥签名密钥；不设则持久化到 `web/.web_secret` |

Web 服务随 XQAnimeBot 插件启动时自动拉起（见插件 `index.ts` 的 `onLoad`/`destroy`）。

## 安装依赖

```bash
# 项目根目录（pnpm workspace）
pnpm install

# 前端依赖
cd plugins/XQAnimeBot/web/frontend && pnpm install
```

## 前端开发模式（独立运行）

前端为独立工程，需单独运行/部署；Bot 后端只提供 API，不托管、不运行前端。

```bash
cd plugins/XQAnimeBot/web/frontend
pnpm dev
```
打开 http://localhost:5173 即可。登录页输入 Bot `/web` 命令给出的**后端 API 地址与密钥**即可连接。

生产部署前端时用：

```bash
cd plugins/XQAnimeBot/web/frontend
pnpm build
```
构建产物生成到 `frontend/dist`，部署到任意静态服务器即可（SPA 路由使用 hash 模式，无需额外配置）。

## 鉴权使用（动态密钥，24 小时有效）

所有 `/api/*`（除 `/api/health`）都需携带 `Authorization: Bearer <key>`。

- **获取方式**：在 Bot 中由主人/管理员发送 `/web`，Bot 返回**可复制的后端 API 地址 + 访问密钥**文本；在前端登录页填入这两项即可连接。
- **密钥机制**：HMAC 签名密钥（自含过期时间，`web/key.ts`），**有效期 24 小时**，
  过期后需重新 `/web` 获取。签名密钥持久化于 `web/.web_secret`（重启后旧 key 仍有效）。
- **主令牌**：可选配置 `WEB_API_TOKEN` 作为不过期的主令牌（兼容旧鉴权）。
- **手动连接**：独立打开前端时，登录页输入后端地址与密钥即可（自动校验 `/api/auth/me`）。

> 前端为前后端分离：前端页面可独立部署，后端 API 服务启动时会在控制台打印 API 地址。
> CORS 已默认开启（允许跨域 + Authorization 头），密钥鉴权保护 API 安全。

## API 一览

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/health` | 健康检查 |
| GET | `/api/stats` | 概览统计（番剧/章节/种子/处理状态） |
| GET | `/api/anime?page=&pageSize=&season=` | 番剧分页（封面浏览）；`season` 可为 `all`、`unknown` 或形如 `2024-1`（1春）的年季 key |
| GET | `/api/anime/seasons` | 年季分类统计（按年份+季度分组的番剧数） |
| POST | `/api/anime` | 添加番剧（body: `{subjectId}` 或手动字段） |
| GET | `/api/anime/search?q=关键词` | 搜索番剧（≥2 字符） |
| GET | `/api/anime/:id` | 番剧详情（含章节与资源） |
| DELETE | `/api/anime/:id` | 删除番剧（连同章节/资源/待审核记录；BT 去重记录保留） |
| GET | `/api/torrents` | BT 下载列表 |
| GET | `/api/torrents/transfer` | BT 全局传输信息 |
| POST | `/api/torrents/:hash/pause` | 暂停种子 |
| POST | `/api/torrents/:hash/resume` | 恢复种子 |
| POST | `/api/torrents/:hash/delete` | 删除种子（body: `{deleteFiles}` 默认 true） |
| GET | `/api/reviews?flow=&status=` | 待确认番剧列表。`flow=pre_post`（先发后审，来自 pendingReviews）或 `flow=pre_review`（先审核后发，来自 cacheAnime 缓存）；`status=pending/approved/rejected` |
| POST | `/api/reviews/:id/approve` | 确认番剧（标记 approved，仅先发后审流程） |
| POST | `/api/reviews/:id/reject` | 拒绝番剧（body: `{remove}` 可彻底移除） |
| POST | `/api/tasks/addanime` | 创建 addanime 任务（body: `{epid, url}`，为已有番剧添加 BT 信息） |
| POST | `/api/tasks/addnewanime` | 创建 addnewanime 任务（body: `{epid, url}`，添加新番剧并关联 BT） |
| GET | `/api/tasks` | BT 任务列表（含实时阶段进度） |
| GET | `/api/tasks/:id` | 单个任务详情 |
| POST | `/api/tasks/:id/cancel` | 取消任务 |
| POST | `/api/tasks/clear` | 清理已完成/失败/取消任务 |
| GET | `/api/ai-calls?page=&pageSize=&scene=` | AI 调用记录 |
| GET | `/api/progress` | 当前处理进度（含 qBittorrent 状态） |
| POST | `/api/progress/cancel` | 取消单个任务（body: `{title}`） |
| POST | `/api/progress/cancel-all` | 取消全部活跃任务 |
