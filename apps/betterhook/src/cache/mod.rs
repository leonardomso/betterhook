//! Content-addressable hook cache.
//!
//! The cache lives under `<common-dir>/betterhook/cache/` and is
//! keyed on `blake3(content) + blake3(tool_binary) + blake3(args)`.
//! A hit lets the runner skip spawning a subprocess and replay the
//! cached `OutputEvent`s through the multiplexer instead — this is
//! where the "faster than hk" v1 claim is earned.
//!
//! Phase 24 ships the scaffolding: hashing primitives, the disk
//! layout, and the store round-trip. Runtime integration with the
//! runner lands in phases 29–31.

pub mod hash;
pub mod lookup;
pub mod store;

pub use hash::{
    ArgsHash, CacheKey, ContentHash, ToolHash, args_hash, combine_key, hash_bytes, hash_file,
};
pub use lookup::{
    args_hash_from_job, derive_key, hash_file_set, lookup, store as store_result, tool_hash_proxy,
};
pub use store::{CachedResult, Store, StoreError, cache_dir};
