export type RssAnimeItem = RssDmhyItem | RssAcgnxItem | RssBangumiItem;

export type RssDmhyItem = {
  /** 本条动漫信息来自的平台 */
  type: "dmhy";
  /** 动漫标题 */
  title: string;
  /** 动漫链接 */
  link: string;
  /** 发布者 */
  author: string;
  /** 动漫发布时间 */
  pubDate: string;
  /** 磁力链接 */
  magnet: string;
};

export type RssAcgnxItem = {
  /** 本条动漫信息来自的平台 */
  type: "acgnx";
  /** 动漫标题 */
  title: string;
  /** 动漫链接 */
  link: string;
  /** 发布者 */
  author: string;
  /** 动漫发布时间 */
  pubDate: string;
  /** 磁力链接 */
  magnet: string;
};

export type RssBangumiItem = {
  /** 本条动漫信息来自的平台 */
  type: "bangumi";
  /** bangumi 动漫 id */
  id: string | number;
  /** 动漫标题 */
  title: string;
  /** 动漫链接 */
  link: string;
  /** 动漫发布时间 */
  pubDate: string;
  /** torrent 文件链接 */
  torrent?: string;
};

export type animeItem = {
  /** 动漫标题 */
  title: string;
  /** 动漫发布时间 */
  pubDate: string;
  /** 磁力链接 */
  magnet: string;
  /** 动漫发布组 */
  team: string;
  /** 发布组列表 */
  fansub: string[];
  /** 标题中的番剧名称 */
  names: string[];
  /** 番剧来源 */
  source?: string;
  /** 当前集数 */
  episode?: string;
};

export type anime = {
  /** 动漫ID */
  id: number;
  /** 中文名 */
  name_cn?: string;
  /** 原名 */
  name: string;
  /** 其他名称 */
  names?: string[];
  /** 图片 */
  image: string;
  /** 简介 */
  summary?: string;
  /** 标签 */
  tags?: string[];
  /** 总集数 */
  episode?: string;
  /** 评分 */
  score?: number | string;
  /** 动漫在TG中的数据 */
  btdata?: BtData;
  /**
   * 导航频道消息链接
   * @deprecated 已被 tgMessage 取代
   */
  navMessageLink?: string;
  /** 新版导航频道消息 */
  navMessage?: messageType;
  /** 多条资源导航消息 */
  navVideoMessage?: {
    /** 索引，从1开始，navMessage为主消息 */
    page: number;
    /** 消息所属的聊天 ID */
    chat_id: number;
    /** 消息 ID */
    message_id: number;
    /** 线程 ID */
    thread_id?: number;
    /** 消息链接 */
    link: string;
  }[];
  /** 是否为R18 */
  r18?: boolean;
  /** 放送星期 */
  airingDay?: string;
  /** 放送开始时间 */
  airingStart?: string;
  /** 数据库中创建时间 */
  createdAt?: Date;
  /** 数据库中最后更新时间 */
  updatedAt?: Date;
};

// 新增：单条 bt 条目的类型
export type BtEntry = {
  /** 动漫集数 */
  episode: string;
  /** TG 链接 */
  TGMegLink?: string;
  /** 新消息详细 */
  Message?: messageType;
  /** 标题 */
  title: string;
  /** 缓存数据库中的 ID */
  cache_id?: number | string;
  /** TG视频唯一 ID */
  videoid?: string;
  /** TG视频远程 ID */
  unique_id?: string;
};

// 新增：btdata 是以字符串键映射到 BtEntry 数组的对象
export type BtData = Record<string, BtEntry[]>;

export type bangumiAnime = {
  /** 条目 ID */
  id: number;
  /** 条目类型
   *  1 = 书籍, 2 = 动画, 3 = 音乐, 4 = 游戏, 6 = 三次元（没有 5）
   */
  type: 1 | 2 | 3 | 4 | 6;
  /** 原名 */
  name: string;
  /** 中文名 */
  name_cn?: string;
  /** 简介/剧情概述 */
  summary?: string;
  /** 放送日期 */
  date?: string;
  /** 平台，如 TV、Web 等 */
  platform?: string;
  /** 图片资源 */
  images?: {
    /** 小图 */
    small?: string;
    /** 网格图 */
    grid?: string;
    /** 大图 */
    large?: string;
    /** 中图 */
    medium?: string;
    /** 常用尺寸图 */
    common?: string;
  };
  /** 标签列表 */
  tags?: {
    /** 标签名 */
    name: string;
    /** 标签计数 */
    count?: number;
    /** 总计数（备用字段） */
    total_cont?: number;
  }[];
  /** 信息框，包含各种属性 */
  infobox?: infobox[];
  /** 评分信息 */
  rating?: {
    /** 排名 */
    rank?: number;
    /** 总评分人数 */
    total?: number;
    /** 各分数的人数统计 */
    count?: {
      [score: string]: number; // 1-10 的分数人数
    };
    /** 平均分 */
    score?: number;
  };
  /** bgm.tv中的章节数量 */
  total_episodes?: number;
  /** 收藏状态 */
  collection?: {
    /** 搁置 */
    on_hold?: number;
    /** 放弃 */
    dropped?: number;
    /** 想看 */
    wish?: number;
    /** 已收藏 */
    collect?: number;
    /** 正在看 */
    doing?: number;
  };

  /** 当前总集数 */
  eps?: number;
  /** 由维基人维护的 tags */
  meta_tags?: string[];
  /** 书籍条目的册数 */
  volumes?: number;
  /** 是否为书籍系列的主条目 */
  series?: boolean;
  /** 是否锁定 */
  locked?: boolean;
  /** 是否为成人向 */
  nsfw?: boolean;
};

export type bangumiSearchResult = {
  /** 搜索结果数组 */
  data?: bangumiAnime[];
  /** 总结果数 */
  total: number;
  /** 每页限制数 */
  limit: number;
  /** 偏移量 */
  offset: number;
};

export type infobox = {
  /** 属性名 */
  key: string;
  /** 属性值，可能是字符串或数组（数组项可为纯字符串、{v: string} 或 {k: string, v: string}） */
  value: string | (string | { v: string } | { k: string; v: string })[];
};

export type messageType = {
  /** 消息所属的聊天 ID */
  chat_id: number;
  /** 消息 ID */
  message_id: number;
  /** 线程 ID */
  thread_id?: number;
  /** 消息链接 */
  link: string;
};
