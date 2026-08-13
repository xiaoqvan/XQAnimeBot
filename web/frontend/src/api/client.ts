// 前后端分离的 API 客户端：可配置后端地址与密钥（本地持久化）。
// 开发时默认 /api（Vite 代理）；通过登录页或 URL 参数（?api=&key=）可指定远端。

const TOKEN_KEY = "xq_anime_web_token";
const API_BASE_KEY = "xq_anime_web_api";

export function getToken(): string {
    return localStorage.getItem(TOKEN_KEY) ?? "";
}

export function setToken(token: string): void {
    if (token) {
        localStorage.setItem(TOKEN_KEY, token);
    } else {
        localStorage.removeItem(TOKEN_KEY);
    }
}

export function getApiBase(): string {
    return localStorage.getItem(API_BASE_KEY) ?? "/api";
}

export function setApiBase(base: string): void {
    // 归一根地址：去掉尾部斜杠与 /api 段（请求时统一拼 /api/...）
    let cleaned = (base || "").trim().replace(/\/+$/, "");
    cleaned = cleaned.replace(/\/api$/, "");
    if (cleaned) {
        localStorage.setItem(API_BASE_KEY, cleaned);
    } else {
        localStorage.removeItem(API_BASE_KEY);
    }
}

/** 保存完整的连接配置（地址 + 密钥） */
export function configureConnection(apiBase: string, token: string): void {
    setApiBase(apiBase);
    setToken(token);
}

/** 清除连接配置（登出） */
export function clearConnection(): void {
    localStorage.removeItem(TOKEN_KEY);
    localStorage.removeItem(API_BASE_KEY);
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
    const headers: Record<string, string> = {
        "Content-Type": "application/json",
        ...(init?.headers as Record<string, string> | undefined),
    };

    const token = getToken();
    if (token) {
        headers.Authorization = `Bearer ${token}`;
    }

    const base = getApiBase();
    const url = `${base}${path}`;

    let res: Response;
    try {
        res = await fetch(url, { ...init, headers });
    } catch {
        throw new ApiError(0, "无法连接到后端，请检查 API 地址或网络");
    }

    // 401 说明密钥失效或缺失
    if (res.status === 401) {
        throw new ApiError(401, "未授权：密钥无效或已过期，请重新获取连接密钥");
    }

    if (!res.ok) {
        let message = `请求失败（${res.status}）`;
        try {
            const data = await res.json();
            if (data && typeof data.error === "string") {
                message = data.error;
            }
        } catch {
            // ignore parse error
        }
        throw new ApiError(res.status, message);
    }

    return (await res.json()) as T;
}

export class ApiError extends Error {
    constructor(
        public status: number,
        message: string
    ) {
        super(message);
        this.name = "ApiError";
    }
}

// ---- 轻量缓存层（sessionStorage）----
// 解决"每次刷新都闪加载中"：刷新/重挂载时先用上次缓存立即渲染，再后台刷新。
// 用 sessionStorage 可在整页刷新后仍保留（关闭标签页自动清除）。
const CACHE_PREFIX = "xq_web_cache_";

export function readCache<T>(key: string): T | null {
    try {
        const raw = sessionStorage.getItem(CACHE_PREFIX + key);
        return raw ? (JSON.parse(raw) as T) : null;
    } catch {
        return null;
    }
}

export function writeCache<T>(key: string, data: T): void {
    try {
        sessionStorage.setItem(CACHE_PREFIX + key, JSON.stringify(data));
    } catch {
        // 容量不足或隐私模式等，忽略
    }
}

export function clearCache(prefix = ""): void {
    try {
        const drop = CACHE_PREFIX + prefix;
        const keys: string[] = [];
        for (let i = 0; i < sessionStorage.length; i++) {
            const k = sessionStorage.key(i);
            if (k && k.startsWith(drop)) keys.push(k);
        }
        keys.forEach((k) => sessionStorage.removeItem(k));
    } catch {
        // ignore
    }
}

// ---- 类型定义 ----

export interface Stats {
    animeCount: number;
    cacheAnimeCount: number;
    torrentCount: number;
    episodeCount: number;
    processing: { active: number; queued: number };
    time: string;
}

export interface ProgressItem {
    title: string;
    animeName?: string;
    stage: string;
    torrentHash?: string;
    startTime: string;
    updatedAt: string;
    qb?: { state: string; label: string } | null;
}

export interface ProgressData {
    activeCount: number;
    queueSize: number;
    delay: {
        intervalMs: number;
        waitMs: number;
        waitEnd: string;
        currentHourInBeijing: number;
        nextChangeHourInBeijing: number;
    };
    items: ProgressItem[];
}

export interface AnimeItem {
    id: number;
    name: string;
    name_cn?: string;
    names?: string[];
    image?: string;
    summary?: string;
    tags?: string[];
    episode?: string;
    score?: number | string;
    r18?: boolean;
    airingDay?: string;
    airingStart?: string;
    updatedAt?: string;
    eps?: { total: number; list: unknown[] };
    resources?: Record<string, unknown[]>;
}

export interface AnimeDetail {
    anime: AnimeItem | null;
}

export interface AnimePage {
    items: AnimeItem[];
    total: number;
    page: number;
    pageSize: number;
}

export interface SeasonItem {
    key: string;
    label: string;
    year: number;
    season: number;
    count: number;
}

export interface TorrentItem {
    hash: string;
    name: string;
    state: string;
    stateLabel: string;
    progress: number;
    size: number;
    sizeLabel: string;
    downloaded: number;
    uploaded: number;
    dlspeed: number;
    upspeed: number;
    dlspeedLabel: string;
    upspeedLabel: string;
    ratio: number;
    num_leechs: number;
    num_seeds: number;
    eta: number;
    save_path: string;
    content_path: string;
    tags: string[];
    added_on: number;
    completion_on: number;
    magnet_uri: string;
}

export interface TorrentsResponse {
    items: TorrentItem[];
}

export interface TransferInfo {
    dl_info_speed?: number;
    up_info_speed?: number;
    dlSpeedLabel?: string;
    upSpeedLabel?: string;
    dl_info_data?: number;
    up_info_data?: number;
    [key: string]: unknown;
}

export interface ReviewItem {
    id: number;
    createdAt: string;
    status: "pending" | "approved" | "rejected";
    animeName: string;
    animeId: number;
    title: string;
    episodeSort: number;
    episodeId: number;
    matchConfidence?: number;
    matchReason?: string;
    flow?: "pre_post" | "pre_review";
    flowLabel?: string;
    image?: string;
    episode?: string;
}

export interface AiCallItem {
    id?: unknown;
    scene: string;
    sceneLabel: string;
    input: string;
    output?: string;
    success: boolean;
    model?: string;
    durationMs?: number;
    meta?: string;
    createdAt: string;
}

export interface AiCallPage {
    items: AiCallItem[];
    total: number;
    page: number;
    pageSize: number;
}

export type BtTaskType = "addanime" | "addnewanime";
export type BtTaskStatus = "queued" | "running" | "done" | "failed" | "canceled";

export interface BtTask {
    id: number;
    type: BtTaskType;
    epid: number | string;
    url: string;
    status: BtTaskStatus;
    title: string;
    animeName: string;
    createdAt: string;
    updatedAt: string;
    startTime?: string;
    stage?: string;
    error?: string;
}

// ---- API 方法 ----

export const api = {
    health: () => request<{ status: string }>("/api/health"),
    // 验证当前连接配置是否有效（受鉴权保护，能返回即密钥有效）
    me: () =>
        request<{ ok: boolean; isDynamicKey?: boolean; expiresAt?: string }>("/api/auth/me"),
    stats: () =>
        request<Stats>("/api/stats").then((d) => {
            writeCache("stats", d);
            return d;
        }),

    // 动漫库
    listAnime: (page = 1, pageSize = 30, season = "all") =>
        request<AnimePage>(`/api/anime?page=${page}&pageSize=${pageSize}&season=${season}`).then(
            (d) => {
                writeCache(`anime:${season}:${page}:${pageSize}`, d);
                return d;
            }
        ),

    listAnimeSeasons: () =>
        request<{ items: SeasonItem[] }>("/api/anime/seasons").then((d) => {
            writeCache("seasons", d);
            return d;
        }),

    addAnime: (payload: {
        subjectId?: number | string;
        name?: string;
        name_cn?: string;
        image?: string;
        summary?: string;
        tags?: string[];
        episode?: string;
        score?: number | string;
        r18?: boolean;
        airingDay?: string;
        airingStart?: string;
    }) =>
        request<{ ok: boolean; id: number; anime?: AnimeItem }>("/api/anime", {
            method: "POST",
            body: JSON.stringify(payload),
        }).then((d) => {
            clearCache("anime:");
            clearCache("seasons");
            clearCache("stats");
            return d;
        }),

    searchAnime: (q: string) =>
        request<{ items: AnimeItem[]; total: number }>(
            `/api/anime/search?q=${encodeURIComponent(q)}`
        ),

    getAnime: (id: number | string) =>
        request<AnimeDetail>(`/api/anime/${id}`).then((d) => {
            writeCache(`anime-detail:${id}`, d);
            return d;
        }),

    deleteAnime: (id: number | string) =>
        request<{ ok: boolean; id: number }>(`/api/anime/${id}`, { method: "DELETE" }).then(
            (d) => {
                clearCache("anime:");
                clearCache("anime-detail:");
                clearCache("seasons");
                clearCache("stats");
                return d;
            }
        ),

    // BT 任务（addanime / addnewanime 映射）
    createTask: (type: BtTaskType, epid: number | string, url: string) =>
        request<{ ok: boolean; id: number }>(`/api/tasks/${type}`, {
            method: "POST",
            body: JSON.stringify({ epid, url }),
        }).then((d) => {
            clearCache("tasks");
            clearCache("progress");
            return d;
        }),
    listTasks: () =>
        request<{ items: BtTask[] }>("/api/tasks").then((d) => {
            writeCache("tasks", d);
            return d;
        }),
    getTask: (id: number) => request<{ task: BtTask }>(`/api/tasks/${id}`),
    cancelTask: (id: number) =>
        request<{ ok: boolean; id: number }>(`/api/tasks/${id}/cancel`, { method: "POST" }),
    clearTasks: () =>
        request<{ ok: boolean }>("/api/tasks/clear", { method: "POST" }).then((d) => {
            clearCache("tasks");
            return d;
        }),

    // 处理进度
    getProgress: () =>
        request<ProgressData>("/api/progress").then((d) => {
            writeCache("progress", d);
            return d;
        }),

    cancelProgress: (title: string) =>
        request<{ ok: boolean; message: string }>("/api/progress/cancel", {
            method: "POST",
            body: JSON.stringify({ title }),
        }),

    cancelAllProgress: () =>
        request<{ results: Array<{ ok: boolean; message: string }> }>(
            "/api/progress/cancel-all",
            { method: "POST" }
        ),

    // BT 下载
    getTorrents: () =>
        request<TorrentsResponse>("/api/torrents").then((d) => {
            writeCache("torrents", d);
            return d;
        }),
    getTransfer: () =>
        request<TransferInfo>("/api/torrents/transfer").then((d) => {
            writeCache("transfer", d);
            return d;
        }),
    pauseTorrent: (hash: string) =>
        request<{ ok: boolean }>(`/api/torrents/${hash}/pause`, { method: "POST" }).then((d) => {
            clearCache("torrents");
            return d;
        }),
    resumeTorrent: (hash: string) =>
        request<{ ok: boolean }>(`/api/torrents/${hash}/resume`, { method: "POST" }).then((d) => {
            clearCache("torrents");
            return d;
        }),
    deleteTorrent: (hash: string, deleteFiles = true) =>
        request<{ ok: boolean }>(`/api/torrents/${hash}/delete`, {
            method: "POST",
            body: JSON.stringify({ deleteFiles }),
        }).then((d) => {
            clearCache("torrents");
            clearCache("transfer");
            return d;
        }),

    // 待确认番剧
    listReviews: (flow: "pre_post" | "pre_review" = "pre_post", status = "pending", page = 1, pageSize = 30) =>
        request<{ items: ReviewItem[]; total?: number; page?: number; pageSize?: number; flow?: string }>(
            `/api/reviews?flow=${flow}&status=${status}&page=${page}&pageSize=${pageSize}`
        ).then((d) => {
            writeCache(`reviews:${flow}:${status}:${page}:${pageSize}`, d);
            return d;
        }),
    approveReview: (id: number) =>
        request<{ ok: boolean }>(`/api/reviews/${id}/approve`, { method: "POST" }).then((d) => {
            clearCache("reviews:");
            return d;
        }),
    rejectReview: (id: number, remove = false) =>
        request<{ ok: boolean }>(`/api/reviews/${id}/reject`, {
            method: "POST",
            body: JSON.stringify({ remove }),
        }).then((d) => {
            clearCache("reviews:");
            return d;
        }),

    // AI 调用记录
    listAiCalls: (page = 1, pageSize = 20, scene?: string) =>
        request<AiCallPage>(
            `/api/ai-calls?page=${page}&pageSize=${pageSize}${scene ? `&scene=${scene}` : ""}`
        ).then((d) => {
            writeCache(`ai:${page}:${pageSize}:${scene ?? "all"}`, d);
            return d;
        }),
};
