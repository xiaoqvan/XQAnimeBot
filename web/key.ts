import { createHmac, timingSafeEqual, randomBytes } from "node:crypto";
import { readFileSync, writeFileSync, existsSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve, dirname } from "node:path";
import logger from "@log/index.ts";

/**
 * 动态签名密钥机制（HMAC）。
 *
 * - 由 Bot 命令（/web）为主人/管理员签发，格式：
 *   `btoaurl(payload) + "." + btoaurl(hmac)`
 *   payload = { u: 签发对象标识, exp: 过期时间戳(ms) }
 * - 服务端用 SECRET 验签并检查 exp，无需存库。
 * - 默认有效期 24 小时。
 */

const KEY_TTL_MS = 24 * 60 * 60 * 1000; // 24 小时

function getSecret(): Buffer {
    const envSecret = process.env.WEB_KEY_SECRET;
    if (envSecret) {
        return Buffer.from(envSecret, "utf8");
    }
    // 持久化到本地文件，保证重启后 24h key 仍有效
    const file = resolve(
        dirname(fileURLToPath(import.meta.url)),
        ".web_secret"
    );
    try {
        if (existsSync(file)) {
            return Buffer.from(readFileSync(file, "utf8").trim(), "utf8");
        }
        const secret = randomBytes(32).toString("hex");
        mkdirSync(dirname(file), { recursive: true });
        writeFileSync(file, secret, { encoding: "utf8", mode: 0o600 });
        logger.info("[Web] 已生成 Web 密钥签名密钥（.web_secret）");
        return Buffer.from(secret, "utf8");
    } catch (err) {
        logger.warn(err, "[Web] 无法持久化密钥签名密钥，改用进程内随机密钥（重启后旧 key 失效）");
        return randomBytes(32);
    }
}

function base64url(s: Buffer | string): string {
    const b = typeof s === "string" ? Buffer.from(s, "utf8") : s;
    return b.toString("base64url");
}

function sign(payloadBase64: string): string {
    return createHmac("sha256", getSecret())
        .update(payloadBase64)
        .digest("base64url");
}

/** 签发一个 24h 有效的访问密钥 */
export function issueKey(label: string = "web"): { key: string; expiresAt: string } {
    const exp = Date.now() + KEY_TTL_MS;
    const payload = JSON.stringify({ u: label, exp });
    const payloadB64 = base64url(payload);
    const sig = sign(payloadB64);
    return {
        key: `${payloadB64}.${sig}`,
        expiresAt: new Date(exp).toISOString(),
    };
}

/** 校验密钥：验签 + 过期检查。通过返回解析后的 payload，否则返回 null */
export function verifyKey(key: string): { u: string; exp: number } | null {
    if (!key) return null;
    const dot = key.indexOf(".");
    if (dot <= 0) return null;
    const payloadB64 = key.slice(0, dot);
    const sig = key.slice(dot + 1);

    try {
        // 验签（恒定时间比较）
        const expected = sign(payloadB64);
        const a = Buffer.from(sig, "base64url");
        const b = Buffer.from(expected, "base64url");
        if (a.length !== b.length || !timingSafeEqual(a, b)) {
            return null;
        }
        const payload = JSON.parse(Buffer.from(payloadB64, "base64url").toString("utf8")) as {
            u?: string;
            exp?: number;
        };
        if (!payload.exp || typeof payload.exp !== "number") return null;
        // 过期检查
        if (payload.exp < Date.now()) return null;
        return { u: String(payload.u ?? ""), exp: payload.exp };
    } catch {
        return null;
    }
}
