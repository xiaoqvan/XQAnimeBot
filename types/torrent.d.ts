export type Torrent = {
  /** 种子 id */
  id: string;
  /** 种子名称 */
  name: string;
  /** 状态描述信息 */
  stateMessage: string;
  /** 当前状态（如 downloading, seeding 等） */
  state: string;
  /** 预计剩余时间（秒） */
  eta: number;
  /** 添加时间（字符串） */
  dateAdded: string;
  /** 是否已完成 */
  isCompleted: boolean;
  /** 下载进度（0-1） */
  progress: number;
  /** 标签/分类 */
  label: string;
  /** 标签列表 */
  tags: string[];
  /** 完成时间（字符串） */
  dateCompleted: string;
  /** 保存路径 */
  savePath: string;
  /** 上传速度（字节/秒） */
  uploadSpeed: number;
  /** 下载速度（字节/秒） */
  downloadSpeed: number;
  /** 队列位置 */
  queuePosition: number;
  /** 当前连接的对等点数量 */
  connectedPeers: number;
  /** 当前连接的种子数量 */
  connectedSeeds: number;
  /** 总对等点数 */
  totalPeers: number;
  /** 总种子数 */
  totalSeeds: number;
  /** 选中总数（一般为选中文件的数量或大小） */
  totalSelected: number;
  /** 总大小（字节） */
  totalSize: number;
  /** 总上传量（字节） */
  totalUploaded: number;
  /** 总下载量（字节） */
  totalDownloaded: number;
  /** 上传/下载比 */
  ratio: number;
  /** 原始种子对象（来自 qBittorrent API） */
  raw: TorrentRaw;
};
export type TorrentRaw = {
  /** 添加时间（UNIX 时间戳） */
  added_on: number;
  /** 剩余字节数 */
  amount_left: number;
  /** 是否启用自动移动到默认目录 */
  auto_tmm: boolean;
  /** 可用性 */
  availability: number;
  /** 类别 */
  category: string;
  /** 注释/备注 */
  comment: string;
  /** 已完成的字节数 */
  completed: number;
  /** 完成时间（UNIX 时间戳） */
  completion_on: number;
  /** 内容路径 */
  content_path: string;
  /** 下载限速（KB/s 或 API 返回值） */
  dl_limit: number;
  /** 当前下载速度（字节/秒） */
  dlspeed: number;
  /** 下载保存路径 */
  download_path: string;
  /** 已下载总字节数 */
  downloaded: number;
  /** 本次会话已下载字节数 */
  downloaded_session: number;
  /** 预计剩余时间（秒） */
  eta: number;
  /** 首位片优先标志 */
  f_l_piece_prio: boolean;
  /** 强制开始标志 */
  force_start: boolean;
  /** 是否有元数据 */
  has_metadata: boolean;
  /** 种子 hash */
  hash: string;
  /** 非活跃做种时间限制 */
  inactive_seeding_time_limit: number;
  /** infohash v1 */
  infohash_v1: string;
  /** infohash v2 */
  infohash_v2: string;
  /** 上次活动时间（UNIX 时间戳） */
  last_activity: number;
  /** 磁力链接 */
  magnet_uri: string;
  /** 最大非活跃做种时间 */
  max_inactive_seeding_time: number;
  /** 最大比例 */
  max_ratio: number;
  /** 最大做种时间 */
  max_seeding_time: number;
  /** 种子名称 */
  name: string;
  /** 完成的客户端数量 */
  num_complete: number;
  /** 未完成的客户端数量 */
  num_incomplete: number;
  /** leech 数 */
  num_leechs: number;
  /** seeds 数 */
  num_seeds: number;
  /** 热度/人气 */
  popularity: number;
  /** 优先级 */
  priority: number;
  /** 是否为私有种子 */
  private: boolean;
  /** 进度（0-1） */
  progress: number;
  /** 上传/下载比 */
  ratio: number;
  /** 比例限制 */
  ratio_limit: number;
  /** 重新公告时间 */
  reannounce: number;
  /** 根路径 */
  root_path: string;
  /** 保存路径 */
  save_path: string;
  /** 做种时间（秒） */
  seeding_time: number;
  /** 做种时间限制（秒） */
  seeding_time_limit: number;
  /** 见证完成的次数 */
  seen_complete: number;
  /** 顺序下载标志 */
  seq_dl: boolean;
  /** 文件大小（字节） */
  size: number;
  /** 状态 */
  state: string;
  /** 超级做种标志 */
  super_seeding: boolean;
  /** 标签（逗号分隔的字符串） */
  tags: string;
  /** 活动时间（秒） */
  time_active: number;
  /** 总大小（字节） */
  total_size: number;
  /** 跟踪器列表（字符串） */
  tracker: string;
  /** 跟踪器数量 */
  trackers_count: number;
  /** 上传限速 */
  up_limit: number;
  /** 已上传字节数 */
  uploaded: number;
  /** 本次会话已上传字节数 */
  uploaded_session: number;
  /** 上传速度（字节/秒） */
  upspeed: number;
};
