<p align="center">
  <img src=".github/header.jpg" alt="betterhook" width="100%" />
</p>

<h1 align="center">Contributing to betterhook</h1>

<p align="center">
  <strong>Welcome — here's how to land your first PR in under an hour.</strong>
</p>

<p align="center">
  <a href="https://github.com/leonardomso/betterhook/issues">Issues</a> ·
  <a href="https://github.com/leonardomso/betterhook/pulls">Pull Requests</a> ·
  <a href="CHANGELOG.md">Changelog</a> ·
  <a href="https://betterhook.dev">Docs</a>
</p>

---

## Your first PR in 5 minutes

```sh
# 1. Clone and build
git clone https://github.com/leonardomso/betterhook && cd betterhook
cargo build --workspace

# 2. Run the test suite (~600 tests, ~30s on a modern machine)
cargo test --workspace

# 3. Make your change. Then:
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# 4. Commit with a Conventional-Commits message
git commit -m "fix(runner): drop lock when timeout fires"

# 5. Push and open a PR against master
```

If all four checks above pass and your commit follows [Conventional Commits](https://www.conventionalcommits.org/), CI will be green and a maintainer will review.

## Prerequisites

| Tool | Version | Why |
|---|---|---|
| **Rust** | 1.86+ (edition 2024) | Workspace MSRV. Install via [rustup](https://rustup.rs). |
| **git** | 2.30+ | Worktree semantics we rely on. |
| **bun** | 1.x | Mintlify docs site only — skip if you're not touching `apps/docs/`. |

## Repository layout

```
apps/betterhook/   library crate — config parser, runner, cache, daemon, builtins
apps/cli/          CLI binary (thin clap wrapper over the library)
apps/docs/         Mintlify documentation site
xtask/             benchmarks, stress harness, fuzz runner
recipes/           drop-in betterhook.toml configs for common stacks
packaging/         Homebrew formula + npm wrapper scaffolds
```

## Build, test, lint

```sh
cargo build --workspace                                  # compile everything
cargo test --workspace                                   # ~600 tests, ~30 seconds
cargo clippy --workspace --all-targets -- -D warnings    # lint (pedantic, deny warnings)
cargo fmt --all -- --check                               # formatting check
```

All four must pass before pushing. CI runs the same matrix on Ubuntu and macOS.

The docs site:

```sh
cd apps/docs && bun install
bun run dev                                              # local Mintlify preview
```

## Benchmarks

```sh
cargo build --release -p betterhook-cli
PATH="$PWD/target/release:$PATH" cargo xtask bench-monorepo
```

Generates a synthetic 10,000-file repo and writes `target/bench-results.md` comparing betterhook against `lefthook` and `hk` (whichever are on `PATH`). Run before any change touching the runner, dispatch, or config loader to catch perf regressions.

<details>
<summary><strong>Fuzzing</strong></summary>

```sh
cargo build --release -p xtask
./target/release/xtask fuzz --duration 30   # 30s per target, ~3 min total
./target/release/xtask fuzz-smoke           # fast seed-corpus check
```

The fuzz harness covers the config parser (TOML/YAML/KDL/JSON), the importer (lefthook/husky/hk/pre-commit), and the bincode wire protocol. Add a target by dropping a `fuzz_target_<name>.rs` into `apps/betterhook/fuzz/` and registering it in `xtask/src/fuzz.rs`.
</details>

<details>
<summary><strong>Mutation testing</strong></summary>

We use [`cargo-mutants`](https://github.com/sourcefrog/cargo-mutants) to find logic that's reached by tests but not meaningfully asserted. Start narrow:

```sh
cargo install --locked cargo-mutants

cargo mutants -p betterhook -f apps/betterhook/src/dispatch.rs -o /tmp/mutants-dispatch
# add tests for survivors, then:
cargo mutants -p betterhook -f apps/betterhook/src/dispatch.rs --iterate -o /tmp/mutants-dispatch
```

Use survivors to drive test additions, rerun with `--iterate` until the remaining survivors are intentional or impractical.
</details>

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/), lowercase types, imperative summary, scope when useful:

```
feat(runner): add DAG-aware parallel scheduler
fix(cache): move blocking I/O off the executor thread
docs(readme): restructure for scannability
test: e2e test for builtin diagnostic pipeline
refactor(builtins): extract shared parse helpers
chore(deps): bump fs4 to 1.1.0
ci: pin actions to exact latest versions
perf(dispatch): hook_for_match returns Cow<Hook>
```

**Never** add `Co-Authored-By` or AI-attribution lines.

## Pull requests

1. **Fork** and branch from `master`.
2. **Make your change.** One logical change per commit. Test additions live in the same commit as the behavior they cover.
3. **Run the four checks** locally (build, test, clippy, fmt).
4. **Open a PR** against `master`. CI runs on Ubuntu + macOS.
5. A maintainer reviews and merges.

Good PR descriptions include:
- **Summary** — what the change does in one sentence
- **Why** — context and motivation
- **Validation** — exact commands run, outcomes
- **Tests** — added or updated, and why

## What makes a good change

| Do | Don't |
|---|---|
| Confirm assumptions when ambiguous — open an issue first | Guess CLI behavior or config semantics |
| Follow existing module boundaries and naming patterns | Refactor unrelated code in the same PR |
| Keep changes small, focused, reversible | Ship behavior without tests |
| Add regression tests for bug fixes | Bypass failing tests, lint, or hooks |
| Stream subprocess output | Buffer subprocess output in memory |
| Test from a linked worktree, not just the primary checkout | Break worktree isolation |

## API stability

betterhook is **pre-release** (v0.x). The library API, config schema, and CLI flags may change between minor versions without deprecation cycles. Pin to an exact version if you depend on the library crate directly.

## License

By contributing, you agree your contributions will be licensed under the [MIT License](LICENSE).
