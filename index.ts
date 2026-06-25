import logger from "@log/index.ts";
import { Plugin } from "@plugin/BasePlugin.ts";
import type { Client } from "tdl";
import { anime } from "./anime/index.ts";

export default class AnimePlugin extends Plugin {
  name = "XQ的动漫插件";
  description = "提供与动漫相关的功能";
  type = "bot";
  version = "1.0.0";

  constructor(client: Client) {
    super(client);

    this.onLoad = async () => {
      logger.info("[XiaoQvanAnime]加载 完成开始获取动漫信息");
      anime(this.client).then();
    };

    this.cmdHandlers = {
      start: {
        description: "处理/start命令",
        scope: "private",
        handler: async (message, _) => {
          const mod = await import("./cmd/start.ts");
          return mod.default(this.client, message.message);
        }
      },
      bindbangumi: {
        description: "绑定Bangumi账户",
        scope: "private",
        handler: async (message, _) => {
          const mod = await import("./cmd/bindbangumi.ts");
          return mod.default(this.client, message.message);
        },
      },
      exitbangumi: {
        description: "退出Bangumi账户绑定",
        scope: "private",
        handler: async (message, _) => {
          const mod = await import("./cmd/exitbangumi.ts");
          return mod.default(this.client, message.message);
        },
      },
      searchanime: {
        description: "搜索频道内的动漫",
        scope: "all",
        handler: async (message, commandParts) => {
          const mod = await import("./cmd/searchanime.ts");
          return mod.default(this.client, message.message, commandParts);
        },
      },
      s: {
        description: "/searchanime命令的短命令",
        scope: "all",
        handler: async (message, commandParts) => {
          const mod = await import("./cmd/searchanime.ts");
          return mod.default(this.client, message.message, commandParts);
        },
      },
      setanimer18: {
        description: "设置动漫的r18字段",
        scope: "private",
        permission: "admin",
        handler: async (message, commandParts) => {
          const mod = await import("./cmd/setanimer18.ts");
          return mod.default(this.client, message.message, commandParts);
        },
      },
      ConAnimeInfo: {
        description: "纠正缓存动漫信息为数据库的内容",
        scope: "private",
        permission: "admin",
        handler: async (message, commandParts) => {
          const mod = await import("./cmd/jz.ts");
          return mod.default(this.client, message.message, commandParts);
        },
      },
      addanime: {
        description: "为指定ID的动漫添加BT信息",
        scope: "private",
        permission: "admin",
        handler: async (message, commandParts) => {
          const mod = await import("./cmd/addanime.ts");
          return mod.default(this.client, message.message, commandParts);
        },
      },
      addnewanime: {
        description: "添加新的动漫并关联BT信息",
        scope: "private",
        permission: "admin",
        handler: async (message, commandParts) => {
          const mod = await import("./cmd/addnewanime.ts");
          return mod.default(this.client, message.message, commandParts);
        },
      },
      updateanime: {
        description: "更新指定ID动漫的信息",
        scope: "private",
        permission: "admin",
        handler: async (message, commandParts) => {
          const mod = await import("./cmd/updateAnime.ts");
          return mod.default(this.client, message.message, commandParts);
        },
      },
      progress: {
        description: "查看当前动漫下载/处理进度",
        scope: "private",
        permission: "admin",
        handler: async (message, _) => {
          const mod = await import("./cmd/progress.ts");
          return mod.default(this.client, message.message);
        },
      },
      canceltask: {
        description: "取消当前堵塞任务并释放并发槽位",
        scope: "private",
        permission: "admin",
        handler: async (message, commandParts) => {
          const mod = await import("./cmd/canceltask.ts");
          return mod.default(this.client, message.message, commandParts);
        },
      },
      animeblacklist: {
        description: "管理动画RSS黑名单关键词",
        scope: "private",
        permission: "admin",
        handler: async (message, commandParts) => {
          const mod = await import("./cmd/animeblacklist.ts");
          return mod.default(this.client, message.message, commandParts);
        },
      },
    };

    this.inlineHandlers = {
      animeNavigation: {
        description: "内联搜索番剧导航消息（图片导航）",
        scope: "all",
        matcher: (ctx) => {
          const query = ctx.query.trim();
          if (!query) return false;
          if (/^\d+$/.test(query)) return 90;
          return query.length >= 2 ? 70 : false;
        },
        handler: async (ctx) => {
          const mod = await import("./inline/animeNavigation.ts");
          return mod.default(this.client, ctx);
        },
      },
      episodeVideo: {
        description: "内联按 EPID 或 bgm 链接发送对应视频",
        scope: "all",
        matcher: (ctx) => {
          const query = ctx.query.trim();
          if (/^\d+$/.test(query)) return 100;
          return /https?:\/\/(?:bgm\.tv|bangumi\.tv)\/ep\/\d+(?:\/|$)/i.test(query)
            ? 100
            : false;
        },
        handler: async (ctx) => {
          const mod = await import("./inline/episodeVideo.ts");
          return mod.default(this.client, ctx);
        },
      },
    };

    this.updateHandlers = {
      updateNewCallbackQuery: {
        handler: async (update) => {
          const mod = await import("./CallbackQuery/index.ts");
          return mod.default(this.client, update);
        },
      },
    };
  }
}
