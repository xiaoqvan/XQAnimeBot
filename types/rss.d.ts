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
    /** 动漫链接 */
    link: string;
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

