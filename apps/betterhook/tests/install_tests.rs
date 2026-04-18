//! Comprehensive integration tests for the worktree-aware install and
//! uninstall system. Covers wrapper creation, permissions, manifest
//! writing, hook filtering, core.hooksPath handling, idempotency,
//! uninstall, and multi-worktree dispatch.

mod common;

use std::os::unix::fs::PermissionsExt;

use betterhook::dispatch::{Dispatch, resolve};
use betterhook::git::git_common_dir;
use betterhook::install::{InstallOptions, install, uninstall};

use common::{git, init_repo, new_repo_with_worktrees, write_config};

// ---------------------------------------------------------------------------
// 1. install_creates_wrapper_in_hooks_dir
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_creates_wrapper_in_hooks_dir() {
    let (_d, repo) = init_repo();
    write_config(&repo, "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n");

    let report = install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let common = git_common_dir(&repo).await.unwrap();
    let wrapper = common.join("hooks").join("pre-commit");
    assert!(
        wrapper.is_file(),
        "wrapper file must exist at {}",
        wrapper.display()
    );
    assert_eq!(report.installed, vec!["pre-commit".to_string()]);
}

// ---------------------------------------------------------------------------
// 2. install_wrapper_is_executable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_wrapper_is_executable() {
    let (_d, repo) = init_repo();
    write_config(&repo, "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n");

    install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let common = git_common_dir(&repo).await.unwrap();
    let wrapper = common.join("hooks").join("pre-commit");
    let perms = std::fs::metadata(&wrapper).unwrap().permissions();
    assert_eq!(
        perms.mode() & 0o111,
        0o111,
        "wrapper must have execute bits set"
    );
}

// ---------------------------------------------------------------------------
// 3. install_writes_manifest_json
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_writes_manifest_json() {
    let (_d, repo) = init_repo();
    write_config(&repo, "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n");

    install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let common = git_common_dir(&repo).await.unwrap();
    let manifest = common.join("betterhook").join("installed.json");
    assert!(
        manifest.is_file(),
        "installed.json must exist at {}",
        manifest.display()
    );
}

// ---------------------------------------------------------------------------
// 4. install_only_specified_hooks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_only_specified_hooks() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n\
         [hooks.pre-push.jobs.test]\nrun = \"echo test\"\n",
    );

    let report = install(InstallOptions {
        worktree: Some(repo.clone()),
        only_hooks: Some(vec!["pre-commit".to_string()]),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    assert_eq!(report.installed, vec!["pre-commit".to_string()]);

    let common = git_common_dir(&repo).await.unwrap();
    let hooks_dir = common.join("hooks");
    assert!(hooks_dir.join("pre-commit").is_file());
    assert!(
        !hooks_dir.join("pre-push").is_file(),
        "pre-push must NOT be installed when only_hooks filters it out"
    );
}

// ---------------------------------------------------------------------------
// 5. install_refuses_foreign_core_hookspath
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_refuses_foreign_core_hookspath() {
    let (_d, repo) = init_repo();
    write_config(&repo, "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n");
    git(&repo, &["config", "core.hooksPath", "/tmp/other"]);

    let err = install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap_err();

    assert!(
        matches!(
            err,
            betterhook::install::InstallError::ForeignCoreHooksPath { .. }
        ),
        "expected ForeignCoreHooksPath, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. install_takeover_unsets_core_hookspath
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_takeover_unsets_core_hookspath() {
    let (_d, repo) = init_repo();
    write_config(&repo, "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n");
    git(&repo, &["config", "core.hooksPath", "/tmp/other"]);

    install(InstallOptions {
        worktree: Some(repo.clone()),
        takeover: true,
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    // Verify core.hooksPath is unset — `git config --get` exits 1 when key absent
    let out = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["config", "--get", "core.hooksPath"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "core.hooksPath should be unset after takeover install"
    );
}

// ---------------------------------------------------------------------------
// 7. install_idempotent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_idempotent() {
    let (_d, repo) = init_repo();
    write_config(&repo, "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n");

    let first = install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let first_bytes = std::fs::read(first.hooks_dir.join("pre-commit")).unwrap();

    let second = install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let second_bytes = std::fs::read(second.hooks_dir.join("pre-commit")).unwrap();

    assert_eq!(
        first_bytes, second_bytes,
        "wrappers must be byte-identical across re-installs"
    );
}

// ---------------------------------------------------------------------------
// 8. uninstall_removes_wrappers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn uninstall_removes_wrappers() {
    let (_d, repo) = init_repo();
    write_config(&repo, "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n");

    let report = install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let wrapper = report.hooks_dir.join("pre-commit");
    assert!(wrapper.is_file(), "wrapper must exist before uninstall");

    let un = uninstall(Some(repo.clone())).await.unwrap();
    assert_eq!(un.removed, vec!["pre-commit".to_string()]);
    assert!(!wrapper.is_file(), "wrapper must be gone after uninstall");
}

// ---------------------------------------------------------------------------
// 9. uninstall_refuses_when_not_installed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn uninstall_refuses_when_not_installed() {
    let (_d, repo) = init_repo();
    // No install performed — uninstall should fail.
    let err = uninstall(Some(repo.clone())).await.unwrap_err();
    assert!(
        matches!(err, betterhook::install::InstallError::NotInstalled { .. }),
        "expected NotInstalled, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 10. shared_wrapper_across_worktrees
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shared_wrapper_across_worktrees() {
    let (_d, primary, linked) = new_repo_with_worktrees(3);
    write_config(
        &primary,
        "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n",
    );

    install(InstallOptions {
        worktree: Some(primary.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let primary_common = git_common_dir(&primary).await.unwrap();
    let wrapper = primary_common.join("hooks").join("pre-commit");
    assert!(wrapper.is_file());

    // Every linked worktree should resolve the same common dir, so the
    // same wrapper is visible from each.
    for wt in &linked {
        let wt_common = git_common_dir(wt).await.unwrap();
        assert_eq!(
            primary_common, wt_common,
            "all worktrees must share the same common dir"
        );
    }
}

// ---------------------------------------------------------------------------
// 11. each_worktree_dispatches_own_config
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_worktree_dispatches_own_config() {
    let (_d, primary, linked) = new_repo_with_worktrees(2);

    write_config(
        &primary,
        "[hooks.pre-commit.jobs.primary-job]\nrun = \"echo primary\"\n",
    );
    write_config(
        &linked[0],
        "[hooks.pre-commit.jobs.wt0-lint]\nrun = \"echo wt0 lint\"\n\
         [hooks.pre-commit.jobs.wt0-test]\nrun = \"echo wt0 test\"\n",
    );

    install(InstallOptions {
        worktree: Some(primary.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    // Primary should resolve its own config.
    match resolve(&primary, "pre-commit").unwrap() {
        Dispatch::Run { config, .. } => {
            let names: Vec<&str> = config.hooks["pre-commit"]
                .jobs
                .iter()
                .map(|j| j.name.as_str())
                .collect();
            assert_eq!(names, vec!["primary-job"]);
        }
        other => panic!(
            "primary should Dispatch::Run, got {:?}",
            dispatch_tag(&other)
        ),
    }

    // wt-0 should resolve its own (different) config.
    match resolve(&linked[0], "pre-commit").unwrap() {
        Dispatch::Run { config, .. } => {
            let names: Vec<&str> = config.hooks["pre-commit"]
                .jobs
                .iter()
                .map(|j| j.name.as_str())
                .collect();
            assert_eq!(names, vec!["wt0-lint", "wt0-test"]);
        }
        other => panic!("wt-0 should Dispatch::Run, got {:?}", dispatch_tag(&other)),
    }
}

// ---------------------------------------------------------------------------
// 12. worktree_without_config_returns_noconfig
// ---------------------------------------------------------------------------

#[tokio::test]
async fn worktree_without_config_returns_noconfig() {
    let (_d, primary, linked) = new_repo_with_worktrees(1);

    write_config(
        &primary,
        "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n",
    );
    // linked[0] has NO betterhook.toml

    install(InstallOptions {
        worktree: Some(primary.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let dispatch = resolve(&linked[0], "pre-commit").unwrap();
    assert!(
        matches!(dispatch, Dispatch::NoConfig),
        "linked worktree without config should return NoConfig"
    );
}

// ---------------------------------------------------------------------------
// 13. install_with_no_config_file_errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_with_no_config_file_errors() {
    let (_d, repo) = init_repo();
    // No config file written — install should fail.
    let err = install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap_err();

    assert!(
        matches!(err, betterhook::install::InstallError::ConfigMissing { .. }),
        "expected ConfigMissing, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 14. install_creates_multiple_hook_wrappers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_creates_multiple_hook_wrappers() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n\
         [hooks.commit-msg.jobs.msg]\nrun = \"echo msg\"\n\
         [hooks.pre-push.jobs.test]\nrun = \"echo test\"\n",
    );

    let report = install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let common = git_common_dir(&repo).await.unwrap();
    let hooks_dir = common.join("hooks");

    for hook in &["pre-commit", "commit-msg", "pre-push"] {
        assert!(
            hooks_dir.join(hook).is_file(),
            "wrapper for {hook} must exist"
        );
    }

    // All three should appear in the report (order may vary since config
    // keys come from a map).
    assert_eq!(report.installed.len(), 3);
    for hook in &["pre-commit", "commit-msg", "pre-push"] {
        assert!(
            report.installed.contains(&hook.to_string()),
            "{hook} missing from report.installed"
        );
    }
}

// ---------------------------------------------------------------------------
// 15. wrapper_contains_dispatch_command
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrapper_contains_dispatch_command() {
    let (_d, repo) = init_repo();
    write_config(&repo, "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n");

    install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let common = git_common_dir(&repo).await.unwrap();
    let wrapper = common.join("hooks").join("pre-commit");
    let text = std::fs::read_to_string(&wrapper).unwrap();

    assert!(
        text.contains("__dispatch"),
        "wrapper must contain __dispatch command"
    );
    assert!(
        text.contains("rev-parse"),
        "wrapper must contain rev-parse for worktree detection"
    );
    assert!(
        text.starts_with("#!/usr/bin/env sh"),
        "wrapper must start with a shebang"
    );
}

// ---------------------------------------------------------------------------
// 16. uninstall_removes_manifest
// ---------------------------------------------------------------------------

#[tokio::test]
async fn uninstall_removes_manifest() {
    let (_d, repo) = init_repo();
    write_config(&repo, "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n");

    install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let common = git_common_dir(&repo).await.unwrap();
    let manifest = common.join("betterhook").join("installed.json");
    assert!(manifest.is_file(), "manifest must exist before uninstall");

    uninstall(Some(repo.clone())).await.unwrap();
    assert!(
        !manifest.is_file(),
        "manifest must be removed after uninstall"
    );
}

// ---------------------------------------------------------------------------
// 17. install_manifest_contains_hook_entries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_manifest_contains_hook_entries() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        "[hooks.pre-commit.jobs.lint]\nrun = \"echo lint\"\n\
         [hooks.pre-push.jobs.test]\nrun = \"echo test\"\n",
    );

    install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let common = git_common_dir(&repo).await.unwrap();
    let manifest_path = common.join("betterhook").join("installed.json");
    let manifest_text = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_text).unwrap();

    let hooks = manifest["hooks"].as_object().unwrap();
    assert!(
        hooks.contains_key("pre-commit"),
        "manifest must list pre-commit"
    );
    assert!(
        hooks.contains_key("pre-push"),
        "manifest must list pre-push"
    );

    // Each hook entry should have a sha256: prefixed value
    for (_name, sha) in hooks {
        let s = sha.as_str().unwrap();
        assert!(
            s.starts_with("sha256:"),
            "hook SHA must be sha256-prefixed, got {s}"
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Human-readable tag for Dispatch variants (for panic messages).
fn dispatch_tag(d: &Dispatch) -> &'static str {
    match d {
        Dispatch::NoConfig => "NoConfig",
        Dispatch::HookNotConfigured => "HookNotConfigured",
        Dispatch::NoJobs => "NoJobs",
        Dispatch::Run { .. } => "Run",
    }
}
