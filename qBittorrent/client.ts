import axios from 'axios';
import type { AxiosInstance, AxiosRequestConfig, AxiosResponse, InternalAxiosRequestConfig } from 'axios';
import http from 'http';
import https from 'https';
import type {
    TorrentInfo,
    TorrentProperties,
    TransferInfo,
    GetTorrentsOptions,
    AddTorrentOptions,
    QbittorrentClientConfig,
} from '../types/qb.d.ts';


/**
 * 自定义 API 错误类
 */
export class ApiError extends Error {
    public statusCode?: number;
    public response?: unknown;

    constructor(
        message: string,
        statusCode?: number,
        response?: unknown
    ) {
        super(message);
        this.name = 'ApiError';
        this.statusCode = statusCode;
        this.response = response;
        Error.captureStackTrace(this, this.constructor);
    }
}

/**
 * qBittorrent WebUI API 客户端
 */
export class QbittorrentClient {
    private axios: AxiosInstance;
    private username?: string;
    private password?: string;
    private autoLogin: boolean;
    private loggedIn: boolean = false;
    private cookieJar: string[] = [];
    private httpAgent: http.Agent;
    private httpsAgent: https.Agent;

    /**
     * 创建 qBittorrent 客户端实例
     * @param config - 客户端配置
     */
    constructor(config: QbittorrentClientConfig) {
        const { baseURL, username, password, autoLogin = false } = config;

        this.username = username;
        this.password = password;
        this.autoLogin = autoLogin;

        this.httpAgent = new http.Agent({
            keepAlive: true,
            keepAliveMsecs: 30000,
            maxSockets: 50,
            maxFreeSockets: 10,
        });
        this.httpsAgent = new https.Agent({
            keepAlive: true,
            keepAliveMsecs: 30000,
            maxSockets: 50,
            maxFreeSockets: 10,
        });

        this.axios = axios.create({
            baseURL,
            timeout: 30000,
            headers: {
                'Content-Type': 'application/x-www-form-urlencoded',
            },
            withCredentials: true,
            httpAgent: this.httpAgent,
            httpsAgent: this.httpsAgent,
        });

        // 请求拦截器
        this.axios.interceptors.request.use(
            (config: InternalAxiosRequestConfig) => {
                // 添加 cookie
                if (this.cookieJar.length > 0) {
                    config.headers.Cookie = this.cookieJar.join('; ');
                }
                return config;
            },
            (error: unknown) => Promise.reject(error)
        );

        // 响应拦截器
        this.axios.interceptors.response.use(
            (response: AxiosResponse) => {
                // 保存 cookie
                const setCookie = response.headers['set-cookie'];
                if (setCookie) {
                    setCookie.forEach((cookie: string) => {
                        const cookieName = cookie.split('=')[0];
                        // 移除旧的同名 cookie
                        this.cookieJar = this.cookieJar.filter(
                            (c) => !c.startsWith(cookieName + '=')
                        );
                        // 添加新 cookie
                        this.cookieJar.push(cookie.split(';')[0]);
                    });
                }
                return response;
            },
            (error: unknown) => {
                if ((error as Record<string, unknown>).response && (error as { response: { status: number } }).response?.status === 403) {
                    this.loggedIn = false;
                    throw new ApiError('Forbidden: Not logged in or session expired', 403, (error as { response: { data: unknown } }).response?.data);
                }

                // 处理连接错误：EPIPE、ECONNRESET、ECONNABORTED 等
                const errorMessage = (error as Error).message || '';
                const errorCode = (error as { code?: string }).code || '';
                if (errorMessage.includes('EPIPE') ||
                    errorCode === 'EPIPE' ||
                    errorCode === 'ECONNRESET' ||
                    errorCode === 'ECONNABORTED') {
                    this.loggedIn = false;
                }

                throw new ApiError(
                    (error as Error).message || 'Request failed',
                    (error as { response?: { status?: number } }).response?.status,
                    (error as { response?: { data?: unknown } }).response?.data
                );
            }
        );
    }

    /**
     * 断开旧连接，重建连接池并重新认证
     */
    private async reconnect(): Promise<void> {
        // 销毁旧连接池，释放所有 keep-alive socket
        this.httpAgent.destroy();
        this.httpsAgent.destroy();

        // 重建连接池
        this.httpAgent = new http.Agent({
            keepAlive: true,
            keepAliveMsecs: 30000,
            maxSockets: 50,
            maxFreeSockets: 10,
        });
        this.httpsAgent = new https.Agent({
            keepAlive: true,
            keepAliveMsecs: 30000,
            maxSockets: 50,
            maxFreeSockets: 10,
        });

        // 更新 axios 实例使用新 Agent
        this.axios.defaults.httpAgent = this.httpAgent;
        this.axios.defaults.httpsAgent = this.httpsAgent;

        // 清除认证状态
        this.loggedIn = false;
        this.cookieJar = [];

        // 若启用自动登录，重新认证
        if (this.autoLogin && this.username && this.password) {
            await this.login();
        }
    }

    /**
     * 通用请求方法
     */
    private async request<T>(
        config: AxiosRequestConfig,
        signal?: AbortSignal
    ): Promise<T> {
        let lastError: Error | ApiError | unknown;
        const maxRetries = 3;
        const retryDelayMs = 30000; // 30秒

        for (let attempt = 0; attempt <= maxRetries; attempt++) {
            try {
                const response: AxiosResponse<T> = await this.axios.request({
                    ...config,
                    signal,
                });
                return response.data;
            } catch (error) {
                lastError = error;
                const errorMessage = (error as Error).message || '';
                const errorCode = (error as { code?: string }).code || '';

                const isTimeout =
                    errorMessage.includes('timeout') ||
                    errorCode === 'ETIMEDOUT';
                const isNetworkError =
                    errorMessage.includes('socket hang up') ||
                    errorMessage.includes('ECONNRESET') ||
                    errorMessage.includes('ECONNABORTED') ||
                    errorMessage.includes('EPIPE') ||
                    errorCode === 'EPIPE' ||
                    errorCode === 'ECONNRESET' ||
                    errorCode === 'ECONNABORTED';

                if (isTimeout || isNetworkError) {
                    if (attempt < maxRetries) {
                        // 网络错误或超时：断开旧连接，重建连接池并重新认证后再重试
                        try {
                            await this.reconnect();
                        } catch {
                            // 重连失败，等待后继续重试
                        }
                        await new Promise(resolve => setTimeout(resolve, retryDelayMs));
                        continue;
                    }
                }

                // 处理 403 自动登录
                if (error instanceof ApiError && error.statusCode === 403 && this.autoLogin) {
                    try {
                        await this.login();
                        const response: AxiosResponse<T> = await this.axios.request({
                            ...config,
                            signal,
                        });
                        return response.data;
                    } catch {
                        // 登录失败，继续抛出原错误
                    }
                }
                throw error;
            }
        }
        throw lastError;
    }

    /**
     * 将 tags 转换为安全的逗号分隔字符串：
     * - 接受字符串或字符串数组
     * - 去除逗号/分号，不允许的字符替换为下划线
     * - 去除空字符串并去重
     */
    private sanitizeTags(tags: string | string[]): string {
        const arr = Array.isArray(tags) ? tags : String(tags).split(',');
        const cleaned = arr
            .map((t) => t.trim())
            .map((t) => t.replace(/[;,]+/g, ''))
            .map((t) => t.replace(/[^a-zA-Z0-9_.-]/g, '_'))
            .filter(Boolean);
        const dedup = Array.from(new Set(cleaned));
        return dedup.join(',');
    }

    /**
     * 解析 API 返回的 tags 字段为 string[]：
     * - 接受字符串或字符串数组
     * - 应用与 sanitizeTags 相同的清洗逻辑，返回数组形式
     */
    private parseTags(tags: string | string[] | undefined | null): string[] {
        if (tags == null) return [];
        const arr = Array.isArray(tags) ? tags : String(tags).split(',');
        const cleaned = arr
            .map((t) => String(t).trim())
            .map((t) => t.replace(/[;,]+/g, ''))
            .map((t) => t.replace(/[^a-zA-Z0-9_.-]/g, '_'))
            .filter(Boolean);
        return Array.from(new Set(cleaned));
    }

    /**
     * 登录到 qBittorrent
     */
    async login(): Promise<void> {
        if (!this.username || !this.password) {
            throw new ApiError('Username and password are required for login');
        }

        const params = new URLSearchParams();
        params.append('username', this.username);
        params.append('password', this.password);

        const response = await this.axios.post<string>('/api/v2/auth/login', params);

        if (response.data === 'Ok.') {
            this.loggedIn = true;
        } else {
            throw new ApiError('Login failed: Invalid credentials');
        }
    }

    /**
     * 登出
     */
    async logout(): Promise<void> {
        await this.request<void>({
            method: 'POST',
            url: '/api/v2/auth/logout',
        });
        this.loggedIn = false;
        this.cookieJar = [];
    }

    /**
     * 检查是否已登录
     */
    isLoggedIn(): boolean {
        return this.loggedIn;
    }

    /**
     * 获取应用版本
     */
    async getAppVersion(signal?: AbortSignal): Promise<string> {
        return this.request<string>(
            {
                method: 'GET',
                url: '/api/v2/app/version',
            },
            signal
        );
    }

    /**
     * 获取 Web API 版本
     */
    async getWebApiVersion(signal?: AbortSignal): Promise<string> {
        return this.request<string>(
            {
                method: 'GET',
                url: '/api/v2/app/webapiVersion',
            },
            signal
        );
    }

    /**
     * 获取 Torrent 列表
     */
    async getTorrents(
        options?: GetTorrentsOptions,
        signal?: AbortSignal
    ): Promise<TorrentInfo[]> {
        const params = new URLSearchParams();

        if (options) {
            Object.entries(options).forEach(([key, value]) => {
                if (value !== undefined) {
                    // 处理 tag 数组并进行防呆转换
                    if (key === 'tag') {
                        params.append(key, this.sanitizeTags(value as string | string[]));
                    } else {
                        params.append(key, String(value));
                    }
                }
            });
        }

        const data = await this.request<any[]>(
            {
                method: 'GET',
                url: '/api/v2/torrents/info',
                params,
            },
            signal
        );

        return data.map((item) => {
            const parsedTags = this.parseTags((item as any).tags);
            return {
                ...item,
                tags: parsedTags,
            } as TorrentInfo;
        });
    }

    /**
     * 通过单个 hash 获取 Torrent（返回第一个匹配项或 null）
     */
    async getTorrentByHash(
        hash: string,
        signal?: AbortSignal
    ): Promise<TorrentInfo | null> {
        const list = await this.getTorrents({ hashes: hash }, signal);
        return list.length > 0 ? list[0] : null;
    }

    /**
     * 获取 Torrent 属性
     */
    async getTorrentProperties(
        hash: string,
        signal?: AbortSignal
    ): Promise<TorrentProperties> {
        const params = new URLSearchParams();
        params.append('hash', hash);

        return this.request<TorrentProperties>(
            {
                method: 'GET',
                url: '/api/v2/torrents/properties',
                params,
            },
            signal
        );
    }

    /**
     * 通过磁力链接添加 Torrent
     */
    async addTorrentByMagnet(
        magnet: string,
        options?: AddTorrentOptions,
        signal?: AbortSignal
    ): Promise<void> {
        const params = new URLSearchParams();
        params.append('urls', magnet);

        if (options) {
            Object.entries(options).forEach(([key, value]) => {
                if (value !== undefined) {
                    if (key === 'tags') {
                        params.append(key, this.sanitizeTags(value as string | string[]));
                    } else {
                        params.append(key, String(value));
                    }
                }
            });
        }

        await this.request<void>(
            {
                method: 'POST',
                url: '/api/v2/torrents/add',
                data: params,
            },
            signal
        );
    }

    /**
     * 通过文件添加 Torrent
     */
    async addTorrentByFile(
        fileBuffer: Buffer,
        options?: AddTorrentOptions,
        signal?: AbortSignal
    ): Promise<void> {
        const boundary = `----WebKitFormBoundary${Math.random().toString(36).substring(2)}`;
        const parts: Buffer[] = [];

        // 添加文件部分
        parts.push(Buffer.from(
            `--${boundary}\r\n` +
            `Content-Disposition: form-data; name="torrents"; filename="torrent.torrent"\r\n` +
            `Content-Type: application/x-bittorrent\r\n\r\n`
        ));
        parts.push(fileBuffer);
        parts.push(Buffer.from('\r\n'));

        // 添加其他字段
        if (options) {
            Object.entries(options).forEach(([key, value]) => {
                if (value !== undefined) {
                    // 处理 tags 数组并进行防呆转换
                    const stringValue = key === 'tags'
                        ? this.sanitizeTags(value as string | string[])
                        : String(value);
                    parts.push(Buffer.from(
                        `--${boundary}\r\n` +
                        `Content-Disposition: form-data; name="${key}"\r\n\r\n` +
                        `${stringValue}\r\n`
                    ));
                }
            });
        }

        // 结束边界
        parts.push(Buffer.from(`--${boundary}--\r\n`));

        const data = Buffer.concat(parts);

        await this.request<void>(
            {
                method: 'POST',
                url: '/api/v2/torrents/add',
                data,
                headers: {
                    'Content-Type': `multipart/form-data; boundary=${boundary}`,
                },
            },
            signal
        );
    }

    /**
     * 删除 Torrent
     * @param hash - Torrent 的 hash 或 hash 数组
     * @param deleteFiles - 是否删除关联的文件，默认为 true
     */
    async deleteTorrent(
        hash: string | string[],
        deleteFiles: boolean = true,
        signal?: AbortSignal
    ): Promise<void> {
        const params = new URLSearchParams();
        params.append('hashes', Array.isArray(hash) ? hash.join('|') : hash);
        params.append('deleteFiles', String(deleteFiles));

        await this.request<void>(
            {
                method: 'POST',
                url: '/api/v2/torrents/delete',
                data: params,
            },
            signal
        );
    }

    /**
     * 暂停 Torrent
     */
    async pauseTorrent(
        hash: string | string[],
        signal?: AbortSignal
    ): Promise<void> {
        const params = new URLSearchParams();
        params.append('hashes', Array.isArray(hash) ? hash.join('|') : hash);

        await this.request<void>(
            {
                method: 'POST',
                url: '/api/v2/torrents/pause',
                data: params,
            },
            signal
        );
    }

    /**
     * 恢复 Torrent
     */
    async resumeTorrent(
        hash: string | string[],
        signal?: AbortSignal
    ): Promise<void> {
        const params = new URLSearchParams();
        params.append('hashes', Array.isArray(hash) ? hash.join('|') : hash);

        await this.request<void>(
            {
                method: 'POST',
                url: '/api/v2/torrents/resume',
                data: params,
            },
            signal
        );
    }

    /**
     * 为 Torrent 添加标签
     */
    async addTags(
        hash: string | string[],
        tags: string | string[],
        signal?: AbortSignal
    ): Promise<void> {
        const params = new URLSearchParams();
        params.append('hashes', Array.isArray(hash) ? hash.join('|') : hash);
        params.append('tags', this.sanitizeTags(tags));

        await this.request<void>(
            {
                method: 'POST',
                url: '/api/v2/torrents/addTags',
                data: params,
            },
            signal
        );
    }

    /**
     * 获取传输信息
     */
    async getTransferInfo(signal?: AbortSignal): Promise<TransferInfo> {
        return this.request<TransferInfo>(
            {
                method: 'GET',
                url: '/api/v2/transfer/info',
            },
            signal
        );
    }
}

/**
 * 创建 qBittorrent 客户端实例的工厂函数
 */
export function createQbittorrentClient(
    config: QbittorrentClientConfig
): QbittorrentClient {
    return new QbittorrentClient(config);
}

export default QbittorrentClient;
