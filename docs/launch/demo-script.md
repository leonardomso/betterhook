# Betterhook v1 launch demo script

A 30-second screen recording that closes the deal: eight worktrees, one
formatter, no corruption, sub-second cache hits, every command on
screen. Each block below is exactly what to type and what should appear.

## Setup (off-camera, ~10 seconds before record)

```bash
mkdir -p /tmp/bh-demo && cd /tmp/bh-demo
git init -q
cargo init --lib -q
cat > betterhook.toml <<'EOF'
[meta]
version = 1

[hooks.pre-commit.jobs.fmt]
run = "cargo fmt --all -- --check"
glob = ["*.rs"]
concurrent_safe = true
isolate = "cargo"
EOF
git add -A && git commit -q -m "init"
betterhook install
```

## Take 1 — single worktree, cache miss → cache hit

```bash
$ betterhook doctor
{ "ok": true, ... }                          # all green

$ time git commit -q --allow-empty -m "warm cache"
real    0m0.040s                              # cold path

$ time git commit -q --allow-empty -m "warm cache"
real    0m0.012s                              # ⚡ cache hit
```

Voiceover: *"Same content twice. The second commit is a cache hit —
zero subprocesses spawned. This is what `concurrent_safe` jobs unlock."*

## Take 2 — eight worktrees, one cargo fmt, no corruption

```bash
$ for i in $(seq 1 8); do
    git worktree add -q -b wt-$i /tmp/bh-demo-wt-$i
  done

$ time (
    for i in $(seq 1 8); do
      (cd /tmp/bh-demo-wt-$i \
        && echo "pub fn id_${i}() -> usize { ${i} }" > src/wt_${i}.rs \
        && git add -A \
        && git commit -q -m "wt $i") &
    done
    wait
  )
real    0m0.600s
```

Voiceover: *"Eight worktrees, eight commits, in parallel. The
coordinator daemon serializes the cargo target dir per worktree so
none of them clobber each other. No stash conflict, no half-baked
build, no corrupted index."*

```bash
$ for i in $(seq 1 8); do
    git -C /tmp/bh-demo-wt-$i log -1 --format=%s
  done
wt 1
wt 2
wt 3
wt 4
wt 5
wt 6
wt 7
wt 8
```

## Take 3 — speculative execution

```bash
$ betterhook status | jq '.speculative'
{
  "watched_worktrees": 9,
  "watch_count": ...,
  "queue_depth": 0,
  "last_prewarm_ms_ago": 142,
  "disabled_reason": null
}
```

Voiceover: *"The watcher prewarmed every save while you were typing.
By the time you commit, the answer's already in the cache."*

## Closing card

```text
betterhook v1.0
Worktree-native git hooks for the AI agent era.
brew install betterhook   |   cargo install betterhook
github.com/leonardomso/betterhook
```

## Recording notes

- Use `asciinema rec --idle-time-limit 1` so the eight-worktree race
  collapses naturally.
- Pin terminal to 100×24 — anything wider clips on the HN front page.
- The closing card is a static image overlaid for 3 seconds; don't
  type it live.
- Total runtime should land between 28 and 35 seconds. Cut take 3 if
  the speculative section pushes past 35.
