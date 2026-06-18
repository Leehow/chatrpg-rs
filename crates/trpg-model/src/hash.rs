//! 内容寻址哈希原语。
//!
//! 从 `lib.rs` facade 拆出的低耦合工具：稳定的 SHA-256 文本哈希与
//! 任意可序列化值的稳定 JSON 哈希。通过 `pub use hash::*` 在 crate 根重导出，
//! 公共 API 保持不变。

use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn sha256_hex(input: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_ref());
    format!("sha256:{:x}", hasher.finalize())
}

pub fn stable_json_hash<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    sha256_hex(bytes)
}
