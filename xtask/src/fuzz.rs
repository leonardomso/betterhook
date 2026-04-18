//! `xtask fuzz` — in-process random mutation fuzzer.
//!
//! This is the **immediate** fuzzing path: no `cargo afl` install, no
//! nightly toolchain. We seed a deterministic PRNG, mutate every seed
//! corpus file under `apps/betterhook/afl/seeds/<target>/` for the
//! requested wall-clock budget, and report any input that triggers a
//! panic.
//!
//! It's not a replacement for AFL or libFuzzer — there is no coverage
//! feedback, so it can't reach deep states behind narrow branches.
//! What it _is_ very good at is shaking out shallow panics in parsers,
//! and that's exactly the surface every harness in this repo targets.
//!
//! Usage:
//!
//! ```bash
//! cargo run -p xtask -- fuzz                  # 60 s per target
//! cargo run -p xtask -- fuzz --duration 120   # 2 min per target
//! cargo run -p xtask -- fuzz --target dag_resolver --duration 30
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use betterhook::fuzz_harnesses::{
    run_cache_key, run_clippy_parser, run_config_parse, run_dag_resolver, run_eslint_parser,
    run_husky_importer,
};

const SEEDS_ROOT: &str = "apps/betterhook/afl/seeds";

type HarnessFn = fn(&[u8]);

struct Target {
    name: &'static str,
    seeds_dir: &'static str,
    harness: HarnessFn,
}

const TARGETS: &[Target] = &[
    Target {
        name: "config_parse",
        seeds_dir: "config_parse",
        harness: run_config_parse,
    },
    Target {
        name: "dag_resolver",
        seeds_dir: "dag_resolver",
        harness: run_dag_resolver,
    },
    Target {
        name: "clippy_parser",
        seeds_dir: "clippy_parser",
        harness: run_clippy_parser,
    },
    Target {
        name: "eslint_parser",
        seeds_dir: "eslint_parser",
        harness: run_eslint_parser,
    },
    Target {
        name: "husky_importer",
        seeds_dir: "husky_importer",
        harness: run_husky_importer,
    },
    Target {
        name: "cache_key",
        seeds_dir: "cache_key",
        harness: run_cache_key,
    },
];

#[allow(clippy::too_many_lines)]
pub fn run(args: &[String]) -> ExitCode {
    let mut duration_secs = 60u64;
    let mut only_target: Option<String> = None;
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--duration" | "-d" => {
                if let Some(n) = iter.next().and_then(|v| v.parse::<u64>().ok()) {
                    duration_secs = n;
                }
            }
            "--target" | "-t" => {
                if let Some(v) = iter.next() {
                    only_target = Some(v.clone());
                }
            }
            "--help" | "-h" => {
                eprintln!("usage: xtask fuzz [--duration <seconds>] [--target <name>]");
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("xtask fuzz: unknown arg {arg}");
                return ExitCode::from(64);
            }
        }
    }

    let budget = Duration::from_secs(duration_secs);
    let mut total_iters = 0u64;
    let mut total_failures = 0usize;
    let mut crash_dir = PathBuf::from("target/fuzz-crashes");
    let _ = std::fs::create_dir_all(&crash_dir);

    for target in TARGETS {
        if only_target
            .as_deref()
            .is_some_and(|only| only != target.name)
        {
            continue;
        }
        let seeds = match collect_seeds(&PathBuf::from(SEEDS_ROOT).join(target.seeds_dir)) {
            Ok(s) if !s.is_empty() => s,
            _ => {
                eprintln!(
                    "xtask fuzz: no seeds for {} — synthesizing one empty seed",
                    target.name
                );
                vec![Vec::new()]
            }
        };

        let target_start = Instant::now();
        let mut iters = 0u64;
        let mut failures_for_target = 0usize;
        let mut rng = SplitMix64::new(seed_for(target.name));
        eprintln!(
            "xtask fuzz: {} ({}s budget, {} seed{})",
            target.name,
            duration_secs,
            seeds.len(),
            if seeds.len() == 1 { "" } else { "s" }
        );

        while target_start.elapsed() < budget {
            // Pick a base seed and mutate it.
            let base = &seeds[rng.gen_range(seeds.len())];
            let input = mutate(base, &mut rng);
            iters += 1;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (target.harness)(&input);
            }));
            if result.is_err() {
                failures_for_target += 1;
                let crash_name = format!(
                    "{}-{}.bin",
                    target.name,
                    chrono_like_id(target_start.elapsed())
                );
                crash_dir.push(&crash_name);
                let _ = std::fs::write(&crash_dir, &input);
                eprintln!("  ✗ panic on input written to {}", crash_dir.display());
                crash_dir.pop();
                // Cap per-target reports so a wide bug doesn't drown the
                // console.
                if failures_for_target >= 10 {
                    eprintln!("  (10 panics — moving on to the next target)");
                    break;
                }
            }
        }
        total_iters += iters;
        total_failures += failures_for_target;
        eprintln!(
            "  ↳ {} iters in {:.1}s, {} panic{}",
            iters,
            target_start.elapsed().as_secs_f64(),
            failures_for_target,
            if failures_for_target == 1 { "" } else { "s" },
        );
    }

    eprintln!(
        "xtask fuzz: {total_iters} total iters, {total_failures} total panic{}",
        if total_failures == 1 { "" } else { "s" }
    );
    if total_failures > 0 {
        eprintln!("xtask fuzz: crashing inputs saved under target/fuzz-crashes/");
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

// ───────────────────────────── PRNG ──────────────────────────────────
//
// SplitMix64 from Vigna's paper. Tiny, fast, deterministic — perfect
// for fuzzing where reproducibility matters.

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn gen_range(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        // Truncating cast is fine: we immediately mod by `max`, and
        // `usize` on every supported target is at least 32 bits, so the
        // distribution still covers the full output range.
        #[allow(clippy::cast_possible_truncation)]
        let v = self.next_u64() as usize;
        v % max
    }

    fn gen_byte(&mut self) -> u8 {
        (self.next_u64() & 0xff) as u8
    }
}

fn seed_for(target_name: &str) -> u64 {
    // Hash the target name into the seed so each target gets a
    // distinct mutation stream but every run is reproducible.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a offset
    for b in target_name.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

// ─────────────────────────── mutators ────────────────────────────────
//
// A small mutation grab-bag. Each call picks one strategy uniformly.
// The strategies are deliberately simple — splice, flip, insert,
// truncate, expand — but the union covers most shallow parser bugs.

fn mutate(input: &[u8], rng: &mut SplitMix64) -> Vec<u8> {
    let mut buf = input.to_vec();
    // Apply 1–4 mutations.
    let n = (rng.next_u64() % 4) + 1;
    for _ in 0..n {
        match rng.next_u64() % 8 {
            0 => byte_flip(&mut buf, rng),
            1 => byte_overwrite(&mut buf, rng),
            2 => insert_byte(&mut buf, rng),
            3 => delete_byte(&mut buf, rng),
            4 => truncate(&mut buf, rng),
            5 => duplicate_chunk(&mut buf, rng),
            6 => insert_special(&mut buf, rng),
            _ => splice(&mut buf, rng),
        }
    }
    // Cap absolute size — the parsers we target don't need megabyte
    // inputs and runaway growth wastes the wall-clock budget.
    if buf.len() > 16 * 1024 {
        buf.truncate(16 * 1024);
    }
    buf
}

fn byte_flip(buf: &mut [u8], rng: &mut SplitMix64) {
    if buf.is_empty() {
        return;
    }
    let idx = rng.gen_range(buf.len());
    let bit = (rng.next_u64() & 7) as u8;
    buf[idx] ^= 1 << bit;
}

fn byte_overwrite(buf: &mut [u8], rng: &mut SplitMix64) {
    if buf.is_empty() {
        return;
    }
    let idx = rng.gen_range(buf.len());
    buf[idx] = rng.gen_byte();
}

fn insert_byte(buf: &mut Vec<u8>, rng: &mut SplitMix64) {
    let idx = rng.gen_range(buf.len() + 1);
    buf.insert(idx, rng.gen_byte());
}

fn delete_byte(buf: &mut Vec<u8>, rng: &mut SplitMix64) {
    if buf.is_empty() {
        return;
    }
    let idx = rng.gen_range(buf.len());
    buf.remove(idx);
}

fn truncate(buf: &mut Vec<u8>, rng: &mut SplitMix64) {
    if buf.is_empty() {
        return;
    }
    let new_len = rng.gen_range(buf.len() + 1);
    buf.truncate(new_len);
}

fn duplicate_chunk(buf: &mut Vec<u8>, rng: &mut SplitMix64) {
    if buf.is_empty() {
        return;
    }
    let start = rng.gen_range(buf.len());
    let len = rng.gen_range(buf.len() - start) + 1;
    let chunk: Vec<u8> = buf[start..start + len].to_vec();
    let dst = rng.gen_range(buf.len() + 1);
    for (i, b) in chunk.iter().enumerate() {
        buf.insert(dst + i, *b);
    }
}

fn insert_special(buf: &mut Vec<u8>, rng: &mut SplitMix64) {
    // Inject a byte from a "interesting values" pool: control chars,
    // structural tokens, BOM markers, high bits.
    const INTERESTING: &[u8] = &[
        0x00, 0x01, 0x07, 0x0a, 0x0d, 0x1b, 0x22, 0x27, 0x28, 0x29, 0x2c, 0x2e, 0x3a, 0x3b, 0x5b,
        0x5c, 0x5d, 0x7b, 0x7c, 0x7d, 0x7f, 0x80, 0xc0, 0xff,
    ];
    let v = INTERESTING[rng.gen_range(INTERESTING.len())];
    let idx = rng.gen_range(buf.len() + 1);
    buf.insert(idx, v);
}

fn splice(buf: &mut Vec<u8>, rng: &mut SplitMix64) {
    if buf.len() < 2 {
        return;
    }
    let mid = rng.gen_range(buf.len());
    let (head, tail) = buf.split_at(mid);
    let mut out = Vec::with_capacity(buf.len());
    out.extend_from_slice(tail);
    out.extend_from_slice(head);
    *buf = out;
}

// ────────────────────────── seed loader ──────────────────────────────

fn collect_seeds(dir: &Path) -> std::io::Result<Vec<Vec<u8>>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            out.push(std::fs::read(entry.path())?);
        }
    }
    Ok(out)
}

fn chrono_like_id(elapsed: Duration) -> String {
    // Truncating to u64 is fine — this is a filename suffix for crash
    // dumps, not a timestamp anyone parses back.
    #[allow(clippy::cast_possible_truncation)]
    let nanos = elapsed.as_nanos() as u64;
    format!("{nanos:08x}")
}

// Per-target harness functions are imported from
// `betterhook::fuzz_harnesses` at the top of this file, so this xtask
// and `apps/betterhook/afl/src/bin/*.rs` always exercise identical
// code paths.
