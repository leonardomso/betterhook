# Changelog

All notable changes to betterhook. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this
project adheres to [semantic versioning](https://semver.org/).

## [0.0.1] - 2026-04-11

Initial scaffolding release. Everything below was built across the 20
phases in `plans/clever-kindling-puffin.md` and is ready for in-repo
dogfooding.

### Added

#### Foundation (phases 1–5)
- Turborepo monorepo with `apps/betterhook` (library + `betterhookd`
  binary) and `apps/cli` (the `betterhook` CLI).
- Cargo workspace on resolver v3, edition 2024, rust 1.85 MSRV.
- Multi-format config parser supporting TOML, YAML, and JSON, all
  lowering to one canonical AST via `serde`.
- `extends` inheritance with depth-first resolution, cross-format
  support, circular-extends detection, and `betterhook.local.*`
  highest-precedence override layer.
- Async git worktree introspection (`rev-parse --absolute-git-dir`,
  `--git-common-dir`, `--show-toplevel`, `worktree list --porcelain`)
  via `tokio::process::Command` — never libgit2/gix, deliberately.
- Fileset templates (`{staged_files}`, `{push_files}`, `{all_files}`,
  `{files}`) with NUL-delimited `-z` parsing, `globset` include/
  exclude filtering, POSIX shell escaping, and `ARG_MAX`-aware
  command chunking.

#### Install model (phases 6–7)
- `betterhook install` writes a single byte-identical POSIX wrapper
  into the shared `<common-dir>/hooks/<hookname>` dir. At runtime the
  wrapper uses `git rev-parse --show-toplevel` to identify the
  current worktree and exec's `betterhook __dispatch` into it.
- SHA-256 manifest in `<common-dir>/betterhook/installed.json` so
  `uninstall` never touches user-modified hooks.
- `core.hooksPath` takeover/refuse semantics so we coexist cleanly
  with other hooks tools.
- `__dispatch` subcommand resolves the per-worktree config, emits
  `NoConfig` / `HookNotConfigured` / `NoJobs` soft-miss exits so a
  worktree without a config never blocks a commit.

#### Execution engine (phases 8–11)
- Sequential executor with line-streaming subprocess output via a
  central `OutputMultiplexer` actor (mpsc → single writer). Lines
  are atomic; memory stays constant regardless of subprocess output
  volume.
- Parallel executor with a `tokio::sync::Semaphore` and priority-
  ordered spawn — directly fixes lefthook #846.
- `fail_fast` uses a shared `Cancel` latch (atomic bool + `Notify`)
  so in-flight run_commands SIGKILL their children instead of
  waiting for `JoinSet::abort_all` to take effect.
- `kill_on_drop(true)` on every spawned child plus explicit
  `start_kill` + abort of the stdout/stderr reader tasks on cancel
  or timeout — prevents orphaned descendants (e.g. inside `sh -c`)
  from holding pipe fds open.
- Untracked-file stash safety via unique-message `git stash push
  --keep-index --include-untracked` with verified pop (lefthook
  #833).
- `stage_fixed` via before/after unstaged-file snapshots; parallel
  jobs serialize the git-index write through a shared async
  `Mutex`.
- Per-job timeouts parsed by `humantime`, enforced in run_command's
  select!, reported as exit code 124.
- `BETTERHOOK_SKIP` / `BETTERHOOK_ONLY` env vars and matching
  `--skip` / `--only` CLI flags, with env-level `RunOptions::from_env`.

#### Agent surfaces (phase 12)
- NDJSON `--json` output mode. Every `OutputEvent` serializes as
  one line on stdout with a stable `kind` tag.
- `betterhook status` prints a JSON snapshot of installed hooks,
  SHA integrity, resolved config, and worktree identity.

#### Worktree integration tests (phase 13)
- End-to-end tests using real `git worktree add` spin-ups that
  verify a single installed wrapper dispatches to each worktree's
  own config correctly.

#### Coordinator daemon (phases 14–15)
- `betterhookd` is the opt-in coordinator daemon. Accepts
  length-prefixed bincode frames over a Unix socket, maintains a
  `LockKey → Arc<Semaphore>` registry with lazy creation, and
  exits 60 s after the last client disconnects.
- Protocol: `Hello` / `Acquire` / `Release` / `Status` / `Ping`.
  Per-connection `HashMap<token, HeldPermit>` ensures connection
  drop releases every permit.
- `fs4` advisory flock fallback under
  `<common-dir>/betterhook/locks/<key>.lock` for environments where
  the daemon can't spawn.
- `key_for_spec` derives the wire key from an `IsolateSpec` and,
  for `cargo`-flavored `ToolPath::PerWorktree`, auto-injects
  `CARGO_TARGET_DIR=<worktree>/target` so concurrent worktree
  builds never collide.
- `BETTERHOOK_NO_LOCKS` / `--no-locks` escape hatch.

#### CLI surfaces (phases 16–17)
- `betterhook run <hook>` (with `--dry-run`, `--json`, `--skip`,
  `--only`).
- `betterhook explain --hook <name> [--job <n>]` (JSON snapshot).
- `betterhook fix` runs every job's `fix = ...` variant —
  dedicated agent self-correction surface.
- `betterhook migrate --from lefthook.yml` converts a real
  lefthook config to betterhook TOML + writes
  `BETTERHOOK_MIGRATION_NOTES.md`.

#### Tooling (phases 18–19)
- Criterion benches: `config_parse` (TOML + YAML parse and
  parse+lower) and `output_multiplexer` (NDJSON per-line cost).
  `xtask bench` runs both with CI-gating exit codes.
- `cargo-fuzz` targets for the multi-format parser chain and the
  dispatch path-detection. Fuzz crate is excluded from the main
  workspace so a stable-toolchain build never drags nightly in.

#### Documentation (phase 20)
- `README.md` — full project intro, pitch, install, config, agent
  surfaces, exit codes, env vars, development.
- `docs/protocol.md` — daemon IPC wire format and lifecycle for
  third-party agent harnesses.
- `man/betterhook.1` — subcommand reference, environment, exit
  codes.

### Known gaps (follow-ups)

- Coordinator daemon client is flock-only in v0.0.1. Socket-backed
  acquire path, spawn retry, and health handshake land next.
- Stash safety covers the untracked-file case (lefthook #833). The
  stacked "unstaged delta under staged delta" case remains a known
  limitation documented inline in `git::stash` and will be solved
  by a patch-based approach in a later phase.
- `xtask stress` and the nightly lefthook-compat diff suite are
  stubbed — real implementations come after v0.0.1.
- `remotes:` config inheritance (with mandatory SHA pinning) is
  deferred to v0.3.
- `LockfileGuarded` isolation (the prettier-on-pnpm-lock fix) is
  deferred to v0.3.
