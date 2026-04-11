//! afl.rs harness for the husky shell-script importer.

fn main() {
    afl::fuzz!(|data: &[u8]| {
        betterhook::fuzz_harnesses::run_husky_importer(data);
    });
}
