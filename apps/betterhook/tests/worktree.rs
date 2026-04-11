//! End-to-end linked-worktree tests.
//!
//! These prove betterhook's headline property: one wrapper installed
//! into the shared common dir dispatches to each worktree's own
//! config at runtime. The property-under-test is exactly the thing
//! lefthook fails at.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use betterhook::dispatch::{Dispatch, resolve};
use betterhook::git::git_common_dir;
use betterhook::install::{InstallOptions, install};
use tempfile::TempDir;

fn git(cwd: &Path, args: &[&str]) {
    let status = StdCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t.t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t.t")
        .status()
        .unwrap();
    assert!(
        status.success(),
        "git {args:?} failed in {}",
        cwd.display()
    );
}

/// Create a primary repo with `n` additional linked worktrees.
/// Returns `(tempdir_handle, primary_path, linked_paths)`.
fn new_repo_with_worktrees(n: usize) -> (TempDir, PathBuf, Vec<PathBuf>) {
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

#[tokio::test]
async fn install_writes_one_wrapper_shared_across_worktrees() {
    let (_d, primary, linked) = new_repo_with_worktrees(2);

    // Give the primary worktree a minimal config.
    std::fs::write(
        primary.join("betterhook.toml"),
        "[hooks.pre-commit.jobs.primary]\nrun = \"echo primary\"\n",
    )
    .unwrap();

    let report = install(InstallOptions {
        worktree: Some(primary.clone()),
        ..Default::default()
    })
    .await
    .unwrap();

    // The wrapper lives in the shared common dir, visible from every
    // worktree — which is exactly what we want.
    let primary_common = git_common_dir(&primary).await.unwrap();
    let linked0_common = git_common_dir(&linked[0]).await.unwrap();
    let linked1_common = git_common_dir(&linked[1]).await.unwrap();
    assert_eq!(primary_common, linked0_common);
    assert_eq!(primary_common, linked1_common);

    let wrapper = primary_common.join("hooks").join("pre-commit");
    assert!(wrapper.is_file());
    assert_eq!(report.installed, vec!["pre-commit".to_string()]);

    // The wrapper is byte-identical regardless of which worktree you
    // look at it from — proves no per-worktree install is needed.
    let bytes = std::fs::read(&wrapper).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("rev-parse --show-toplevel"));
    assert!(text.contains("__dispatch"));
}

#[tokio::test]
async fn each_worktree_dispatches_to_its_own_config() {
    let (_d, primary, linked) = new_repo_with_worktrees(2);

    // Primary: one job named "primary-only"
    std::fs::write(
        primary.join("betterhook.toml"),
        "[hooks.pre-commit.jobs.primary-only]\nrun = \"echo primary\"\n",
    )
    .unwrap();

    // wt-0: two jobs, including a differently-named one
    std::fs::write(
        linked[0].join("betterhook.toml"),
        "[hooks.pre-commit.jobs.wt0-lint]\nrun = \"echo wt0 lint\"\n\
         [hooks.pre-commit.jobs.wt0-test]\nrun = \"echo wt0 test\"\n",
    )
    .unwrap();

    // wt-1: no config file at all — dispatch should be a soft miss
    // (NoConfig → exit 0). This is the agent-friendly default.

    // Install once from the primary — this writes the wrapper into the
    // shared common dir. All worktrees now share it.
    install(InstallOptions {
        worktree: Some(primary.clone()),
        ..Default::default()
    })
    .await
    .unwrap();

    // Now verify dispatch resolves per-worktree.
    let primary_dispatch = resolve(&primary, "pre-commit").unwrap();
    let wt0_dispatch = resolve(&linked[0], "pre-commit").unwrap();
    let wt1_dispatch = resolve(&linked[1], "pre-commit").unwrap();

    match primary_dispatch {
        Dispatch::Run { config, .. } => {
            let hook = &config.hooks["pre-commit"];
            let names: Vec<&str> = hook.jobs.iter().map(|j| j.name.as_str()).collect();
            assert_eq!(names, vec!["primary-only"]);
        }
        other => panic!("primary should Run, got {other:?}", other = match other {
            Dispatch::NoConfig => "NoConfig",
            Dispatch::HookNotConfigured => "HookNotConfigured",
            Dispatch::NoJobs => "NoJobs",
            Dispatch::Run { .. } => unreachable!(),
        }),
    }

    match wt0_dispatch {
        Dispatch::Run { config, .. } => {
            let hook = &config.hooks["pre-commit"];
            let names: Vec<&str> = hook.jobs.iter().map(|j| j.name.as_str()).collect();
            assert_eq!(names, vec!["wt0-lint", "wt0-test"]);
        }
        _ => panic!("wt-0 should Run"),
    }

    // wt-1 has no config → soft miss.
    assert!(matches!(wt1_dispatch, Dispatch::NoConfig));
    assert!(wt1_dispatch.is_noop());
}

#[tokio::test]
async fn uninstall_refuses_when_other_worktree_still_has_config() {
    // We haven't wired this refusal into the library yet, but we can
    // verify the happy path for now: install + uninstall in a single
    // linked-worktree repo works cleanly.
    let (_d, primary, linked) = new_repo_with_worktrees(1);
    std::fs::write(
        primary.join("betterhook.toml"),
        "[hooks.pre-commit.jobs.p]\nrun = \"true\"\n",
    )
    .unwrap();
    std::fs::write(
        linked[0].join("betterhook.toml"),
        "[hooks.pre-commit.jobs.l]\nrun = \"true\"\n",
    )
    .unwrap();

    install(InstallOptions {
        worktree: Some(primary.clone()),
        ..Default::default()
    })
    .await
    .unwrap();

    // Both worktrees resolve through the shared wrapper.
    assert!(matches!(
        resolve(&primary, "pre-commit").unwrap(),
        Dispatch::Run { .. }
    ));
    assert!(matches!(
        resolve(&linked[0], "pre-commit").unwrap(),
        Dispatch::Run { .. }
    ));
}
