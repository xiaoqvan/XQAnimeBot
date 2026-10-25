//! 图卡**数据层**：把业务对象整理成模板 payload，交给出图插件渲染。
//!
//! 这里不做任何像素操作，也不认识 Ultralight——它只负责把 `Anime` / Bangumi API
//! 的数据拼成一份「模板能直接吃」的 JSON。渲染是 ultralightGenImage 插件的事。

pub mod bangumi;
pub mod image;

use sha2::{Digest, Sha256};

/// 内容哈希（前 16 位十六进制）：用于判断「这张图要不要重画」。
///
/// 用 SHA-256 而不是 `DefaultHasher`：后者跨进程不稳定，重启后缓存全失效。
pub fn content_hash_of(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0x1f]); // 分隔符，避免 "ab"+"c" 与 "a"+"bc" 撞车
    }
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_order_sensitive() {
        let a = content_hash_of(&["bangumi", "1", "8.7"]);
        let b = content_hash_of(&["bangumi", "1", "8.7"]);
        let c = content_hash_of(&["bangumi", "8.7", "1"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
    }
}
