#![no_main]

//! Fuzz the multi-format config parser. Every format deserialize
//! must either return a RawConfig or a structured ConfigError — a
//! panic here is a bug.

use betterhook::config::{Format, parse_bytes};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    // Try every format — they all deserialize into the same RawConfig.
    let _ = parse_bytes(s, Format::Toml, "fuzz.toml");
    let _ = parse_bytes(s, Format::Yaml, "fuzz.yml");
    let _ = parse_bytes(s, Format::Json, "fuzz.json");
});
