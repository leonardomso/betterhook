# CLAUDE.md

Operating guide for contributors and AI agents working on this repository. **Read this before making changes.**

If instructions conflict, use this priority order:
1. Direct user request
2. This file
3. Existing code patterns
4. Personal preference

The same content lives in `AGENTS.md` -- keep both files in sync.

---

## 1. What betterhook is

betterhook is a fast, memory-tight, worktree-native git hooks manager built for the era of parallel AI coding agents. It replaces lefthook for teams and tooling where multiple coding agents (Claude Code, Cursor, Codex, Aider, ...) run in parallel, each in its own git worktree. It's a single static Rust binary with ~30 ms binary start, ~50 ms no-op hook run, line-streaming subprocess I/O, and an opt-in coordinator daemon that serializes tool conflicts across worktrees.

Read first:
- `README.md`
- `apps/docs/introduction.mdx` -- feature overview
- `apps/docs/quickstart.mdx` -- 60-second setup
- `apps/docs/architecture/overview.mdx` -- internals and commit flow
- `apps/docs/why-betterhook.mdx` -- comparison with lefthook, husky, pre-commit
- `CONTRIBUTING.md` -- build, test, and PR workflow

---

## 2. Tech stack

- **Language**: Rust (edition 2024, MSRV 1.86), Cargo workspace
- **Async runtime**: `tokio` (full features) + `tokio-util` (codec)
- **CLI**: `clap` (derive)
- **Config parsing**: `toml`, `serde_yaml_ng`, `kdl`, `serde_json` (multi-format: TOML + YAML + KDL + JSON)
- **Error handling**: `miette` (fancy diagnostics), `thiserror` (typed errors)
- **Logging**: `tracing` + `tracing-subscriber` (JSON + env-filter)
- **Hashing**: `blake3` (content-addressable cache), `sha2`
- **File watching**: `notify`
- **File locking**: `fs4` (advisory flock fallback)
- **Serialization**: `bincode` (wire protocol), `serde`
- **Terminal output**: `owo-colors`
- **Glob matching**: `globset`
- **Tool resolution**: `which`
- **Testing**: `insta` (snapshot testing), `criterion` (benchmarks), `tempfile`
- **Lint**: `clippy` (pedantic)
- **Docs**: Mintlify (`apps/docs/`)

---

## 3. Repository layout

Cargo workspace with three members:

```
apps/betterhook/   Library crate -- config parser, runner, cache, daemon, builtins
apps/cli/          CLI binary -- thin clap wrapper over the library
apps/docs/         Mintlify documentation site
xtask/             Dev tooling: benchmarks, stress harness, fuzz runner
recipes/           Drop-in betterhook.toml configs for common stacks
packaging/         Homebrew formula + npm wrapper scaffolds
```

Inside `apps/betterhook/src/`:

| Module | Purpose |
|---|---|
| `config/` | Multi-format parser (TOML + YAML + KDL + JSON), typed AST, extends inheritance, importers |
| `runner/` | Tokio executor (sequential + parallel), line-streaming subprocess wrapper, output multiplexer |
| `cache/` | Content-addressable hook cache keyed on blake3 hashes |
| `daemon/` | Coordinator daemon -- Unix socket listener, lock registry, idle-exit lifecycle |
| `builtins/` | 12 builtin linter wrappers with structured diagnostic output |
| `git/` | Worktree introspection, NUL-delimited fileset computation, stash safety |
| `install/` | Wrapper script rendering, SHA-verified install/uninstall, manifest |
| `lock/` | Coordinator client, bincode wire protocol, fs4 flock fallback |
| `dispatch.rs` | Runtime config resolution for the hook wrapper |
| `status.rs` | Agent introspection (NDJSON output) |
| `error.rs` | Typed error definitions |

---

## 4. Features and where to read more

Map of capabilities. Each links to the doc that explains it -- read these instead of guessing.

**Core**
- Configuration (TOML + YAML + KDL + JSON) -- `apps/docs/essentials/configuration.mdx`
- Config formats -- `apps/docs/essentials/formats.mdx`
- Config extends / inheritance -- `apps/docs/essentials/extends.mdx`
- Config templates -- `apps/docs/essentials/templates.mdx`
- Worktree isolation -- `apps/docs/essentials/isolation.mdx`
- Monorepo support -- `apps/docs/essentials/monorepo.mdx`
- Installation -- `apps/docs/essentials/installation.mdx`

**Architecture**
- Architecture overview and commit flow -- `apps/docs/architecture/overview.mdx`
- Worktree model -- `apps/docs/architecture/worktree-model.mdx`
- Runner -- `apps/docs/architecture/runner.mdx`
- Capability DAG scheduler -- `apps/docs/architecture/capability-dag.mdx`
- Content-addressable cache -- `apps/docs/architecture/cache.mdx`
- Speculative execution -- `apps/docs/architecture/speculative.mdx`
- Coordinator daemon -- `apps/docs/architecture/daemon.mdx`

**Commands**
- Overview -- `apps/docs/commands/overview.mdx`
- `betterhook init` -- `apps/docs/commands/init.mdx`
- `betterhook install` -- `apps/docs/commands/install.mdx`
- `betterhook uninstall` -- `apps/docs/commands/uninstall.mdx`
- `betterhook run` -- `apps/docs/commands/run.mdx`
- `betterhook status` -- `apps/docs/commands/status.mdx`
- `betterhook explain` -- `apps/docs/commands/explain.mdx`
- `betterhook fix` -- `apps/docs/commands/fix.mdx`
- `betterhook doctor` -- `apps/docs/commands/doctor.mdx`
- `betterhook import` -- `apps/docs/commands/import.mdx`
- `betterhook cache` -- `apps/docs/commands/cache.mdx`
- `betterhook builtins` -- `apps/docs/commands/builtins.mdx`
- `betterhook migrate` -- `apps/docs/commands/migrate.mdx`

**Agent integration**
- Agent overview (NDJSON, self-correction, Conductor) -- `apps/docs/agents/overview.mdx`
- Conductor integration -- `apps/docs/agents/conductor.mdx`
- NDJSON protocol -- `apps/docs/agents/ndjson.mdx`
- Self-correction loop -- `apps/docs/agents/self-correction.mdx`

**Migration guides**
- From lefthook -- `apps/docs/migration/from-lefthook.mdx`
- From husky -- `apps/docs/migration/from-husky.mdx`
- From pre-commit -- `apps/docs/migration/from-pre-commit.mdx`

**Reference**
- Environment variables -- `apps/docs/reference/env-vars.mdx`
- Exit codes -- `apps/docs/reference/exit-codes.mdx`
- Performance -- `apps/docs/reference/performance.mdx`
- NDJSON protocol -- `apps/docs/reference/protocol.mdx`

If you have unresolved questions about scope, config schema, CLI behavior, or user-facing semantics after reading the relevant docs, **ask the user before implementing.** Do not assume.

---

## 5. Local setup

```bash
# Build
cargo build --workspace

# Run tests
cargo test --workspace

# Lint
cargo clippy --workspace --all-targets -- -D warnings

# Format check
cargo fmt --all -- --check

# Install locally
cargo install --path apps/cli
```

For the docs site:

```bash
cd apps/docs && bun install
bun run dev
```

---

## 6. Validation commands

Run before pushing:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Fuzzing (optional):

```bash
cargo build --release -p xtask
./target/release/xtask fuzz --duration 30
./target/release/xtask fuzz-smoke
```

---

## 7. Engineering rules (non-negotiable)

1. **Rust edition 2024.** MSRV 1.86.
2. **Async with Tokio.** All subprocess and I/O work goes through Tokio. No blocking in async context.
3. **Errors.** Use `miette` for user-facing errors, `thiserror` for internal typed errors. Provide helpful diagnostic messages.
4. **Clippy pedantic.** Treat warnings as errors. Don't suppress without justification.
5. **Snapshot tests.** Use `insta` for config parsing, output formatting, and diagnostic rendering tests.
6. **No emojis.** In code, comments, logs, docs, commits, PR text.
7. **Streaming, not buffered.** Subprocess output must stream line-by-line. Never buffer entire output in memory.
8. **Worktree-safe.** Every feature must work correctly from a linked git worktree, not just the primary checkout.
9. **Multi-format config.** Changes to the config model must work across all four formats (TOML, YAML, KDL, JSON).

---

## 8. Commits and PRs

### Conventional Commits (mandatory)

Format: `type(scope): summary`

Allowed types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`, `chore`, `revert`. Use `!` for breaking changes and explain in the body. Lowercase types, imperative summary, scope when useful.

Examples:
- `feat(runner): add DAG-aware parallel scheduler`
- `fix(cache): move blocking I/O off the executor thread`
- `test: e2e test for builtin diagnostic pipeline`

**Never** add:
- "Co-Authored-By" lines
- "Generated with Claude Code" or any AI attribution
- Vague messages (`update`, `misc`, `fix stuff`)

### PR descriptions

Substantive, not boilerplate. Include:
- **Summary** -- what the PR does in plain language
- **Why** -- context and motivation
- **What changed** -- grouped by area
- **Validation** -- exact commands run and outcomes
- **Tests added or updated** and why

---

## 9. DOs and DON'Ts

### Do
- Confirm assumptions when requirements are ambiguous.
- Follow existing module boundaries and naming patterns.
- Keep changes small, focused, and reversible.
- Add tests for new behavior; regression tests for bug fixes.
- Run the full validation suite before pushing.

### Don't
- Guess CLI behavior or config semantics.
- Refactor unrelated code in the same PR.
- Ship behavior without tests.
- Bypass failing tests, lint, or hooks.
- Buffer subprocess output in memory.
- Break worktree isolation.

---

## 10. Definition of done

A change is done only when:

1. `cargo build --workspace` succeeds.
2. `cargo test --workspace` passes.
3. `cargo clippy --workspace --all-targets -- -D warnings` is clean.
4. `cargo fmt --all -- --check` is clean.
5. Docs updated for behavior changes.
6. Summary provided: what changed, why, how validated.

---

When in doubt, prefer established project patterns over novelty, ask clarifying questions early, and keep changes explicit and verifiable.
