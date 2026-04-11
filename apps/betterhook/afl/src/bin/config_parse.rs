//! afl.rs harness for the multi-format config parser.
//!
//! Feeds raw stdin bytes through every supported format. A panic from
//! any of these calls is a parser bug. The four formats are tried
//! independently because afl can't know which one a given input is
//! "supposed" to be.

use betterhook::config::parse::Format;
use betterhook::config::parse_bytes;

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let Ok(s) = std::str::from_utf8(data) else {
            return;
        };
        let _ = parse_bytes(s, Format::Toml, "afl.toml");
        let _ = parse_bytes(s, Format::Yaml, "afl.yml");
        let _ = parse_bytes(s, Format::Json, "afl.json");
        let _ = parse_bytes(s, Format::Kdl, "afl.kdl");
    });
}
