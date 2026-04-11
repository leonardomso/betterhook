//! afl.rs harness for cache key derivation primitives.
//!
//! Splits each input on `0x00` and feeds the resulting fragments
//! through `hash_bytes` and `args_hash`. The goal is to verify the
//! NUL-separated args canonicalization is panic-free for any byte
//! sequence — including ones that contain control characters,
//! invalid UTF-8 boundaries, and zero-length args.

use betterhook::cache::{args_hash, hash_bytes};

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let _ = hash_bytes(data);
        let parts: Vec<String> = data
            .split(|b| *b == 0)
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect();
        let _ = args_hash(&parts);
    });
}
