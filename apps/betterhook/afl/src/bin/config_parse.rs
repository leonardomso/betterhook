//! afl.rs harness for the multi-format config parser. The actual
//! harness logic lives in `betterhook::fuzz_harnesses` so every
//! fuzzer in the repo exercises the same code path.

fn main() {
    afl::fuzz!(|data: &[u8]| {
        betterhook::fuzz_harnesses::run_config_parse(data);
    });
}
