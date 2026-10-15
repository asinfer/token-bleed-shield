//! Hashing primitives: content addressing, shingle hashing, template skeletonization.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};

static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
        .expect("uuid regex")
});
static HEX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[0-9a-fA-F]{8,}\b").expect("hex regex"));
static NUM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d+(?:\.\d+)?\b").expect("num regex"));
static TMP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:/tmp|/var/tmp|/var/folders)/[^\s"']+"#).expect("tmp regex"));

pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_ref());
    hex::encode(hasher.finalize())
}

pub fn sha256_16(data: impl AsRef<[u8]>) -> String {
    sha256_hex(data).chars().take(16).collect()
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Word-level n-gram shingles hashed to u64. Holds hashes only, never text.
pub fn shingle_hashes(text: &str, n: usize) -> Vec<u64> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < n || n == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(words.len() - n + 1);
    for window in words.windows(n) {
        let mut buf: Vec<u8> = Vec::with_capacity(64);
        for w in window {
            buf.extend_from_slice(w.to_lowercase().as_bytes());
            buf.push(b' ');
        }
        out.push(fnv1a64(&buf));
    }
    out
}

/// k-line shingles over trimmed lines, hashed. Used for duplication/containment.
pub fn line_shingles(text: &str, k: usize) -> Vec<u64> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < k || k == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(lines.len() - k + 1);
    for window in lines.windows(k) {
        let mut buf: Vec<u8> = Vec::with_capacity(128);
        for l in window {
            buf.extend_from_slice(l.as_bytes());
            buf.push(b'\n');
        }
        out.push(fnv1a64(&buf));
    }
    out
}

/// Mask volatile literals (ids, numbers, temp paths) to hash stable templates.
pub fn template_skeleton(text: &str) -> String {
    let out = UUID_RE.replace_all(text, "<uuid>");
    let out = HEX_RE.replace_all(&out, "<hex>");
    let out = TMP_RE.replace_all(&out, "<tmp>");
    NUM_RE.replace_all(&out, "<n>").into_owned()
}

/// serde_json maps are BTreeMap-backed (sorted keys) without `preserve_order`.
pub fn canonical_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// Remove volatile keys recursively before hashing tool arguments.
pub fn strip_volatile(v: &mut Value) {
    const VOLATILE: &[&str] = &[
        "timestamp",
        "timestamps",
        "ts",
        "time",
        "date",
        "created_at",
        "updated_at",
        "nonce",
    ];
    match v {
        Value::Object(map) => {
            map.retain(|k, _| !VOLATILE.contains(&k.to_ascii_lowercase().as_str()));
            for (_, child) in map.iter_mut() {
                strip_volatile(child);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                strip_volatile(item);
            }
        }
        _ => {}
    }
}

pub fn canonical_tool_args(args: &Value) -> String {
    let mut v = args.clone();
    strip_volatile(&mut v);
    canonical_json(&v)
}
