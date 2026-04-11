//! afl.rs harness for the eslint `--format=json` parser.
//!
//! eslint's JSON shape is the most heavily nested of any builtin (a
//! top-level array → per-file objects → per-message arrays), so it's
//! the most likely place to find a panic on unexpected types.

use betterhook::builtins::eslint;

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let Ok(s) = std::str::from_utf8(data) else {
            return;
        };
        let _ = eslint::parse_output(s);
    });
}
