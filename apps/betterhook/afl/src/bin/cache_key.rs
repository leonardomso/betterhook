//! afl.rs harness for cache key derivation primitives.

fn main() {
    afl::fuzz!(|data: &[u8]| {
        betterhook::fuzz_harnesses::run_cache_key(data);
    });
}
