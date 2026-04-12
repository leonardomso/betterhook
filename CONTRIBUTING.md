# Contributing to betterhook

Thanks for your interest in improving betterhook! This document covers
the basics: how to build, how to test, and how to get your changes
merged.

## Prerequisites

- **Rust 1.86+** (edition 2024). Install via [rustup](https://rustup.rs).
- **bun 10+** for the Mintlify docs and the Turbo monorepo scripts.
- **Node 20+** for the docs dev server.
- **git** (any recent version).

## Repository layout

```
apps/betterhook/   Rust library — config parser, runner, cache, daemon, builtins
apps/cli/          Rust CLI binary — thin wrapper over the library
apps/docs/         Mintlify documentation site
xtask/             Dev tooling: benchmarks, stress harness, fuzz runner
```

## Build and test

```bash
cargo build --workspace          # compile everything
cargo test --workspace           # run the full test suite (~220 tests)
cargo clippy --workspace --all-targets -- -D warnings   # lint
cargo fmt --all -- --check       # formatting check
```

The docs site:

```bash
bun install
bun run docs:dev                # local Mintlify preview
```

## Fuzzing

```bash
cargo build --release -p xtask
./target/release/xtask fuzz --duration 30   # 30s per target, ~3 min total
./target/release/xtask fuzz-smoke           # fast seed-corpus check
```

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(runner): add DAG-aware parallel scheduler
fix(cache): move blocking I/O off the executor thread
docs: update README for v0.0.2
test: e2e test for builtin diagnostic pipeline
refactor(builtins): extract shared parse helpers
chore: bump workspace version
ci: add release workflow
perf(dispatch): hook_for_match returns Cow<Hook>
```

## Pull requests

1. Fork the repo and create a branch from `master`.
2. Make your changes. Each commit should be a single logical change.
3. Run `cargo test --workspace` and `cargo clippy` locally before pushing.
4. Open a PR against `master`. The CI matrix runs on Ubuntu and macOS.
5. A maintainer will review and merge.

## API stability

Betterhook is **pre-release** (v0.0.x). The library API, config schema,
and CLI flags may change between minor versions without deprecation
cycles. Pin to an exact version if you depend on the library crate
directly.

## License

By contributing, you agree that your contributions will be licensed
under the MIT License.
