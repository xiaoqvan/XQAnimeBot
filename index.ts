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
      anime(this.client);
    };

    this.cmdHandlers = {
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
      updateanime: {
        description: "更新指定ID动漫的信息",
        scope: "private",
        permission: "admin",
        handler: async (message, commandParts) => {
          const mod = await import("./cmd/updateAnime.ts");
          return mod.default(this.client, message.message, commandParts);
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
