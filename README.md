# betterhook

> Memory-efficient, worktree-native git hooks manager built for the AI agent era.

`betterhook` is a [lefthook](https://lefthook.dev)-style git hooks manager designed for the workflow where multiple AI coding agents (Claude Code, Cursor, Codex, Aider, …) run in parallel via [Conductor](https://conductor.build), each in its own git worktree.

## The pitch

One wrapper script installed once into the shared `.git/hooks/` dir. At commit time, `git rev-parse --show-toplevel` resolves the current worktree, and betterhook dispatches to **that worktree's own** `betterhook.toml`. Every worktree runs its own config through a single byte-identical wrapper. This is the property lefthook fails to provide and is the headline reason betterhook exists.

### What's wrong with lefthook today

- `lefthook install` fails with exit 128 inside linked worktrees ([#901](https://github.com/evilmartians/lefthook/issues/901)).
- Remote config clone corrupts the git index when invoked from a worktree ([#962](https://github.com/evilmartians/lefthook/issues/962)).
- Go's `os/exec` buffers entire subprocess stdout/stderr in memory; under 4 agents × N parallel jobs across 4 worktrees this regularly OOMs.
- Parallel execution ignores priority ordering ([#846](https://github.com/evilmartians/lefthook/issues/846)).
- Untracked files trip formatter hooks with false positives ([#833](https://github.com/evilmartians/lefthook/issues/833)).
- ESLint cache, `cargo target/`, prettier on `pnpm-lock.yaml`, `.tsbuildinfo` — all corrupt under concurrent worktree runs, with no coordinator anywhere in the stack.

### What betterhook ships

- **Worktree-aware wrapper** installed once into the shared common dir, dispatches at runtime per worktree.
- **Line-streaming subprocess I/O** via Tokio — output renders live, memory stays constant regardless of how chatty the subprocess is.
- **Priority-aware parallel scheduler** that actually respects `priority = [...]` under contention.
- **Untracked stash safety** with unique-message verification on pop.
- **`stage_fixed`** via before/after unstaged-file snapshots.
- **Per-job timeouts** with clean SIGKILL escalation and exit code 124.
- **Cancellation token** shared across parallel jobs so `fail_fast` actually aborts in-flight children (not just the task futures).
- **Opt-in coordinator daemon** (`betterhookd`) exposing cross-worktree tool locks over a tiny Unix-socket bincode protocol; falls back to `fs4` advisory flock when the daemon can't start.
- **Auto-injection of `CARGO_TARGET_DIR`** for `isolate = { tool = "cargo", target_dir = "per-worktree" }` so concurrent cargo builds in sibling worktrees never collide.
- **Multi-format config** — TOML, YAML, or JSON, all lowering to one canonical AST.
- **NDJSON `--json` output** and stable exit-code contract for agent parsing.
- **`betterhook migrate`** — best-effort converter from `lefthook.yml`.
- **Single static binary** — about 6 MB on macOS arm64, ~50 ms cold start.

## Install

```sh
# macOS / Linux
cargo install --path apps/cli  # until we publish to crates.io
cd your-repo
betterhook init
betterhook install
```

`betterhook install` writes the wrapper into `<common-dir>/hooks/<hookname>` for every hook type declared in your `betterhook.toml`. If you're using husky or pre-commit and they've claimed `core.hooksPath`, pass `--takeover` to replace it.

## Configuration

`betterhook.toml`:

```toml
[meta]
version = 1

[hooks.pre-commit]
parallel = true
priority = ["fmt", "lint", "test"]

[hooks.pre-commit.jobs.fmt]
run = "prettier --write {staged_files}"
fix = "prettier --write {files}"
glob = ["*.ts", "*.tsx"]
exclude = ["**/*.gen.ts"]
stage_fixed = true
isolate = "prettier"
timeout = "60s"

[hooks.pre-commit.jobs.lint]
run = "eslint --cache --fix {staged_files}"
glob = ["*.ts", "*.tsx"]
isolate = "eslint"

[hooks.pre-commit.jobs.test]
run = "cargo test --quiet"
isolate = { tool = "cargo", target_dir = "per-worktree" }
```

Supports TOML, YAML, or JSON — the parser auto-detects by extension. `betterhook.local.{toml,yml,json}` is merged on top as a gitignored override.

## Agent-facing commands

- `betterhook status` — JSON snapshot of installed hooks, config, worktree identity.
- `betterhook run <hook> --dry-run --json` — resolved plan without executing.
- `betterhook explain --hook pre-commit --job lint` — what would run for a single job.
- `betterhook fix [--hook pre-commit]` — run every job's `fix` variant. Use this when a formatter hook fails and the agent wants to auto-correct.
- `betterhook migrate --from lefthook.yml --to betterhook.toml` — converter + migration notes.

## Exit codes

| Code | Meaning                        |
|------|--------------------------------|
| 0    | all jobs ok                    |
| 1    | at least one job failed        |
| 2    | config parse/schema error      |
| 3    | lock acquisition timeout       |
| 4    | git error (stash pop, etc.)    |
| 5    | install/uninstall error        |
| 64   | usage error (from clap)        |
| 124  | per-job timeout expired        |
| 130  | interrupted (SIGINT)           |

## Environment variables

| Variable                 | Purpose                                                    |
|--------------------------|------------------------------------------------------------|
| `BETTERHOOK_SKIP=lint,x` | Comma-separated job names to skip                          |
| `BETTERHOOK_ONLY=lint`    | Comma-separated allowlist                                  |
| `BETTERHOOK_NO_LOCKS`    | Bypass the daemon and file locks                           |
| `BETTERHOOK_DAEMON_SOCK` | Explicit socket path (skips auto-discovery/spawn)         |

## Repo layout

```
betterhook/
├── apps/
│   ├── betterhook/     # core library + `betterhookd` binary
│   └── cli/            # the `betterhook` CLI
├── xtask/              # bench, stress, lefthook-compat harness
├── Cargo.toml          # cargo workspace
├── turbo.json          # turborepo pipeline
└── package.json        # turbo root
```

Library: `apps/betterhook/src/`

- `config/` — multi-format parser (TOML + YAML + JSON), typed AST, extends inheritance, lefthook migrator
- `git/` — worktree introspection (`rev-parse`, `worktree list`), `-z` fileset computation, stash safety
- `runner/` — Tokio executor (sequential + parallel), line-streaming subprocess wrapper, output multiplexer (TTY + NDJSON)
- `lock/` — coordinator client, protocol, `fs4` flock fallback
- `daemon/` — `betterhookd` server (Unix socket, bincode, lock registry)
- `install/` — wrapper script rendering, SHA-verified install/uninstall, `installed.json` manifest
- `dispatch.rs` — runtime config resolution for the wrapper
- `status.rs` — agent introspection

## Development

```sh
pnpm install
pnpm run build      # turbo → cargo build
pnpm run test
pnpm run lint       # clippy -D warnings
cargo fmt --all

# benchmarks
cargo run -p xtask -- bench

# fuzz (nightly toolchain required)
cd apps/betterhook/fuzz
cargo +nightly fuzz run config_parse
```

## Documentation

- [docs/protocol.md](docs/protocol.md) — daemon IPC wire format (for Conductor and third-party agent harness integration)
- [man/betterhook.1](man/betterhook.1) — man page (exit codes, env vars, subcommand reference)
- [CHANGELOG.md](CHANGELOG.md) — release history

## License

MIT — see [LICENSE](LICENSE).
