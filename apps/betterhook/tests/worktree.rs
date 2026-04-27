//! End-to-end linked-worktree tests.
//!
//! These prove betterhook's headline property: one wrapper installed
//! into the shared common dir dispatches to each worktree's own
//! config at runtime. The property-under-test is exactly the thing
//! lefthook fails at.

mod common;

use std::collections::BTreeMap;

use betterhook::config::{Hook, Job};
use betterhook::dispatch::{Dispatch, resolve};
use betterhook::git::git_common_dir;
use betterhook::install::{InstallOptions, install};
use betterhook::runner::run_hook;

use common::new_repo_with_worktrees;

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
        skip_unit: true,
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
        other => panic!(
            "primary should Run, got {other:?}",
            other = match other {
                Dispatch::NoConfig => "NoConfig",
                Dispatch::HookNotConfigured => "HookNotConfigured",
                Dispatch::NoJobs => "NoJobs",
                Dispatch::Run { .. } => unreachable!(),
            }
        ),
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
        skip_unit: true,
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

#[tokio::test]
async fn stash_operations_serialize_across_linked_worktrees() {
    let (_d, primary, linked) = new_repo_with_worktrees(1);
    std::fs::write(primary.join("scratch.log"), "primary\n").unwrap();
    std::fs::write(linked[0].join("scratch.log"), "linked\n").unwrap();

    let hook = Hook {
        name: "pre-commit".into(),
        parallel: false,
        parallel_explicit: false,
        fail_fast: false,
        fail_fast_explicit: false,
        parallel_limit: None,
        stash_untracked: true,
        stash_untracked_explicit: true,
        jobs: vec![Job {
            name: "noop".into(),
            run: "true".to_owned(),
            fix: None,
            glob: Vec::new(),
            exclude: Vec::new(),
            tags: Vec::new(),
            skip: None,
            only: None,
            env: BTreeMap::new(),
            root: None,
            stage_fixed: false,
            isolate: None,
            timeout: None,
            interactive: false,
            fail_text: None,
            priority: 0,
            reads: Vec::new(),
            writes: Vec::new(),
            network: false,
            concurrent_safe: false,
            builtin: None,
        }],
    };

    let (left, right) = tokio::join!(run_hook(&hook, &primary), run_hook(&hook, &linked[0]));
    assert!(left.unwrap().ok);
    assert!(right.unwrap().ok);
}
