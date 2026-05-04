<p align="center">
  <img src=".github/header.jpg" alt="betterhook" width="100%" />
</p>

<h1 align="center">betterhook</h1>

<p align="center">
  <strong>Fast, worktree-native git hooks for the era of parallel AI coding agents.</strong>
</p>

<p align="center">
  <a href="https://github.com/leonardomso/betterhook/actions/workflows/ci.yml"><img src="https://github.com/leonardomso/betterhook/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="https://crates.io/crates/betterhook-cli"><img src="https://img.shields.io/crates/v/betterhook-cli?label=crates.io" alt="crates.io" /></a>
  <a href="https://www.npmjs.com/package/betterhook"><img src="https://img.shields.io/npm/v/betterhook?label=npm" alt="npm" /></a>
  <a href="https://github.com/leonardomso/betterhook/blob/master/LICENSE"><img src="https://img.shields.io/badge/license-MIT-green" alt="License" /></a>
  <img src="https://img.shields.io/badge/rust-1.86%2B-orange" alt="Rust 1.86+" />
  <img src="https://img.shields.io/badge/cold%20start-~30ms-brightgreen" alt="Cold start ~30ms" />
</p>

<p align="center">
  <a href="https://betterhook.dev">Docs</a> ·
  <a href="https://betterhook.dev/quickstart">Quickstart</a> ·
  <a href="https://crates.io/crates/betterhook-cli">crates.io</a> ·
  <a href="https://www.npmjs.com/package/betterhook">npm</a> ·
  <a href="CHANGELOG.md">Changelog</a>
</p>

---

A single static Rust binary that replaces lefthook, husky, and pre-commit when you're running multiple AI coding agents in parallel — each in its own [git worktree](https://git-scm.com/docs/git-worktree), each opening its own PR, all sharing one `.git` directory.

**4 config formats** · **12 builtin linters** · **~6 MB binary** · **~30 ms cold start** · **streaming I/O**

> **Already on lefthook, husky, hk, or pre-commit?** One command imports your config and takes over the hook wrappers — no rewrites:
> ```sh
> betterhook import --from lefthook.yml && betterhook install --takeover
> ```
> A `BETTERHOOK_MIGRATION_NOTES.md` lists anything that didn't translate one-to-one. See [Migration](#migration) below.

## Why betterhook

Every other hooks manager (lefthook, husky, pre-commit) was built around a single working tree. The moment two worktrees fire hooks at the same time, things break:

- **lefthook can't install** into a linked worktree ([#901](https://github.com/evilmartians/lefthook/issues/901)) — it confuses `.git` (the worktree pointer) with the shared `.git/` dir and exits 128.
- **Tools collide.** Two `cargo build` processes writing to the same `target/`. Two ESLint runs fighting over the same cache.
- **Output gets eaten.** Go's `os/exec` buffers entire subprocess output in memory. Four agents × four worktrees and lefthook OOMs.
- **Stashing breaks.** Formatters see untracked files that aren't in the commit and flag false positives ([#833](https://github.com/evilmartians/lefthook/issues/833)).

If you're a single dev in a single worktree, lefthook is great — keep using it. If you're running parallel agents, you need something built for that. That's betterhook.

## Get started

**1. Install**

<details open>
<summary><strong>cargo</strong></summary>

```sh
cargo install betterhook-cli
```
</details>

<details>
<summary><strong>npm</strong></summary>

```sh
npm install -g betterhook
```
</details>

<details>
<summary><strong>Homebrew</strong></summary>

```sh
brew install leonardomso/tap/betterhook
```
</details>

<details>
<summary><strong>From source</strong></summary>

```sh
git clone https://github.com/leonardomso/betterhook
cd betterhook
cargo install --path apps/cli
```
</details>

**2. Set it up**

```sh
cd your-repo
betterhook init       # writes a starter betterhook.toml
betterhook install    # installs the worktree-aware hook wrapper
betterhook status     # confirm everything looks right
```

**3. Commit**

```sh
git commit -am "go"   # hooks run, per-worktree, correctly
```

## What's inside

| Capability | What it does |
|---|---|
| **Worktree-aware wrapper** | One byte-identical hook in the shared `.git/hooks/` dir. Dispatches to whichever worktree is committing and loads *that* worktree's config. |
| **Streaming subprocess I/O** | Every line goes through a Tokio multiplexer the moment it's written. Output renders live, memory stays flat. |
| **Cross-worktree coordination** | Opt-in daemon hands out per-tool mutexes and per-worktree `CARGO_TARGET_DIR` so parallel `cargo build`s never collide. |
| **Capability-aware DAG scheduler** | Jobs declare reads/writes; the runner parallelizes everything that doesn't conflict. |
| **Content-addressable cache** | `concurrent_safe` jobs are keyed on `blake3(files) + blake3(tool) + blake3(args)`. Cache hits replay output without spawning a process. |
| **Multi-format config** | TOML, YAML, JSON, KDL — all parse to the same AST. `extends` works across formats. |
| **12 builtin linter wrappers** | `rustfmt`, `clippy`, `prettier`, `eslint`, `ruff`, `black`, `gofmt`, `govet`, `biome`, `oxlint`, `shellcheck`, `gitleaks`. |
| **NDJSON output for agents** | Stable wire protocol so AI agents can parse, retry, and self-correct without scraping logs. |

## Benchmarks

Reproducible — run `cargo xtask bench-monorepo` on your own hardware and post the diff. Synthetic 10,000-file repo across 5 packages, noop hook (`run = "true"`), measured on an M1 MacBook Pro:

| Measurement | betterhook 0.1.0 | lefthook 1.x |
|---|---:|---:|
| Binary startup (`--version`) | **<10 ms** | ~25 ms |
| `explain` (no execution) | **~10 ms** | n/a |
| Pre-commit on 10k files (cold) | **126 ms** | 139 ms |
| Pre-commit on 10k files (warm) | **107 ms** | 162 ms |
| Binary size (stripped, arm64) | **4.5 MB** | ~15 MB |

Reproduce:

```sh
cargo build --release -p betterhook-cli
PATH="$PWD/target/release:$PATH" cargo xtask bench-monorepo
```

Output goes to `target/bench-results.md`. The harness installs `n/a` rows for any tool not on `PATH`, so it works without lefthook or `hk` installed — but the comparison only fills in for tools you actually have.

Where betterhook *really* pulls ahead is what synthetic numbers can't show: streaming output (memory stays flat under chatty jobs — lefthook OOMs on 4-worktree fan-out), cache hits (sub-ms replay of captured output), and worktree correctness (lefthook can't even install in a linked worktree, exit 128).

## Recipes

Drop-in `betterhook.toml` configs for common stacks — copy, edit, install. Every recipe is verified to parse against the current schema in CI.

| Recipe | Stack | Highlights |
|---|---|---|
| [`recipes/typescript.toml`](recipes/typescript.toml) | TypeScript / JS monorepo | Prettier + ESLint + tsc, sharded `tsc` semaphore |
| [`recipes/rust.toml`](recipes/rust.toml) | Rust workspace | rustfmt + clippy + tests, per-worktree `CARGO_TARGET_DIR` |
| [`recipes/python.toml`](recipes/python.toml) | Python | Ruff format + lint + mypy |
| [`recipes/go.toml`](recipes/go.toml) | Go module | gofmt + govet + go test (uses builtins) |
| [`recipes/polyglot.toml`](recipes/polyglot.toml) | Mixed monorepo | All four stacks above, glob-routed |

```sh
cp recipes/typescript.toml /path/to/your-repo/betterhook.toml
betterhook install
```

Or extend one and override just the bits you need:

```toml
extends = ["./.betterhook/typescript.toml"]

[hooks.pre-commit.jobs.lint]
glob = ["src/**/*.ts"]
```

Full notes in [`recipes/README.md`](recipes/README.md).

## Migration

betterhook ships an importer for the four hook managers people are most likely already using. The flow is the same in every case:

```sh
betterhook import --from <existing-config>
betterhook install --takeover
```

| From | Command | Notes |
|---|---|---|
| lefthook | `betterhook import --from lefthook.yml` | Most one-to-one — `glob`, `exclude`, `parallel`, `fail_fast`, `priority` map directly. `skip` conditions become `skip` strings. |
| husky | `betterhook import --from .husky/pre-commit --from-format husky` | One job per hook script. Multi-line scripts become a single `run` block — split them after if you want parallelism. |
| pre-commit | `betterhook import --from .pre-commit-config.yaml --from-format pre-commit` | Each `repo`/`hook` becomes a betterhook job. Python-only `language` repos translate; the importer notes any that don't. |
| hk | `betterhook import --from hk.toml --from-format hk` | Native TOML, almost a passthrough. |

The importer always writes a `BETTERHOOK_MIGRATION_NOTES.md` next to your new config, listing anything that didn't translate (custom Python language repos, in-repo hook scripts, etc.). `--takeover` rewrites `.git/hooks/` so your existing tool stops firing — uninstall it from your package manager whenever you're ready.

## Configuration

betterhook reads `betterhook.toml` by default. `.yml`, `.yaml`, `.json`, and `.kdl` all parse to the same internal representation.

```toml
[meta]
version = 1

[hooks.pre-commit]
parallel = true
fail_fast = false
priority = ["fmt", "lint", "test"]

[hooks.pre-commit.jobs.fmt]
run = "prettier --write {staged_files}"
fix = "prettier --write {files}"       # used by `betterhook fix`
glob = ["*.ts", "*.tsx", "*.css"]
exclude = ["**/*.gen.ts"]
stage_fixed = true                      # re-stage files the job modified
isolate = "prettier"                    # mutex across worktrees
timeout = "60s"

[hooks.pre-commit.jobs.lint]
run = "eslint --cache --fix {staged_files}"
glob = ["*.ts", "*.tsx"]
isolate = "eslint"
env = { NODE_OPTIONS = "--max-old-space-size=2048" }

[hooks.pre-commit.jobs.test]
run = "cargo test --quiet"
isolate = { tool = "cargo", target_dir = "per-worktree" }
timeout = "5m"
```

<details>
<summary><strong>Template variables</strong></summary>

| Variable | Expands to |
|---|---|
| `{staged_files}` | Files in the index (NUL-delimited, glob-filtered) |
| `{push_files}` | Files changed vs. remote ref (for pre-push hooks) |
| `{all_files}` | Every tracked file |
| `{files}` | Glob-filtered subset of whichever file set is active |

File lists come from NUL-delimited git output, so filenames with spaces, unicode, or leading dashes work correctly. Long lists are chunked across multiple invocations to stay under `ARG_MAX`.
</details>

<details>
<summary><strong>Isolation modes</strong></summary>

| `isolate =` | What happens |
|---|---|
| `"eslint"` | Global mutex for `eslint` across every worktree |
| `{ name = "tsc", slots = 4 }` | Sharded semaphore: up to 4 concurrent invocations |
| `{ tool = "cargo", target_dir = "per-worktree" }` | Per-worktree key + auto-injected `CARGO_TARGET_DIR` |
</details>

<details>
<summary><strong>Config inheritance</strong></summary>

```toml
extends = [".betterhook/base.toml"]
```

Extends chains resolve depth-first with overlay-wins semantics. Cross-format extends works (a TOML file can extend a YAML file). A `betterhook.local.toml` next to your main config auto-merges with highest precedence — useful for per-machine overrides.
</details>

<details>
<summary><strong>Builtin wrappers</strong></summary>

Skip the run/glob/reads/writes boilerplate for common tools:

```toml
[hooks.pre-commit.jobs.fmt]
builtin = "rustfmt"
```

Available: `rustfmt`, `clippy`, `prettier`, `eslint`, `ruff`, `black`, `gofmt`, `govet`, `biome`, `oxlint`, `shellcheck`, `gitleaks`. Run `betterhook builtins list` for the full set with their resolved expansions.
</details>

## Agent integration

Built with AI coding agents in mind. Every agent-facing surface produces parseable output and stable exit codes.

```sh
betterhook run pre-commit --json
```

```jsonl
{"kind":"job_started","job":"lint","cmd":"eslint a.ts"}
{"kind":"line","job":"lint","stream":"stdout","line":"a.ts: clean"}
{"kind":"job_finished","job":"lint","exit":0,"duration":"312ms"}
{"kind":"summary","ok":true,"jobs_run":3,"jobs_skipped":0,"total":"890ms"}
```

**Self-correction loop.** When a formatter hook fails, an agent fixes and retries:

```sh
betterhook run pre-commit --json   # exit 1, fmt failed
betterhook fix --hook pre-commit   # runs each job's fix variant
git add -u
betterhook run pre-commit --json   # retry
```

**Introspection.** `status` (JSON snapshot), `explain` (which jobs would run, their DAG, resolved env), `--dry-run` (plan without executing), `doctor` (health check across install, config, cache, and tools on PATH).

## Commands

| Command | What it does |
|---|---|
| `betterhook init` | Scaffold a starter `betterhook.toml` |
| `betterhook install` | Write worktree-aware wrappers into `.git/hooks/` |
| `betterhook uninstall` | Remove wrappers whose SHA matches what we wrote |
| `betterhook run <hook>` | Run a hook directly (`--dry-run`, `--json`, `--skip`, `--only`) |
| `betterhook fix` | Run every job's `fix` variant (auto-format mode) |
| `betterhook status` | JSON snapshot of install state and config |
| `betterhook explain` | Print a job's resolved plan and DAG without executing |
| `betterhook doctor` | Health check across install, config, cache, and tools |
| `betterhook import` | Convert config from lefthook, husky, hk, or pre-commit |
| `betterhook cache` | Inspect, verify, or clear the content-addressable cache |
| `betterhook builtins` | List or show builtin linter/formatter wrappers |
| `betterhook completions <shell>` | Generate shell completions (bash, zsh, fish, elvish, powershell) |

Pre-built completions for bash, zsh, and fish ship in every [GitHub Release](https://github.com/leonardomso/betterhook/releases) as `betterhook-completions.tar.gz`.

## How it works

```
git commit
    │
    ▼
.git/hooks/pre-commit              one wrapper, all worktrees
    │
    │  git rev-parse --show-toplevel
    ▼
betterhook __dispatch              picks THIS worktree's config
    │
    ▼
load betterhook.toml               extends + local overrides resolved
    │
    ▼
DAG scheduler → tokio runner       streams output, acquires locks
    │
    ▼
exit 0 (commit proceeds) | exit 1 (commit blocked)
```

The piece other hook managers get wrong is step 3: their wrappers live in the shared hooks dir but operate as if there's only one worktree. betterhook's wrapper dispatches to whichever worktree is actually committing — that's the whole trick.

## Comparison

|  | betterhook | lefthook | husky | pre-commit |
|---|:---:|:---:|:---:|:---:|
| Worktree-aware install | yes | no | no | no |
| Streaming subprocess output | yes | buffered | partial | buffered |
| Capability-aware parallel scheduler | yes | no | no | no |
| Cross-worktree tool coordination | yes | no | no | no |
| Content-addressable hook cache | yes | no | no | no |
| Builtin linter wrappers | 12 | 0 | 0 | partial |
| NDJSON output for agents | yes | no | no | no |
| Config formats | 4 | YAML | JS | YAML |
| Binary size | ~6 MB | ~15 MB | (node) | (python) |
| Cold start | ~30 ms | ~100 ms | slower | slowest |
| Runtime dependency | none | none | Node.js | Python |

## Reference

<details>
<summary><strong>Exit codes</strong> — stable across releases, agents can rely on them</summary>

| Code | Meaning |
|---:|---|
| `0` | All jobs passed |
| `1` | At least one job failed |
| `2` | Config parse or schema error |
| `3` | Lock acquisition timeout |
| `4` | Git error (stash conflict, unexpected failure) |
| `5` | Install/uninstall error |
| `64` | Usage error (bad flags) |
| `124` | Job timeout (matches GNU `timeout(1)`) |
| `130` | Interrupted (SIGINT) |
</details>

<details>
<summary><strong>Environment variables</strong></summary>

| Variable | Purpose |
|---|---|
| `BETTERHOOK_SKIP=a,b` | Skip these jobs for this run |
| `BETTERHOOK_ONLY=a,b` | Only run these jobs |
| `BETTERHOOK_NO_LOCKS=1` | Bypass the coordinator daemon entirely |
| `BETTERHOOK_HOOK` | Set by betterhook in every job's env (current hook name) |
</details>

<details>
<summary><strong>Repository layout</strong></summary>

```
apps/betterhook/   library crate — config parser, runner, cache, daemon, builtins
apps/cli/          CLI binary (thin clap wrapper)
apps/docs/         documentation site (Mintlify)
xtask/             benchmarks, stress harness, fuzz runner
packaging/         Homebrew formula + npm wrapper scaffolds
```
</details>

## Development

Rust 1.86+, standard git, [Bun](https://bun.sh) for the docs site only.

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

All commits use [Conventional Commits](https://www.conventionalcommits.org/). See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the full workflow.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `betterhook install` fails inside a linked worktree | You're hitting the bug betterhook exists to solve — confirm you're on a recent release; this should always work. File an issue with `betterhook doctor` output. |
| Hooks don't fire on commit | `core.hooksPath` is set to a custom location. Run `git config --get core.hooksPath` and either unset it or run `betterhook install --hooks-path "$(git config --get core.hooksPath)"`. |
| `cargo build` jobs collide between worktrees | Add `isolate = { tool = "cargo", target_dir = "per-worktree" }` to the job — betterhook will inject a unique `CARGO_TARGET_DIR`. |
| Lock timeout (exit code 3) | Coordinator daemon is busy or stuck. Inspect with `betterhook status`, or set `BETTERHOOK_NO_LOCKS=1` to bypass it. |
| `betterhook doctor` reports a missing tool | The job's command isn't on `PATH` for the shell git invokes. Pin via `tool =` or set `PATH` in the job's `env`. |
| Output looks buffered | Some tools detect non-tty and disable colors/streaming. Force it (`--color=always`, `FORCE_COLOR=1`) in the job's `env`. |

## License

MIT — see [`LICENSE`](LICENSE).
