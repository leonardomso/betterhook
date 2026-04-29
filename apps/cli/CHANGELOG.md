# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/leonardomso/betterhook/releases/tag/betterhook-cli-v0.1.0) - 2026-04-29

### Added

- *(cli)* add shell completions and install instructions
- *(cli)* modernize init template with DAG capability fields
- *(cli)* explain --format dot and --format svg for DAG visualization
- *(cli)* betterhook import rename with husky, hk, and pre-commit sources
- *(cli)* betterhook doctor health check
- *(cli)* betterhook builtins list and show subcommands
- *(cli)* betterhook cache stats, clear, and verify subcommands
- *(cli)* status and explain surface the resolved DAG
- *(daemon)* persistent per-repo lifecycle with launchd and systemd units
- *(cli)* migrate from lefthook.yml to betterhook.toml
- *(cli)* fix, explain, and dry-run subcommands
- *(runner)* lock client with fs4 flock fallback and CARGO_TARGET_DIR injection
- *(cli)* json output mode and status introspection
- *(runner)* per-job timeouts and BETTERHOOK_SKIP / BETTERHOOK_ONLY
- *(runner)* sequential job executor with line-streaming output
- *(cli)* dispatch subcommand for runtime config resolution
- *(cli)* install and uninstall with worktree-aware hook wrapper

### Fixed

- address code review findings across CI, npm, and docs
- *(cli)* doctor uses tokio::process for git subprocess calls

### Other

- release v0.1.0
- add release-plz, Renovate, and crates.io publish pipeline
- replace prose double-dashes with proper punctuation in README
- rewrite README and clean up Conductor references
- *(core)* improve install status and runner maintainability
- *(comments)* clarify code documentation
- add dependabot, issue/PR templates, Windows CI, MSRV check, fuzz-smoke CI
- apply cargo fmt to entire workspace
- add missing command cards, fix migration guides, update env table
- fix stale versions, deprecated commands, add KDL docs, remove planned features
- fix betterhookd references, wrong event names, stale paths
- fix .gitignore and remove tracked .turbo artifacts
- update references to removed docs/ and man/ paths
- migrate from pnpm/turbo to bun, remove turborepo
- update README for v0.0.2 feature set
- consolidate betterhookd into betterhook serve subcommand
- *(readme)* restructure for scanability and getting-started flow
- *(deps)* upgrade rust toolchain, cargo deps, and turbo to latest
- readme, daemon protocol spec, man page, and v0.0.1 changelog
- scaffold turborepo monorepo with apps/betterhook and apps/cli
