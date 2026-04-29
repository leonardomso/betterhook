# betterhook

A git hooks manager that actually works with worktrees.

---

## The problem

Git hooks are simple: put a script in `.git/hooks/`, git runs it before your commit. Every hooks manager (lefthook, husky, pre-commit) works fine until you start using [git worktrees](https://git-scm.com/docs/git-worktree).

Worktrees let you have multiple checkouts of the same repo side by side. Each worktree has its own working directory and branch, but they all share one `.git` directory. This is how teams run multiple AI coding agents in parallel: each agent gets its own worktree, writes code on its own branch, and opens its own PR.

The moment two worktrees try to run hooks at the same time, things break:

- **lefthook can't install** into a linked worktree ([#901](https://github.com/evilmartians/lefthook/issues/901)). It confuses `.git` (the worktree pointer) with the shared `.git/` dir and exits with code 128.
- **Tools collide.** Two `cargo build` processes writing to the same `target/` directory. Two ESLint processes fighting over the same cache. Two `prettier` processes reformatting the same lockfile.
- **Output gets eaten.** Go's `os/exec` buffers the entire subprocess output in memory. Under four agents running hooks across four worktrees, lefthook OOMs.
- **Stashing breaks.** Formatters see untracked files that aren't part of the commit and flag false positives ([#833](https://github.com/evilmartians/lefthook/issues/833)).

If you're a single developer in a single worktree, lefthook works great and you should use it. But if you're running parallel agents, each in its own worktree, you need something that was designed for that from the ground up.

## What betterhook does

betterhook is a single Rust binary (~6 MB, ~30 ms cold start) that manages your git hooks with worktree isolation as a first-class constraint.

**One wrapper, correct dispatch.** A single byte-identical shell wrapper lives in the shared `.git/hooks/` dir. When git fires the hook, the wrapper calls `git rev-parse --show-toplevel` to find which worktree is actually committing, then loads *that* worktree's `betterhook.toml`. Every worktree runs its own config through the same wrapper.

**Streaming output.** Every line from every subprocess goes through a Tokio multiplexer the instant it's written. Output renders live. Memory stays constant no matter how chatty a job gets.

**Tool coordination.** An opt-in coordinator daemon hands out per-tool mutexes across worktrees. Two worktrees running `cargo build` get separate `CARGO_TARGET_DIR` paths automatically. Two running ESLint wait for each other instead of corrupting the cache.

**Content-addressable caching.** Jobs that declare `concurrent_safe = true` get cached by `blake3(file_content) + blake3(tool_binary) + blake3(args)`. A cache hit replays captured output without spawning a process at all.

**DAG scheduler.** Jobs declare what they read and write. The runner builds a dependency graph and runs everything that doesn't conflict in parallel. Only conflicting pairs serialize.

```sh
betterhook init           # writes a starter betterhook.toml
betterhook install        # installs the hook wrapper into .git/hooks/
git commit -am "go"       # hooks run, per-worktree, correctly
```

---

## Quickstart

```sh
# Install from source (Rust 1.86+)
cargo install --path apps/cli

# In your repo
betterhook init
betterhook install
betterhook status          # check everything looks right
```

Your next `git commit` will run the jobs in `betterhook.toml`.

### Migrating from lefthook?

```sh
betterhook import --from lefthook.yml
betterhook install --takeover
```

This converts your lefthook config to `betterhook.toml` and writes a `BETTERHOOK_MIGRATION_NOTES.md` listing anything that didn't translate directly.

Import also supports `husky`, `hk`, and `pre-commit`:

```sh
betterhook import --from .husky/pre-commit --from-format husky
```

---

## Configuration

betterhook reads `betterhook.toml` by default. It also supports `.yml`, `.yaml`, `.json`, and `.kdl`. All four formats parse into the same internal representation.

```toml
[meta]
version = 1

[hooks.pre-commit]
parallel = true
fail_fast = false

# Jobs run in priority order when the parallel limit is reached.
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
# Per-worktree target dir so parallel cargo builds never collide.
isolate = { tool = "cargo", target_dir = "per-worktree" }
timeout = "5m"
```

### Template variables

| Variable | Expands to |
|---|---|
| `{staged_files}` | Files in the index (NUL-delimited, glob-filtered) |
| `{push_files}` | Files changed vs. remote ref (for pre-push hooks) |
| `{all_files}` | Every tracked file |
| `{files}` | Glob-filtered subset of whichever file set is active |

File lists are parsed from NUL-delimited git output, so filenames with spaces, unicode, or leading dashes work correctly. Long lists are chunked across multiple invocations to stay under `ARG_MAX`.

### Config inheritance

```toml
extends = [".betterhook/base.toml"]
```

Extends chains resolve depth-first with overlay-wins semantics. Cross-format extends works (a TOML file can extend a YAML file). A `betterhook.local.toml` next to your main config is auto-merged with highest precedence, useful for per-machine overrides.

### Isolation modes

| `isolate =` | What happens |
|---|---|
| `"eslint"` | Global mutex for "eslint" across every worktree |
| `{ name = "tsc", slots = 4 }` | Sharded semaphore: up to 4 concurrent invocations |
| `{ tool = "cargo", target_dir = "per-worktree" }` | Per-worktree key + auto-injected env var |

### Builtin wrappers

Instead of writing the full run/glob/reads/writes config for common tools, use a builtin:

```toml
[hooks.pre-commit.jobs.fmt]
builtin = "rustfmt"
```

This expands to `cargo fmt --all -- --check` with the right globs, capability fields, and a `fix` variant. Available builtins: `rustfmt`, `clippy`, `prettier`, `eslint`, `ruff`, `black`, `gofmt`, `govet`, `biome`, `oxlint`, `shellcheck`, `gitleaks`.

Run `betterhook builtins list` to see them all.

---

## Agent integration

betterhook was built with AI coding agents in mind. Every agent-facing surface produces parseable output and stable exit codes.

### NDJSON output

```sh
betterhook run pre-commit --json
```

```jsonl
{"kind":"job_started","job":"lint","cmd":"eslint a.ts"}
{"kind":"line","job":"lint","stream":"stdout","line":"a.ts: clean"}
{"kind":"job_finished","job":"lint","exit":0,"duration":"312ms"}
{"kind":"summary","ok":true,"jobs_run":3,"jobs_skipped":0,"total":"890ms"}
```

### Self-correction loop

When a formatter hook fails, an agent can fix and retry:

```sh
betterhook run pre-commit --json   # exit 1, fmt failed
betterhook fix --hook pre-commit   # runs each job's fix variant
git add -u
betterhook run pre-commit --json   # retry
```

### Introspection

```sh
betterhook status                  # JSON snapshot: installed hooks, config, worktree identity
betterhook explain --hook pre-commit  # which jobs would run, their DAG, resolved env
betterhook run pre-commit --dry-run   # plan without executing
betterhook doctor                  # health check: install, config, cache, tools on PATH
```

---

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

## Exit codes

Stable across releases. Agents can rely on these.

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

## Environment variables

| Variable | Purpose |
|---|---|
| `BETTERHOOK_SKIP=a,b` | Skip these jobs for this run |
| `BETTERHOOK_ONLY=a,b` | Only run these jobs |
| `BETTERHOOK_NO_LOCKS=1` | Bypass the coordinator daemon entirely |
| `BETTERHOOK_HOOK` | Set by betterhook in every job's env (current hook name) |

---

## How it works

1. You run `git commit`.
2. Git fires `.git/hooks/pre-commit`, the wrapper betterhook installed.
3. The wrapper runs `git rev-parse --show-toplevel` to find the **current worktree**, not the shared `.git/` dir.
4. It execs into `betterhook __dispatch --hook pre-commit --worktree /path/to/this/worktree`.
5. betterhook loads `betterhook.toml` from that worktree, resolves extends and local overrides.
6. The runner spawns jobs (sequential or parallel), streams output line by line, acquires coordinator locks for isolated jobs, and reports results.
7. Non-zero exit on any job blocks the commit.

Step 3 is the part other hook managers get wrong. The wrapper lives in the shared hooks dir (one copy for all worktrees), but it dispatches to whichever worktree is actually committing. This is what makes multi-worktree setups work.

---

## Comparison

|  | betterhook | lefthook | husky | pre-commit |
|---|:---:|:---:|:---:|:---:|
| Worktree-aware install | yes | no | no | no |
| Streaming subprocess output | yes | buffered | partial | buffered |
| Capability-aware parallel scheduler | yes | no | no | no |
| Cross-worktree tool coordination | yes | no | no | no |
| Content-addressable hook cache | yes | no | no | no |
| Builtin linter wrappers (12) | yes | no | no | partial |
| NDJSON output for agents | yes | no | no | no |
| Multi-format config | 4 formats | YAML | JS | YAML |
| Binary size | ~6 MB | ~15 MB | (node) | (python) |
| Cold start | ~30 ms | ~100 ms | slower | slowest |
| Runtime dependency | none | none | Node.js | Python |

---

## Development

Rust 1.86+, standard git. [Bun](https://bun.sh) for the docs site only.

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

```
apps/betterhook/     # library crate: config parser, runner, cache, daemon, builtins
apps/cli/            # CLI binary (thin clap wrapper)
apps/docs/           # documentation site (Mintlify)
xtask/               # benchmarks, stress harness, fuzz runner
packaging/           # Homebrew formula + npm wrapper scaffolds
```

All commits use [conventional commits](https://www.conventionalcommits.org/). See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the full workflow.

---

## Documentation

- [betterhook.dev](https://betterhook.dev) — full docs (commands, architecture, reference)
- [`CHANGELOG.md`](CHANGELOG.md) — release history
- `betterhook --help` — per-subcommand reference

## License

MIT. See [`LICENSE`](LICENSE).
