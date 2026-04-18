//! Shared test helpers.
//!
//! Each integration test binary (`worktree.rs`, `end_to_end.rs`, etc.)
//! previously spelled out its own `git()`, `init_repo()`, and
//! `new_repo_with_worktrees()` helpers. They drifted subtly from one
//! file to the next. This module is the single source of truth.
//!
//! Integration test files include this module with `mod common;` at
//! their top. Rustc treats `tests/common/mod.rs` as a non-test module,
//! so it doesn't get a redundant `running 0 tests` line.

#![allow(dead_code)] // each test binary uses a subset

use std::path::{Path, PathBuf};
use std::process::Command;

use betterhook::runner::{RunOptions, SinkKind};
use tempfile::TempDir;

/// Run a git command in `cwd`, panic on failure.
pub fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t.t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t.t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Create a fresh single-worktree repo in a tempdir. Returns the
/// tempdir handle (keep it alive to hold the repo) and the repo root.
pub fn init_repo() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("README.md"), "hi").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    (dir, repo)
}

/// Create a primary repo with `n` additional linked worktrees.
/// Returns `(tempdir_handle, primary_path, linked_paths)`.
pub fn new_repo_with_worktrees(n: usize) -> (TempDir, PathBuf, Vec<PathBuf>) {
    let dir = TempDir::new().unwrap();
    let primary = dir.path().join("primary");
    std::fs::create_dir_all(&primary).unwrap();
    git(&primary, &["init", "-q", "-b", "main"]);
    git(&primary, &["config", "user.email", "t@t.t"]);
    git(&primary, &["config", "user.name", "t"]);
    std::fs::write(primary.join("README.md"), "hi").unwrap();
    git(&primary, &["add", "README.md"]);
    git(&primary, &["commit", "-q", "-m", "init"]);

    let linked: Vec<PathBuf> = (0..n)
        .map(|i| {
            let wt = dir.path().join(format!("wt-{i}"));
            git(
                &primary,
                &[
                    "worktree",
                    "add",
                    wt.to_str().unwrap(),
                    "-b",
                    &format!("feature-{i}"),
                ],
            );
            wt
        })
        .collect();

    (dir, primary, linked)
}

/// Write the given body as `betterhook.toml` in `repo`.
pub fn write_config(repo: &Path, body: &str) {
    std::fs::write(repo.join("betterhook.toml"), body).unwrap();
}

/// `RunOptions` that silence stdout/stderr for runner tests.
pub fn run_options_quiet() -> RunOptions {
    RunOptions {
        sink: SinkKind::Json,
        ..Default::default()
    }
}
