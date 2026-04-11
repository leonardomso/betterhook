//! afl.rs harness for the eslint JSON parser.

fn main() {
    afl::fuzz!(|data: &[u8]| {
        betterhook::fuzz_harnesses::run_eslint_parser(data);
    });
}
