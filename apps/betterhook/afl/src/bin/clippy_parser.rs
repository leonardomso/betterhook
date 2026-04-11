//! afl.rs harness for the clippy/cargo `--message-format=json` parser.
//!
//! Each stdin payload is sent through `clippy::parse_output`. The
//! parser is hand-written over `serde_json::Value` so the goal here is
//! to find a JSON shape that triggers an unwrap or out-of-range index.

use betterhook::builtins::clippy;

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let Ok(s) = std::str::from_utf8(data) else {
            return;
        };
        let _ = clippy::parse_output(s);
    });
}
