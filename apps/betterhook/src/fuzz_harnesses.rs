//! Canonical fuzz-target entry points.
//!
//! These were previously duplicated across
//! `apps/betterhook/afl/src/bin/*.rs`, `xtask::fuzz_smoke`, and
//! `xtask::fuzz`. Any drift between the three copies meant an afl
//! fuzzing campaign, a smoke test, and an in-process mutation run
//! could exercise *different* code paths — a correctness hole.
//!
//! Single source of truth now lives here, gated behind the
//! `fuzz-harnesses` cargo feature. Enable the feature in the crate
//! that needs these functions; the main betterhook library builds
//! without them by default.

use std::path::PathBuf;

use crate::builtins::{clippy, eslint};
use crate::cache::{args_hash, hash_bytes};
use crate::config::import::husky;
use crate::config::parse::Format;
use crate::config::parse_bytes;
use crate::runner::dag::build_dag;

/// Run every supported config format against `data`. A panic from any
/// parser is a bug.
pub fn run_config_parse(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = parse_bytes(s, Format::Toml, "fuzz.toml");
    let _ = parse_bytes(s, Format::Yaml, "fuzz.yml");
    let _ = parse_bytes(s, Format::Json, "fuzz.json");
    let _ = parse_bytes(s, Format::Kdl, "fuzz.kdl");
}

/// Parse TOML, lower to `Config`, then run `build_dag` against every
/// hook (root + packages). Panics in any layer are bugs.
pub fn run_dag_resolver(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(raw) = parse_bytes(s, Format::Toml, "fuzz.toml") else {
        return;
    };
    let Ok(cfg) = raw.lower() else {
        return;
    };
    if let Some(hook) = cfg.hooks.get("pre-commit") {
        let _ = build_dag(&hook.jobs);
    }
    for pkg in cfg.packages.values() {
        for hook in pkg.hooks.values() {
            let _ = build_dag(&hook.jobs);
        }
    }
}

/// Feed `data` through the clippy compiler-message JSON parser.
pub fn run_clippy_parser(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = clippy::parse_output(s);
}

/// Feed `data` through the eslint JSON parser.
pub fn run_eslint_parser(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = eslint::parse_output(s);
}

/// Feed `data` through the husky shell-script importer.
pub fn run_husky_importer(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = husky::from_script(s, &PathBuf::from(".husky/pre-commit"));
}

/// Feed `data` through the cache key derivation primitives.
pub fn run_cache_key(data: &[u8]) {
    let _ = hash_bytes(data);
    let parts: Vec<String> = data
        .split(|b| *b == 0)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect();
    let _ = args_hash(&parts);
}
