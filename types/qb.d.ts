/**
 * qBittorrent WebUI API 类型定义
 */

/**
 * Torrent 状态枚举
 */
export enum TorrentState {
    Error = 'error',
    MissingFiles = 'missingFiles',
    Uploading = 'uploading',
    PausedUP = 'pausedUP',
    QueuedUP = 'queuedUP',
    StalledUP = 'stalledUP',
    CheckingUP = 'checkingUP',
    ForcedUP = 'forcedUP',
    Allocating = 'allocating',
    Downloading = 'downloading',
    MetaDL = 'metaDL',
    PausedDL = 'pausedDL',
    QueuedDL = 'queuedDL',
    StalledDL = 'stalledDL',
    CheckingDL = 'checkingDL',
    ForcedDL = 'forcedDL',
    CheckingResumeData = 'checkingResumeData',
    Moving = 'moving',
    Unknown = 'unknown'
}

/**
 * Torrent 信息接口
 */
export interface TorrentInfo {
    /** 添加时间戳 */
    added_on: number;
    /** 剩余大小（字节） */
    amount_left: number;
    /** 自动种子管理 */
    auto_tmm: boolean;
    /** 可用性 */
    availability: number;
    /** 分类 */
    category: string;
    /** 已完成大小（字节） */
    completed: number;
    /** 完成时间戳 */
    completion_on: number;
    /** 内容路径 */
    content_path: string;
    /** 下载限速（字节/秒） */
    dl_limit: number;
    /** 下载速度（字节/秒） */
    dlspeed?: number;
    /** 备用下载速度字段名（有些版本/接口返回该字段） */
    dl_speed?: number;
    /** 已下载大小（字节） */
    downloaded: number;
    /** 本次会话已下载大小（字节） */
    downloaded_session: number;
    /** 预计剩余时间（秒） */
    eta: number;
    /** 首尾文件优先 */
    f_l_piece_prio: boolean;
    /** 强制开始 */
    force_start: boolean;
    /** 是否有 metadata */
    has_metadata?: boolean;
    /** 种子哈希值 */
    hash: string;
    /** v1 infohash（通常等于 hash） */
    infohash_v1?: string;
    /** v2 infohash（若支持） */
    infohash_v2?: string;
    /** 最后活动时间戳 */
    last_activity: number;
    /** 磁力链接 */
    magnet_uri: string;
    /** 最大分享率 */
    max_ratio: number;
    /** 最大做种时间（秒） */
    max_seeding_time: number;
    /** 最大不活跃做种时间（秒） */
    max_inactive_seeding_time?: number;
    /** 不活跃做种时间限制（秒） */
    inactive_seeding_time_limit?: number;
    /** 种子名称 */
    name: string;
    /** 完整种子数 */
    num_complete: number;
    /** 不完整种子数 */
    num_incomplete: number;
    /** 下载者数量 */
    num_leechs: number;
    /** 做种者数量 */
    num_seeds: number;
    /** 热度/流行度 */
    popularity?: number;
    /** 优先级 */
    priority: number;
    /** 进度（0-1） */
    progress: number;
    /** 分享率 */
    ratio: number;
    /** 分享率限制 */
    ratio_limit: number;
    /** 重新汇报间隔（秒） */
    reannounce?: number;
    /** 保存路径 */
    save_path: string;
    /** 根路径（当存在时） */
    root_path?: string;
    /** 下载路径（有时返回） */
    download_path?: string;
    /** 做种时间（秒） */
    seeding_time: number;
    /** 做种时间限制（秒） */
    seeding_time_limit: number;
    /** 上次完整时间戳 */
    seen_complete: number;
    /** 顺序下载 */
    seq_dl: boolean;
    /** 大小（字节） */
    size: number;
    /** 状态 */
    state: TorrentState;
    /** 超级种子模式 */
    super_seeding: boolean;
    /** 标签 */
    tags: string[];
    /** 活动时间（秒） */
    time_active: number;
    /** 总大小（字节） */
    total_size: number;
    /** Tracker 地址 */
    tracker: string;
    /** Tracker 数量 */
    trackers_count?: number;
    /** 上传限速（字节/秒） */
    up_limit: number;
    /** 已上传大小（字节） */
    uploaded: number;
    /** 本次会话已上传大小（字节） */
    uploaded_session: number;
    /** 上传速度（字节/秒） */
    upspeed: number;
    /** 私有标志，可能为 null */
    private?: boolean | null;
}

/**
 * Torrent 属性接口
 */
export interface TorrentProperties {
    /** 添加日期时间戳 */
    addition_date: number;
    /** 备注 */
    comment: string;
    /** 完成日期时间戳 */
    completion_date: number;
    /** 创建者 */
    created_by: string;
    /** 创建日期时间戳 */
    creation_date: number;
    /** 下载限速（字节/秒） */
    dl_limit: number;
    /** 下载速度（字节/秒） */
    dl_speed: number;
    /** 平均下载速度（字节/秒） */
    dl_speed_avg: number;
    /** 预计剩余时间（秒） */
    eta: number;
    /** 最后可见时间戳 */
    last_seen: number;
    /** 当前连接数 */
    nb_connections: number;
    /** 最大连接数限制 */
    nb_connections_limit: number;
    /** Peer 数量 */
    peers: number;
    /** 总 Peer 数量 */
    peers_total: number;
    /** 分块大小（字节） */
    piece_size: number;
    /** 已有分块数 */
    pieces_have: number;
    /** 总分块数 */
    pieces_num: number;
    /** 重新汇报时间（秒） */
    reannounce: number;
    /** 保存路径 */
    save_path: string;
    /** 做种时间（秒） */
    seeding_time: number;
    /** 做种者数量 */
    seeds: number;
    /** 总做种者数量 */
    seeds_total: number;
    /** 分享率 */
    share_ratio: number;
    /** 已用时间（秒） */
    time_elapsed: number;
    /** 总下载量（字节） */
    total_downloaded: number;
    /** 本次会话总下载量（字节） */
    total_downloaded_session: number;
    /** 总大小（字节） */
    total_size: number;
    /** 总上传量（字节） */
    total_uploaded: number;
    /** 本次会话总上传量（字节） */
    total_uploaded_session: number;
    /** 浪费流量（字节） */
    total_wasted: number;
    /** 上传限速（字节/秒） */
    up_limit: number;
    /** 上传速度（字节/秒） */
    up_speed: number;
    /** 平均上传速度（字节/秒） */
    up_speed_avg: number;
    /** 下载路径（有时返回） */
    download_path?: string;
    /** 是否有 metadata */
    has_metadata?: boolean;
    /** 种子哈希 */
    hash?: string;
    /** v1 infohash（通常等于 hash） */
    infohash_v1?: string;
    /** v2 infohash（若支持） */
    infohash_v2?: string;
    /** 私有标志（布尔） */
    is_private?: boolean;
    /** 人气/热度 */
    popularity?: number;
    /** 私有标志，可能为 null */
    private?: boolean | null;
    /** 进度（0-1） */
    progress?: number;
    /** 种子名称 */
    name?: string;
}

/**
 * 传输信息接口
 */
export interface TransferInfo {
    /** 连接状态 */
    connection_status: string;
    /** DHT 节点数 */
    dht_nodes: number;
    /** 下载数据总量（字节） */
    dl_info_data: number;
    /** 下载速度（字节/秒） */
    dl_info_speed: number;
    /** 下载速率限制（字节/秒） */
    dl_rate_limit: number;
    /** 上传数据总量（字节） */
    up_info_data: number;
    /** 上传速度（字节/秒） */
    up_info_speed: number;
    /** 上传速率限制（字节/秒） */
    up_rate_limit: number;
    /** 当前可用磁盘空间（字节） */
    free_space_on_disk?: number;
}

/**
 * 获取 Torrent 列表选项
 */
export interface GetTorrentsOptions {
    /** 过滤器 */
    filter?: 'all' | 'downloading' | 'seeding' | 'completed' | 'paused' | 'active' | 'inactive' | 'resumed' | 'stalled' | 'stalled_uploading' | 'stalled_downloading';
    /** 分类 */
    category?: string;
    /** 标签 */
    tag?: string[];
    /** 排序字段 */
    sort?: string;
    /** 反向排序 */
    reverse?: boolean;
    /** 返回数量限制 */
    limit?: number;
    /** 偏移量 */
    offset?: number;
    /** 种子哈希值（多个用|分隔） */
    hashes?: string;
}

/**
 * 添加 Torrent 选项
 */
export interface AddTorrentOptions {
    /** 保存路径 */
    savepath?: string;
    /** Cookie */
    cookie?: string;
    /** 分类 */
    category?: string;
    /** 标签 */
    tags?: string[];
    /** 跳过校验 */
    skip_checking?: boolean;
    /** 暂停状态添加 */
    paused?: boolean;
    /** 创建根目录 */
    root_folder?: boolean;
    /** 重命名 */
    rename?: string;
    /** 上传限速（字节/秒） */
    upLimit?: number;
    /** 下载限速（字节/秒） */
    dlLimit?: number;
    /** 分享率限制 */
    ratioLimit?: number;
    /** 做种时间限制（分钟） */
    seedingTimeLimit?: number;
    /** 自动种子管理 */
    autoTMM?: boolean;
    /** 顺序下载 */
    sequentialDownload?: boolean;
    /** 首尾文件优先下载 */
    firstLastPiecePrio?: boolean;
}

/**
 * 客户端配置接口
 */
export interface QbittorrentClientConfig {
    /** qBittorrent WebUI 地址 */
    baseURL: string;
    /** 用户名 */
    username?: string;
    /** 密码 */
    password?: string;
    /** 自动登录（403 时自动重新登录） */
    autoLogin?: boolean;
}
