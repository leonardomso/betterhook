//! Content-addressable hook cache.
//!
//! The cache lives under `<common-dir>/betterhook/cache/` and is
//! keyed on `blake3(content) + blake3(tool_binary) + blake3(args)`.
//! A hit lets the runner skip spawning a subprocess and replay the
//! cached `OutputEvent`s through the multiplexer instead — this is
//! where the "faster than hk" v1 claim is earned.
//!
//! This module owns the hashing primitives, on-disk layout, and the
//! high-level lookup/store helpers the runner uses to replay cached
//! hook results.

pub mod hash;
pub mod lookup;
pub mod store;
pub mod tool_hash;

pub use hash::{
    ArgsHash, CacheKey, ContentHash, ToolHash, args_hash, combine_key, hash_bytes, hash_file,
};
pub use lookup::{
    args_hash_from_job, derive_key, hash_file_set, inputs_fresh, lookup, lookup_blocking,
    snapshot_inputs, store as store_result, store_blocking as store_result_blocking,
    tool_hash_proxy,
};
pub use store::{CachedInput, CachedResult, Stats, Store, StoreError, cache_dir};
pub use tool_hash::{resolve_tool_hash, try_resolve_tool_hash};
