# betterhook

**A fast, memory-tight, worktree-native git hooks manager — built for the era of parallel AI coding agents.**

`betterhook` replaces [lefthook](https://lefthook.dev) for teams and tooling where multiple coding agents (Claude Code, Cursor, Codex, Aider, ...) run in parallel via [Conductor](https://conductor.build) or similar harnesses, each in its own git worktree. It's a single static Rust binary with ~30 ms binary start (~50 ms no-op hook run), line-streaming subprocess I/O, and an opt-in coordinator daemon that serializes tool conflicts across worktrees.

```sh
betterhook init           # scaffold betterhook.toml
betterhook install        # write worktree-aware wrapper into .git/hooks
git commit -am "go"       # your hook runs, per-worktree, correctly
```

---

## Contents

- [Why betterhook](#why-betterhook)
- [Feature comparison](#feature-comparison)
- [Quickstart](#quickstart-60-seconds)
- [Configuration](#configuration)
- [Agent integration](#agent-integration)
- [Commands](#commands)
- [Exit codes](#exit-codes)
- [Environment variables](#environment-variables)
- [Architecture](#architecture)
- [Development](#development)
- [Documentation](#documentation)
- [License](#license)

---

## Why betterhook

The workflow most teams are moving to: multiple coding agents running in parallel, each on its own branch in a separate git worktree, each opening its own PR. Every agent's pre-commit hook runs formatters, linters, and tests. Today they trip over each other.

### lefthook breaks under this load

| Pain point | Issue |
|---|---|
| `lefthook install` fails with exit 128 inside linked worktrees | [#901](https://github.com/evilmartians/lefthook/issues/901) |
| Remote config clone corrupts the index from a worktree | [#962](https://github.com/evilmartians/lefthook/issues/962) |
| `GIT_DIR` env pollution leaks into subprocess calls | [#1265](https://github.com/evilmartians/lefthook/issues/1265) |
| Go's `os/exec` buffers entire subprocess output in memory → OOM at scale | — |
| `parallel: true` silently ignores `priority` ordering | [#846](https://github.com/evilmartians/lefthook/issues/846) |
| Untracked files trip formatter hooks with false positives | [#833](https://github.com/evilmartians/lefthook/issues/833) |
| 100x regression in v1.6.18 | [#764](https://github.com/evilmartians/lefthook/discussions/764) |

### betterhook fixes all of these

- **Worktree-native dispatch.** A single byte-identical wrapper lives in the shared `.git/hooks/` dir. At commit time it runs `git rev-parse --show-toplevel`, resolves the current worktree, and dispatches to *that* worktree's own `betterhook.toml`. Every worktree runs its own config through the same wrapper — the exact property lefthook cannot provide.
- **Streaming, not buffered.** Every subprocess line goes through a Tokio multiplexer the instant it's emitted. Output renders live; memory stays constant regardless of how chatty a job gets.
- **Capability DAG scheduler.** Jobs declare `reads`, `writes`, `network`, `concurrent_safe`. The runner builds a real dependency graph and runs every root in parallel — only conflicting pairs serialize.
- **Content-addressable hook cache.** A cache hit skips the subprocess entirely and replays captured output events. Keyed on `blake3(content) + blake3(tool_binary) + blake3(args)` with mise/nvm-aware tool resolution.
- **Speculative execution.** The coordinator daemon watches the worktree, prewarms `concurrent_safe` jobs on save, and the commit-time runner just hits the cache.
- **12 builtin linter wrappers.** `builtin = "rustfmt"` in your config merges defaults and parses the tool's native output into structured `Diagnostic` events on the NDJSON stream.
- **Coordinator daemon** (opt-in). Per-tool mutexes, sharded semaphores, and automatic `CARGO_TARGET_DIR` injection so concurrent cargo builds in sibling worktrees never collide. Persistent via launchd (macOS) or systemd (Linux). Falls back to `fs4` advisory flock when the daemon can't start.
- **Built-in agent affordances.** NDJSON `--json` output, `betterhook status`, `betterhook explain --format dot|svg`, `betterhook fix`, a stable exit-code contract, and machine-readable errors.
- **`betterhook doctor`** health check. Walks the install manifest, config parse, builtin tools on PATH, cache writability, watcher health, orphan stashes, and conflicting `core.hooksPath`.
- **`betterhook import --from <lefthook|husky|hk|pre-commit>`** converts another hook manager's config in one command.

---

## Feature comparison

|                                               | betterhook | lefthook    | husky    | pre-commit |
|-----------------------------------------------|:----------:|:-----------:|:--------:|:----------:|
| Worktree-aware install                        | yes        | **no**      | no       | no         |
| Capability DAG scheduler                      | yes        | no          | no       | no         |
| Content-addressable hook cache                | yes        | no          | no       | no         |
| Speculative on-save pre-run                   | yes        | no          | no       | no         |
| 12 builtin structured linter wrappers         | yes        | no          | no       | partial    |
| Line-streaming subprocess output              | yes        | buffered    | partial  | buffered   |
| Cross-worktree tool coordinator               | yes (opt-in) | **no**    | no       | no         |
| Automatic `CARGO_TARGET_DIR` per worktree     | yes        | no          | no       | no         |
| Untracked-file stash safety                   | yes        | **broken** (#833) | no | via script  |
| NDJSON + structured diagnostics for agents    | yes        | no          | no       | no         |
| Multi-format config (TOML + YAML + KDL + JSON)| yes        | YAML only   | JS only  | YAML only  |
| Import from other tools                       | 4 sources  | no          | no       | no         |
| Health check (`doctor`)                       | yes        | no          | no       | no         |
| Binary size                                    | ~6 MB      | ~15 MB      | (node)   | (python)   |
| Cold start                                     | ~30 ms     | ~100 ms     | slower   | slowest    |
| Runtime required                               | none       | none        | Node.js  | Python     |

---

## Quickstart (60 seconds)

```sh
# 1. Install the binary (from source)
git clone https://github.com/leonardomso/betterhook && cd betterhook
cargo install --path apps/cli

# 2. Drop a starter config into your repo
cd my-repo
betterhook init

# 3. Install worktree-aware hook wrappers into .git/hooks
betterhook install

# 4. Verify
betterhook status | jq
```

That's it. Your next `git commit` will run the jobs defined in `betterhook.toml`, with streaming output, per-worktree config resolution, and (if declared) the coordinator daemon serializing tool conflicts across sibling worktrees.

### Already using lefthook?

```sh
betterhook import --from lefthook.yml
# writes: betterhook.toml, BETTERHOOK_MIGRATION_NOTES.md
betterhook install --takeover    # unset lefthook's core.hooksPath
```

### Already using husky or pre-commit?

```sh
betterhook install --takeover    # refuses unless you pass it
```

---

## Configuration

`betterhook.toml` (or `betterhook.yml` / `betterhook.kdl` / `betterhook.json` — all four parse into the same internal AST):

```toml
[meta]
version = 1

[hooks.pre-commit]
parallel = true
fail_fast = false
# Priority order for the parallel scheduler. Higher-priority (earlier
# in the list) jobs are dispatched first when the semaphore is full.
priority = ["fmt", "lint", "test"]
# Stash untracked files before running so formatters don't see them.
# Default: true for pre-commit.
stash_untracked = true

[hooks.pre-commit.jobs.fmt]
run = "prettier --write {staged_files}"
fix = "prettier --write {files}"       # used by `betterhook fix`
glob = ["*.ts", "*.tsx", "*.css"]
exclude = ["**/*.gen.ts"]
stage_fixed = true                      # re-stage files the job modified
isolate = "prettier"                    # global prettier mutex across worktrees
timeout = "60s"

[hooks.pre-commit.jobs.lint]
run = "eslint --cache --fix {staged_files}"
glob = ["*.ts", "*.tsx"]
isolate = "eslint"                      # serialize eslint across worktrees
env = { NODE_OPTIONS = "--max-old-space-size=2048" }

[hooks.pre-commit.jobs.test]
run = "cargo test --quiet"
# Per-worktree CARGO_TARGET_DIR is injected automatically so parallel
# cargo builds in sibling worktrees never collide on target/.
isolate = { tool = "cargo", target_dir = "per-worktree" }
timeout = "5m"

[hooks.pre-push.jobs.audit]
run = "cargo audit"

# Inherit shared defaults from another file. Cross-format extends works.
extends = ["./.betterhook/base.toml"]
```

### Template variables

| Variable            | Expands to                                                          |
|---------------------|---------------------------------------------------------------------|
| `{staged_files}`    | `git diff --name-only --cached -z`                                  |
| `{push_files}`      | `git diff --name-only -z <remote-ref>...HEAD` (for pre-push)        |
| `{all_files}`       | `git ls-files -z`                                                   |
| `{files}`           | The glob-filtered subset of whichever file set is active            |

All file sets are parsed from NUL-delimited git output, so filenames with spaces, unicode, or leading dashes round-trip correctly. Long lists are automatically chunked across multiple invocations to stay under `ARG_MAX`.

### Isolation (coordinator lock)

| `isolate =` shape                                               | What it does                                                  |
|-----------------------------------------------------------------|---------------------------------------------------------------|
| `"eslint"`                                                      | Global mutex for "eslint" across every worktree of this repo  |
| `{ name = "tsc", slots = 4 }`                                   | Sharded semaphore: up to 4 concurrent `tsc` invocations       |
| `{ tool = "cargo", target_dir = "per-worktree" }`               | Per-worktree key (never contends) + auto-injected env var     |

### Local overrides

Create `betterhook.local.toml` (gitignored) next to your main config. It's merged on top with highest precedence — useful for one-off per-machine timeouts, skipping slow jobs, or pointing `isolate` at a custom daemon socket.

---

## Agent integration

betterhook was designed for AI coding agents that need to reason about hook state programmatically. Every agent-facing surface produces parseable output and stable exit codes.

### Machine-readable output

```sh
betterhook run pre-commit --json
```

emits one NDJSON event per line:

```json
{"kind":"job_started","hook":"pre-commit","job":"lint","cmd":"eslint a.ts"}
{"kind":"line","job":"lint","stream":"stdout","line":"a.ts: clean"}
{"kind":"job_finished","job":"lint","exit":0,"duration":"312ms"}
{"kind":"summary","ok":true,"jobs_run":3,"jobs_skipped":0,"total":"890ms"}
```

Agents filter on `kind == "job_finished"` for pass/fail, `kind == "line"` for live logs, and `kind == "summary"` for the final verdict.

### Self-correction loop

When a formatter hook fails, an agent can auto-correct and retry:

```sh
betterhook run pre-commit --json
# exit 1, fmt failed
betterhook fix --hook pre-commit      # runs each job's `fix = ...` variant
git add -u
betterhook run pre-commit --json      # retry
```

### Status introspection

```sh
betterhook status
```

returns a JSON snapshot of installed hooks, SHA integrity, the resolved config, worktree identity, and (when running) the daemon socket path. Agents can check whether a worktree is set up before even attempting a commit.

### Dry-run planning

```sh
betterhook run pre-commit --dry-run
betterhook explain --hook pre-commit --job lint
```

Both return JSON plans — which jobs will run, which files they'd see, what env vars they'd get — without actually executing anything.

---

## Commands

| Command                                                | Purpose                                                              |
|--------------------------------------------------------|----------------------------------------------------------------------|
| `betterhook init [--path] [--force]`                   | Scaffold a starter `betterhook.toml`                                 |
| `betterhook install [--hook] [--takeover]`             | Write worktree-aware wrappers into `.git/hooks/`                     |
| `betterhook uninstall`                                 | Remove wrappers whose SHA matches what betterhook wrote              |
| `betterhook status [--worktree]`                       | JSON snapshot for agent introspection                                |
| `betterhook run <hook> [--dry-run] [--json] [--skip] [--only]` | Run a hook directly                                          |
| `betterhook explain --hook <name> [--job <n>]`         | Print a job's resolved plan without executing                        |
| `betterhook fix [--hook] [--job]`                      | Run every job's `fix = ...` variant (auto-format mode)               |
| `betterhook import --from <lefthook\|husky\|hk\|pre-commit>` | Import config from another hooks tool                          |

The installed wrapper dispatches to an internal `__dispatch` subcommand that's hidden from `--help`.

---

## Exit codes

Stable contract — agents can rely on these across releases.

| Code | Meaning                                                              |
|-----:|----------------------------------------------------------------------|
|  `0` | All jobs ok                                                          |
|  `1` | At least one job failed                                              |
|  `2` | Config parse/schema error                                            |
|  `3` | Lock acquisition timeout                                             |
|  `4` | Git error (e.g. stash pop conflict, unexpected `git` failure)        |
|  `5` | Install/uninstall error                                              |
| `64` | Usage error (from clap)                                              |
|`124` | Per-job timeout expired (matches GNU `timeout(1)`)                   |
|`130` | Interrupted (SIGINT)                                                 |

---

## Environment variables

| Variable                  | Purpose                                                         |
|---------------------------|-----------------------------------------------------------------|
| `BETTERHOOK_SKIP=a,b`     | Comma-separated job names to skip for this run                  |
| `BETTERHOOK_ONLY=a,b`     | Comma-separated allowlist (overrides everything else)           |
| `BETTERHOOK_NO_LOCKS=1`   | Bypass the daemon and file-lock fallback entirely               |
| `BETTERHOOK_DAEMON_SOCK`  | Explicit path to the coordinator daemon socket (skips discovery)  |
| `BETTERHOOK_HOOK`         | Set by betterhook in every job's env to the current hook name     |

---

## Architecture

### Repo layout

```
betterhook/
├── apps/
│   ├── betterhook/        # library crate (config, runner, cache, daemon, builtins)
│   │   ├── src/
│   │   │   ├── config/    # multi-format parser, AST, extends, migrator
│   │   │   ├── git/       # worktree introspection, fileset, stash
│   │   │   ├── runner/    # tokio executor, output multiplexer
│   │   │   ├── lock/      # daemon client + flock fallback + protocol
│   │   │   ├── daemon/    # betterhookd server (unix socket, bincode)
│   │   │   ├── install/   # wrapper script, SHA manifest
│   │   │   ├── dispatch.rs
│   │   │   └── status.rs
│   │   ├── benches/       # criterion benches
│   │   ├── tests/         # integration + linked-worktree tests
│   │   └── fuzz/          # cargo-fuzz targets
│   └── cli/               # the `betterhook` CLI (thin clap frontend)
├── xtask/                 # bench + stress + fuzz harness
├── packaging/             # Homebrew formula + npm wrapper scaffolds
├── Cargo.toml             # cargo workspace
└── package.json           # root scripts
```

### How a hook actually fires

1. You run `git commit`.
2. Git executes `.git/hooks/pre-commit` (the wrapper we installed).
3. The wrapper runs `git rev-parse --show-toplevel` and captures the **current** worktree — even though the wrapper itself lives in the shared common dir.
4. The wrapper `exec`s into `betterhook __dispatch --hook pre-commit --worktree <that-path>`.
5. betterhook walks `<that-path>/betterhook.{toml,yml,yaml,json}`, loads it, resolves `extends` and the local override, and hits the AST cache if the content hash hits.
6. If the hook has jobs, the runner spawns them (sequential or parallel), streams output line by line through the multiplexer, acquires coordinator locks for any job with `isolate = ...`, applies `stage_fixed`, and reports.
7. Non-zero exit on any job blocks the commit.

Steps 3–5 are the part lefthook cannot get right. See the [protocol reference](https://betterhook.dev/reference/protocol) for the daemon wire format and [`CHANGELOG.md`](CHANGELOG.md) for the implementation history.

### Performance targets

| Metric                                        | Target                                     |
|------------------------------------------------|--------------------------------------------|
| Cold start `betterhook --version`             | `< 30 ms`                                  |
| Cold start `run pre-commit` (no-op config)    | `< 50 ms`                                  |
| Daemon idle RSS                                | `< 8 MB`                                   |
| Peak runner RSS for 8 parallel jobs            | `< 30 MB` (excluding subprocess RSS)        |
| Wrapper overhead per hook fire                 | `< 5 ms`                                   |
| Stripped binary size (macOS arm64)             | `< 6 MB`                                   |
| 8 worktrees committing concurrently            | Linear scaling, no quadratic memory growth |
| Output multiplexer overhead                    | `< 1 µs/line`                              |

Run `cargo run -p xtask -- bench` to measure on your machine.

---

## Development

Rust 1.86+ and standard git. [Bun](https://bun.sh) is used for the docs site only.

```sh
# Build + test + lint
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# Docs site (requires bun)
cd apps/docs && bun install && bun run dev

# Benchmarks (criterion + hyperfine)
cargo run -p xtask -- bench

# Fuzz (nightly toolchain + cargo-fuzz)
cargo install cargo-fuzz
cd apps/betterhook/fuzz
cargo +nightly fuzz run config_parse
cargo +nightly fuzz run wrapper_paths
```

All commits follow [conventional commits](https://www.conventionalcommits.org/). See [CHANGELOG.md](CHANGELOG.md) for the full build history.

---

## Documentation

- [betterhook.dev](https://betterhook.dev) — full Mintlify docs (commands, architecture, reference)
- [`CHANGELOG.md`](CHANGELOG.md) — release history + known gaps
- `betterhook --help` — per-subcommand reference

---

## License

MIT — see [`LICENSE`](LICENSE).

---

<sub>Built for the workflow where multiple coding agents ship code in parallel. If that's not you, you probably want [lefthook](https://lefthook.dev) — it's great for single-developer repos and we've learned a lot from it.</sub>
