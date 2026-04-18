//! afl.rs harness for the clippy JSON parser.

fn main() {
    afl::fuzz!(|data: &[u8]| {
        betterhook::fuzz_harnesses::run_clippy_parser(data);
    });
}
