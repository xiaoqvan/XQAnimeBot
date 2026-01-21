const BASE62_CHARS = '0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ';

/**
 * 将 Base62 编码的字符串转换为十六进制字符串
 * @param s Base62 编码的字符串
 * @returns 对应的十六进制字符串
 */
export function base62ToHex(s: string) {
    let val = 0n;
    for (const ch of s) {
        const idx = BASE62_CHARS.indexOf(ch);
        if (idx === -1) throw new Error('invalid base62 char ' + ch);
        val = val * 62n + BigInt(idx);
    }
    let hx = val.toString(16); // 返回 hex（无 0x 前缀）
    // 保留前导零：默认填充到 40 个 hex 字符（如 SHA1 长度），如果需要可调整
    const TARGET_HEX_LEN = 40;
    if (hx.length < TARGET_HEX_LEN) hx = hx.padStart(TARGET_HEX_LEN, '0');
    // 若长度为奇数，前面补 0 使其为偶数长度
    if (hx.length % 2 === 1) hx = '0' + hx;
    return hx;
}