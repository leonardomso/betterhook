//! `xtask fuzz-smoke` — bit-rot guard for the afl.rs fuzz harnesses.
//!
//! This is **not** a fuzzing campaign. The goal is the opposite: we
//! want a fast, deterministic check that every harness still compiles
//! and accepts every seed input without panicking, so a refactor can
//! never silently break a harness without anyone noticing.
//!
//! It works by importing the same library functions each harness
//! calls and feeding them the bytes of every file under
//! `apps/betterhook/afl/seeds/<target>/`. No `cargo afl` install
//! required.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use betterhook::builtins::{clippy, eslint};
use betterhook::cache::{args_hash, hash_bytes};
use betterhook::config::import::husky;
use betterhook::config::parse::Format;
use betterhook::config::parse_bytes;
use betterhook::runner::dag::build_dag;

const SEEDS_ROOT: &str = "apps/betterhook/afl/seeds";

type HarnessFn = fn(&[u8]);

pub fn run(_args: &[String]) -> ExitCode {
    let targets: &[(&str, HarnessFn)] = &[
        ("config_parse", run_config_parse),
        ("dag_resolver", run_dag_resolver),
        ("clippy_parser", run_clippy_parser),
        ("eslint_parser", run_eslint_parser),
        ("husky_importer", run_husky_importer),
        ("cache_key", run_cache_key),
    ];

    let mut total_seeds = 0usize;
    let mut total_failures = 0usize;
    for (name, harness) in targets {
        let dir = PathBuf::from(SEEDS_ROOT).join(name);
        let seeds = match collect_seeds(&dir) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("xtask fuzz-smoke: cannot read seeds for {name}: {e}");
                total_failures += 1;
                continue;
            }
        };
        if seeds.is_empty() {
            eprintln!(
                "xtask fuzz-smoke: WARN — no seeds for {name} (looked in {})",
                dir.display()
            );
            continue;
        }
        eprintln!("xtask fuzz-smoke: {name} ({} seed{})", seeds.len(), if seeds.len() == 1 { "" } else { "s" });
        for seed in &seeds {
            total_seeds += 1;
            // Each harness call is wrapped in `catch_unwind` so a
            // single panic doesn't take down the whole smoke pass.
            let bytes = match std::fs::read(seed) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("  ✗ {} — read error: {e}", seed.display());
                    total_failures += 1;
                    continue;
                }
            };
            let seed_for_msg = seed.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                harness(&bytes);
            }));
            if result.is_err() {
                eprintln!("  ✗ {} — harness panicked", seed_for_msg.display());
                total_failures += 1;
            }
        }
    }

    // After every seed has been replayed, also feed each harness a
    // small set of pathological adversarial inputs. These are not
    // meant to find bugs (the unit + integration tests already cover
    // these shapes); they exist so that *future* regressions where a
    // refactor reintroduces a panic on garbage are caught at smoke
    // time, not by an end user.
    let adversarial: &[&[u8]] = &[
        b"",
        b"\x00",
        b"\x00\x00\x00\x00",
        b"\xff\xfe\xfd\xfc",
        b"{",
        b"}",
        b"[",
        b"]",
        b"\"",
        b"\\",
        b"\xc3\x28", // invalid UTF-8 boundary
        &[0xff; 256],
    ];
    for (name, harness) in targets {
        for input in adversarial {
            total_seeds += 1;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                harness(input);
            }));
            if result.is_err() {
                eprintln!("  ✗ {name} — adversarial input panicked: {input:?}");
                total_failures += 1;
            }
        }
    }

    eprintln!(
        "xtask fuzz-smoke: {total_seeds} input{} across {} target{}, {total_failures} failure{}",
        if total_seeds == 1 { "" } else { "s" },
        targets.len(),
        if targets.len() == 1 { "" } else { "s" },
        if total_failures == 1 { "" } else { "s" },
    );
    if total_failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn collect_seeds(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

// ────────────────────── per-target harness wrappers ──────────────────
//
// These are intentionally byte-for-byte equivalent to the bodies of
// the corresponding `apps/betterhook/afl/src/bin/<target>.rs`
// harnesses. If they ever drift, the smoke test loses its value as a
// bit-rot guard.

fn run_config_parse(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = parse_bytes(s, Format::Toml, "smoke.toml");
    let _ = parse_bytes(s, Format::Yaml, "smoke.yml");
    let _ = parse_bytes(s, Format::Json, "smoke.json");
    let _ = parse_bytes(s, Format::Kdl, "smoke.kdl");
}

fn run_dag_resolver(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(raw) = parse_bytes(s, Format::Toml, "smoke.toml") else {
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

fn run_clippy_parser(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = clippy::parse_output(s);
}

fn run_eslint_parser(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = eslint::parse_output(s);
}

fn run_husky_importer(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = husky::from_script(s, &PathBuf::from(".husky/pre-commit"));
}

fn run_cache_key(data: &[u8]) {
    let _ = hash_bytes(data);
    let parts: Vec<String> = data
        .split(|b| *b == 0)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect();
    let _ = args_hash(&parts);
}
