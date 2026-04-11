# Show HN: Betterhook — a worktree-native git hooks manager built for the AI agent era

Betterhook is a memory-efficient, single-binary git hooks manager
written in Rust. It exists because every existing tool in this space
(husky, lefthook, hk, pre-commit) was designed in a world where one
human ran one terminal in one checkout. That world is gone.

We now run **N agents in N worktrees**, all editing the same monorepo,
all racing each other into commit. The bottleneck isn't the linter
anymore — it's the *coordination* of the linter. Betterhook is the tool
that finally treats that as a first-class concern.

## What's different

- **Capability DAG scheduler.** Jobs declare `reads = [...]`,
  `writes = [...]`, `network = ...`, and `concurrent_safe = ...`. The
  scheduler builds a real DAG, runs every root in parallel, and
  serializes only the pairs that actually conflict. No more
  hand-tuned `priority = [...]` lists.

- **Content-addressable hook cache.** A cached hook run is keyed on
  `blake3(file_content) + blake3(tool_binary) + blake3(args)`, follows
  mise/nvm shims to the concrete binary, and lives under
  `.git/common/betterhook/cache/`. Repeating a clean commit is a
  sub-100ms cache replay, not a re-run. This is where the "faster than
  hk" claim is earned.

- **On-save speculative execution.** A coordinator daemon watches the
  worktree, prewarms every `concurrent_safe` job into the cache after
  a debounce, and the commit-time runner just hits the cache. The
  golden path on a clean repo is *zero subprocesses spawned*.

- **Persistent per-repo daemon, but truly opt-in.** No daemon if your
  config doesn't declare locks. When you do, `betterhook install`
  drops a launchd plist (macOS) or systemd user unit (Linux) so the
  daemon survives reboots and crashes auto-restart. RSS is under
  12 MB at idle.

- **Single binary.** No `betterhookd` to install separately —
  `betterhook serve` is the same binary running as a daemon.

- **12 builtin linter wrappers** with structured NDJSON diagnostics:
  rustfmt, clippy, prettier, eslint, ruff, black, gofmt, govet, biome,
  oxlint, shellcheck, gitleaks. Each one knows its tool's native
  output (cargo `--message-format=json`, eslint `--format=json`, etc.)
  and emits `{kind: "diagnostic", file, line, column, severity, rule,
  message}` events on the same wire format the runner uses for
  everything else. Agents parse JSON, not unstructured text.

- **`betterhook import --from <husky|lefthook|hk|pre-commit>`** —
  bring your existing config in one command, get a notes file
  flagging anything that didn't survive the round-trip.

- **`betterhook doctor`** — pre-flight health check that's safe to run
  in CI. Walks the install manifest, config parse, builtin tool
  availability on PATH, cache writability, watcher permissions, and
  conflicting `core.hooksPath`.

## Honest limitations

- No Windows day-one parity. The Unix socket and launchd plist need
  Windows-native equivalents; both are scoped for v1.x.
- Speculative execution depends on `notify`. On NFS or sandboxed
  containers it gracefully degrades to non-speculative mode and tells
  you why in `betterhook status`.

## Numbers

On a synthetic 10k-file monorepo (`xtask bench-monorepo` reproduces
this):

| Metric | Target | Notes |
|---|---|---|
| Cold `betterhook --version` | < 30 ms | |
| Warm `run pre-commit` (10 jobs, all cached) | < 20 ms | the v1 cache hit |
| Daemon idle RSS | < 12 MB | with watcher |
| 10k-file monorepo cold run | within 20% of hk | launch gate |
| 10k-file monorepo warm run (cache hit) | 2–5× faster than hk | launch gate |

## Try it

```bash
brew install betterhook   # or: cargo install betterhook
cd your-repo
betterhook init           # writes a starter betterhook.toml
betterhook install        # writes the hook + the launchd unit
betterhook doctor         # everything green?
git commit                # done
```

OSS, MIT, no telemetry, no hosted dashboard. PRs welcome.

GitHub: https://github.com/leonardomso/betterhook
