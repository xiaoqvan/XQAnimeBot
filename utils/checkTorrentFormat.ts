import parseTorrent, { remote } from "parse-torrent";
import logger from "@log/index.ts";
import { getQBClient } from "../qBittorrent/index.ts";

export type TorrentFormat = "mkv" | "mp4" | "mixed" | "unknown";

/**
 * 根据文件名列表判断视频容器格式
 */
function classifyFileNames(names: string[]): TorrentFormat {
    let hasMkv = false;
    let hasMp4 = false;

    for (const fileName of names) {
        const lower = (fileName ?? "").toLowerCase();
        if (lower.endsWith(".mkv")) hasMkv = true;
        else if (lower.endsWith(".mp4")) hasMp4 = true;
    }

    if (hasMkv && !hasMp4) return "mkv";
    if (hasMp4 && !hasMkv) return "mp4";
    if (hasMkv && hasMp4) return "mixed";
    return "unknown";
}

/**
 * 通过 qBittorrent 获取磁力链的种子元数据（文件列表）。
 *
 * 注意：parse-torrent@11 的 remote() 对 magnet 只做 URI 解析（infoHash/dn/tracker），
 * 不会拉取 metadata，因此 parsed.files 恒为空，后缀预检 100% 失败。
 * 必须借 qBittorrent 的 metadata 通道读文件名。
 *
 * 已存在的种子会复用；新添加的种子以暂停态加入，仅取 metadata，不立刻下载。
 */
async function checkFormatViaQbittorrent(magnetLink: string): Promise<TorrentFormat> {
    const QBclient = await getQBClient();
    const parsed = await parseTorrent(magnetLink);
    const hash = parsed.infoHash;
    if (!hash) return "unknown";

    let torrent = await QBclient.getTorrentByHash(hash);
    if (!torrent) {
        await QBclient.addTorrentByMagnet(magnetLink, { paused: true });
    }

    const deadline = Date.now() + 60_000;
    while (Date.now() < deadline) {
        torrent = await QBclient.getTorrentByHash(hash);
        if (torrent?.has_metadata === true) break;
        await new Promise((r) => setTimeout(r, 2000));
    }

    if (!torrent?.has_metadata) {
        logger.warn(
            `[checkTorrentFormat] qBittorrent 元数据超时，标记为 unknown: ${magnetLink.slice(0, 60)}...`
        );
        return "unknown";
    }

    const files = await QBclient.getTorrentFiles(hash);
    if (!files || files.length === 0) {
        const name: string = torrent.name ?? "";
        return classifyFileNames([name]);
    }

    return classifyFileNames(files.map((f) => f.name ?? ""));
}

/**
 * 预检查种子视频格式（在完整下载前判断是否为 MKV，以便路由到烧录队列）。
 *
 * @param magnetLink - 磁力链接或 .torrent URL
 * @returns 检测到的格式类型
 *   - "mkv": 只包含 MKV 文件
 *   - "mp4": 只包含 MP4 文件
 *   - "mixed": 同时包含 MKV 和 MP4
 *   - "unknown": 无法获取元数据或无视频文件
 */
export async function checkTorrentFormat(
    magnetLink: string
): Promise<TorrentFormat> {
    try {
        const isMagnet = magnetLink.trim().toLowerCase().startsWith("magnet:");

        // 磁力链：走 qBittorrent metadata（parse-torrent.remote 对 magnet 无效）
        if (isMagnet) {
            return await checkFormatViaQbittorrent(magnetLink);
        }

        // .torrent URL / 本地路径：parse-torrent.remote 可直接解出 files
        const parsed = await new Promise<any>((resolve, reject) => {
            (remote as any)(
                magnetLink,
                { timeout: 30_000 },
                (err: Error | null, parsed: any) => {
                    if (err) reject(err);
                    else resolve(parsed);
                }
            );
        });

        const files: { name?: string; path?: string; length?: number }[] =
            parsed.files ?? [];

        if (files.length === 0) {
            const name: string = parsed.name ?? "";
            return classifyFileNames([name]);
        }

        return classifyFileNames(
            files.map((f) => f.name ?? f.path ?? "")
        );
    } catch (err) {
        logger.warn(err, `[checkTorrentFormat] 获取种子元数据失败，标记为 unknown: ${magnetLink.slice(0, 60)}...`);
        return "unknown";
    }
}
