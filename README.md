# betterhook

> Memory-efficient, worktree-native git hooks manager built for the AI agent era.

`betterhook` is a [lefthook](https://lefthook.dev)-style git hooks manager designed for the workflow where multiple AI coding agents (Claude Code, Cursor, Codex, Aider, …) run in parallel via [Conductor](https://conductor.build), each in its own git worktree.

## Why

lefthook is fast and language-agnostic, but it breaks under the agent-era workload:

- **Worktree bugs.** `lefthook install` fails with exit 128 inside linked worktrees, remote configs corrupt the index, and `$GIT_DIR` pollution leaks into subprocess calls.
- **Memory pressure.** Go's `os/exec` buffers entire subprocess stdout/stderr in memory; 4 agents × N parallel jobs across 4 worktrees regularly hit OOM.
- **Tool contention.** ESLint cache, `cargo target/`, prettier rewriting `pnpm-lock.yaml` and `.tsbuildinfo` all corrupt under concurrent worktree runs. No tool today coordinates them.

`betterhook` fixes these with:

- A **worktree-aware wrapper** installed once into the shared `.git/hooks/` dir that dispatches at runtime via `git rev-parse --show-toplevel`, so every worktree runs *its own* config from the same wrapper.
- **Line-streaming subprocess I/O** via Tokio — output renders live, never buffered, memory stays constant.
- An **opt-in coordinator daemon** (`betterhookd`) exposing cross-worktree tool locks (mutex, sharded, tool-path-aware) over a tiny Unix-socket protocol, with an `fs4` flock fallback.
- **NDJSON `--json` output** and a stable exit-code contract so agents can parse failures and self-correct.
- **Multi-format config** — TOML, YAML, or JSON, all deserializing to one canonical AST.
- A **`betterhook migrate`** command from `lefthook.yml`.

## Status

**Under construction.** This is phase 1 of 20 — scaffolding only. See `/Users/leonardomaldonado/.claude/plans/clever-kindling-puffin.md` for the full implementation plan.

## Repo layout

```
betterhook/
├── apps/
│   ├── betterhook/   # core library + daemon binary (betterhookd)
│   └── cli/          # the `betterhook` CLI
├── xtask/            # bench, stress, lefthook-compat harness
├── Cargo.toml        # cargo workspace
├── turbo.json        # turborepo pipeline
└── package.json      # turbo root
```

## Development

```sh
pnpm install        # install turbo
pnpm run build      # cargo build -p betterhook && cargo build -p betterhook-cli
pnpm run test
pnpm run lint       # clippy -D warnings
cargo fmt --all
```

## License

MIT — see [LICENSE](LICENSE).
