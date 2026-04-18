# Repo Cleanup & Docs Overhaul

## Context

Full repo review revealed 40+ issues across git hygiene, documentation accuracy, build config, and release workflow. This spec covers all fixes in 6 phases.

## Decisions

- Package manager: **bun** (not pnpm)
- Lock file: **commit `bun.lock`** for reproducibility
- Planned/unshipped features: **remove from docs entirely** (track in GitHub issues instead)

## Phase 1: Git Hygiene

Commit: `chore: fix .gitignore and remove tracked .turbo artifacts`

- Add `.turbo/`, `.DS_Store` to root `.gitignore`
- `git rm -r --cached` the 6 tracked `.turbo` log files
- Delete `.turbo/` dirs from disk

## Phase 2: Bun Migration

Commit: `chore: complete bun migration, remove pnpm artifacts`

- Delete pnpm-managed `node_modules/`
- Update `.gitignore`: allow `bun.lock` (keep `bun.lockb` ignored)
- Add `"packageManager": "bun@1.2.12"` to root `package.json`
- Run `bun install`, commit `bun.lock`

## Phase 3: Cargo & Build Cleanup

Commit: `chore: remove unused deps, fix CHANGELOG, clean up release workflow`

- Remove `ignore` from `[workspace.dependencies]` in root `Cargo.toml`
- Remove duplicate `futures`, `bytes` from `[dev-dependencies]` in `apps/betterhook/Cargo.toml`
- Remove unused `which` from `apps/cli/Cargo.toml`
- Add `## [Unreleased]` to top of `CHANGELOG.md`
- Fix MSRV `1.85` -> `1.86` in CHANGELOG v0.0.1 entry
- Remove redundant `strip` step from `release.yml`
- Add sha256 checksum generation to `release.yml`
- Make `prerelease` conditional on tag pattern

## Phase 4: Docs Critical Fixes

Commit: `docs: fix betterhookd references, wrong event names, stale paths`

- Replace `betterhookd` -> `betterhook serve` in code blocks and prose
- Fix README repo layout (no betterhookd binary)
- Fix NDJSON event names in README (`job_end` -> `job_finished`, `job_output` -> `line`)
- Remove `docs/` and `man/` from architecture tree
- Annotate removed paths in CHANGELOG
- Fix CONTRIBUTING.md turbo/node references

## Phase 5: Docs Content Cleanup

Commit: `docs: fix stale versions, deprecated commands, add KDL docs, remove planned features`

- Update v0.0.1 -> v0.0.2 version refs
- Replace `betterhook migrate` -> `betterhook import`
- Remove `--chain` (unshipped), `lock_wait` event (planned), `xtask compat` description (stubbed)
- Remove "Betterhook v1" references
- Add KDL format to formats.mdx
- Fix explain.mdx isolate field (Rust Debug -> JSON)
- Normalize capitalization

## Phase 6: Docs Completeness & Polish

Commit: `docs: add missing command cards, fix migration guides, update env table`

- Add `cache`, `builtins`, `doctor`, `import` to commands overview cards
- Add `BETTERHOOK_HOOK` to README env vars table
- Add npm/bun alternatives in migration guides
- Fix README quickstart (add clone step)
- Fix doctor.mdx exit code description
- Update SECURITY.md for all config formats
- Clarify cold start claims (~30ms binary, ~50ms hook)
- Remove stale Turbo dep from changelog
- Fix performance.mdx CI baseline claim

## Verification

Each phase: `cargo check --workspace`, `cargo clippy`, `cargo test` (phase 3+). `bun install` (phase 2). Manual link check (phase 6).
