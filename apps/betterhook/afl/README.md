# betterhook-afl

[afl.rs](https://github.com/rust-fuzz/afl.rs) (American Fuzzy Lop)
fuzz harnesses for betterhook. Sister crate to `apps/betterhook/fuzz/`,
which uses libFuzzer / `cargo-fuzz` — afl is complementary, not a
replacement. The two engines find different bug shapes.

## Why both

| Engine    | Strength                                           | Weakness                          |
|-----------|----------------------------------------------------|------------------------------------|
| libFuzzer | Branch coverage, fast in-process iterations        | Can miss structured-input bugs    |
| afl.rs    | Mutates structured input well, persistent mode    | Slower per-iteration, needs corpus |

We keep cargo-fuzz for the high-volume coverage runs and use afl for
overnight runs against the structured parsers (config, JSON
diagnostics, husky shell scripts).

## One-time setup

```bash
cargo install afl
```

This builds the AFL runtime once. It's a stable-toolchain install.

## Running a target

Every harness lives at `src/bin/<target>.rs`. Each has a paired seed
corpus under `seeds/<target>/`. Build, then fuzz:

```bash
cd apps/betterhook/afl
cargo afl build --release

# Replace <target> with one of:
#   config_parse  dag_resolver  clippy_parser  eslint_parser
#   husky_importer  cache_key
cargo afl fuzz \
  -i seeds/<target> \
  -o out/<target> \
  target/release/<target>
```

`out/<target>/default/crashes/` collects any input that triggered a
panic. Reproduce locally with:

```bash
cargo run --release --bin <target> < out/<target>/default/crashes/id:000000,*
```

## Targets

| Target           | What it fuzzes                                                       |
|------------------|----------------------------------------------------------------------|
| `config_parse`   | All four formats: TOML, YAML, JSON, KDL                              |
| `dag_resolver`   | TOML → `RawConfig` → `lower()` → `build_dag` chain                   |
| `clippy_parser`  | `cargo --message-format=json` line parser                            |
| `eslint_parser`  | `eslint --format=json` parser (most deeply nested JSON shape)        |
| `husky_importer` | husky shell-script importer (`strip_runner`, `job_name_from`)        |
| `cache_key`      | `hash_bytes` + `args_hash` NUL-separated canonicalization            |

## Smoke test from CI

The repo includes `cargo xtask fuzz-smoke`, which runs every target
against the seed corpus plus an adversarial "interesting bytes" set.
It's a *bit-rot guard*, not a real fuzzing campaign.

## In-process random fuzzing (no install required)

For immediate exhaustive runs without installing the AFL runtime, the
repo also ships `cargo xtask fuzz`. It feeds the same harness
functions a random-mutation stream seeded from the same seed corpus
as the AFL targets, runs for a wall-clock budget you control, and
saves any crashing input under `target/fuzz-crashes/`.

```bash
cargo xtask fuzz                                # 60 s per target
cargo xtask fuzz --duration 120                 # 2 min per target
cargo xtask fuzz --target dag_resolver -d 600   # 10 min on one target
```

A 2-minute run hits >100 million parser invocations across the six
targets — enough to surface every shallow panic our test surface
covers. For coverage-guided campaigns past that depth, switch to the
real AFL path above.

## Workspace exclusion

This crate is intentionally **not** part of the cargo workspace
(`[workspace]` is empty in its `Cargo.toml`). That keeps
`cargo build --workspace` from trying to compile harnesses without
the AFL runtime built first, the same pattern `apps/betterhook/fuzz/`
already follows.
