//! blake3 hashing primitives for the content-addressable cache.
//!
//! Every hash is a 32-byte blake3 digest serialized as its 64-char
//! lowercase hex form. The cache uses the first two hex chars of the
//! tool hash as a sharding prefix on disk to avoid 10k+ files in a
//! single directory.

use std::io;
use std::path::Path;

/// Size of a blake3 digest in bytes. Matches `blake3::OUT_LEN`.
pub const HASH_LEN: usize = 32;

/// Hex-encoded blake3 digest for a file's contents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentHash(pub String);

/// Hex-encoded blake3 digest for the resolved tool binary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolHash(pub String);

/// Hex-encoded blake3 digest for the canonicalized argument list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArgsHash(pub String);

/// A combined (content + tool + args) cache key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub content: ContentHash,
    pub tool: ToolHash,
    pub args: ArgsHash,
}

impl CacheKey {
    /// Produce the on-disk relative path for this key:
    /// `<tool-hex-first-2>/<tool-hex-rest>__<content-hex>__<args-hex>.json`.
    ///
    /// Returns a `PathBuf` so callers can `join()` it onto the cache
    /// root without converting through an intermediate `String`.
    #[must_use]
    pub fn relative_path(&self) -> std::path::PathBuf {
        let (head, tail) = self.tool.0.split_at(2);
        let file_name = format!("{tail}__{c}__{a}.json", c = self.content.0, a = self.args.0);
        std::path::PathBuf::from(head).join(file_name)
    }
}

/// Blake3-hash arbitrary bytes.
#[must_use]
pub fn hash_bytes(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

/// Stream-hash a file via `blake3::Hasher::update_mmap` when possible,
/// falling back to a buffered read for files that can't be mmapped.
#[must_use = "discarding a content hash means wasted I/O"]
pub fn hash_file(path: &Path) -> io::Result<ContentHash> {
    let mut hasher = blake3::Hasher::new();
    // `update_mmap` returns an io::Error on failure; we don't have a
    // fallback to plain buffered reads in v1 because every platform we
    // target supports mmap. The `_try` variant would let us degrade,
    // but v1 is macOS + Linux only and both always succeed.
    hasher.update_mmap(path)?;
    Ok(ContentHash(hasher.finalize().to_hex().to_string()))
}

/// Hash an args list in a canonical form (NUL-separated). Using NUL
/// means any permutation is a distinct key, which is what we want —
/// `eslint --fix a.ts b.ts` and `eslint a.ts --fix b.ts` must cache
/// separately because different tools may reorder behavior.
#[must_use]
pub fn args_hash(args: &[String]) -> ArgsHash {
    let cap = args.iter().map(|a| a.len() + 1).sum();
    let mut joined = Vec::with_capacity(cap);
    for arg in args {
        joined.extend_from_slice(arg.as_bytes());
        joined.push(0);
    }
    ArgsHash(hash_bytes(&joined))
}

/// Hash job fields directly into a blake3 hasher without allocating
/// intermediate `String`s. Each field is NUL-delimited so reordering
/// or insertion is always distinguishable.
#[must_use]
pub fn args_hash_from_fields(job: &crate::config::Job) -> ArgsHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"run:");
    hasher.update(job.run.as_bytes());
    hasher.update(b"\0");
    if let Some(fix) = &job.fix {
        hasher.update(b"fix:");
        hasher.update(fix.as_bytes());
        hasher.update(b"\0");
    }
    for g in &job.glob {
        hasher.update(b"glob:");
        hasher.update(g.as_bytes());
        hasher.update(b"\0");
    }
    for e in &job.exclude {
        hasher.update(b"exclude:");
        hasher.update(e.as_bytes());
        hasher.update(b"\0");
    }
    for (k, v) in &job.env {
        hasher.update(b"env:");
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update(b"\0");
    }
    ArgsHash(hasher.finalize().to_hex().to_string())
}

/// Combine the three axes into a single `CacheKey`.
#[must_use]
pub fn combine_key(content: ContentHash, tool: ToolHash, args: ArgsHash) -> CacheKey {
    CacheKey {
        content,
        tool,
        args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blake3_hex_length() {
        let h = hash_bytes(b"hello");
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn hash_bytes_is_stable() {
        assert_eq!(hash_bytes(b"hello"), hash_bytes(b"hello"));
        assert_ne!(hash_bytes(b"hello"), hash_bytes(b"world"));
    }

    #[test]
    fn args_hash_respects_order() {
        let a = args_hash(&["--fix".to_owned(), "a.ts".to_owned()]);
        let b = args_hash(&["a.ts".to_owned(), "--fix".to_owned()]);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_file_matches_hash_bytes_for_small_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sample.txt");
        let data = b"betterhook cache test contents";
        std::fs::write(&path, data).unwrap();
        let file_hash = hash_file(&path).unwrap();
        assert_eq!(file_hash.0, hash_bytes(data));
    }

    #[test]
    fn cache_key_relative_path_format() {
        let key = CacheKey {
            content: ContentHash("c".repeat(64)),
            tool: ToolHash("ab".to_string() + &"f".repeat(62)),
            args: ArgsHash("a".repeat(64)),
        };
        let rel = key.relative_path();
        assert!(rel.starts_with("ab"));
        assert!(rel.to_string_lossy().contains("__"));
        assert!(
            rel.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        );
    }
}
