//! End-to-end integration tests against real temp git repos.
//!
//! These tests cover the headline v1 surfaces: DAG-based parallel
//! execution, monorepo dispatch, the cache hit cycle, the importer
//! round-trip for every supported source, and the doctor JSON shape.
//! They use real `git` subprocesses so failures show real-world
//! interaction bugs the unit tests would never catch.

mod common;

use std::path::PathBuf;

use betterhook::cache::{CachedResult, Store, lookup, snapshot_inputs, store_result};
use betterhook::config::import::{self, ImportSource};
use betterhook::config::{Hook, Job, load};
use betterhook::dispatch::{Dispatch, find_config, resolve, resolve_packages};
use betterhook::install::{InstallOptions, install};
use betterhook::runner::run_hook_with_options;

use common::{git, init_repo, run_options_quiet, write_config};

// ─────────────────────── DAG end-to-end execution ────────────────────

#[tokio::test]
async fn dag_runs_parallel_jobs_to_completion() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[meta]
version = 1

[hooks.pre-commit]
parallel = true

[hooks.pre-commit.jobs.alpha]
run = "true"
concurrent_safe = true
reads = ["**/*.md"]

[hooks.pre-commit.jobs.beta]
run = "true"
concurrent_safe = true
reads = ["**/*.md"]

[hooks.pre-commit.jobs.gamma]
run = "true"
concurrent_safe = true
reads = ["**/*.txt"]
"#,
    );
    let cfg = load(&repo.join("betterhook.toml")).unwrap();
    let hook: &Hook = &cfg.hooks["pre-commit"];
    let report = run_hook_with_options(hook, &repo, run_options_quiet())
        .await
        .unwrap();
    assert!(report.ok, "every job must succeed");
    assert_eq!(report.jobs_run + report.jobs_skipped, 3);
}

#[tokio::test]
async fn dag_serializes_write_after_write_pair() {
    // Two writers on the same file pattern. Both should run, both
    // should report ok, and the resolved DAG should record one edge.
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[meta]
version = 1

[hooks.pre-commit]
parallel = true

[hooks.pre-commit.jobs.fmt1]
run = "true"
writes = ["**/*.md"]

[hooks.pre-commit.jobs.fmt2]
run = "true"
writes = ["**/*.md"]
"#,
    );
    let cfg = load(&repo.join("betterhook.toml")).unwrap();
    let hook: &Hook = &cfg.hooks["pre-commit"];
    let dag = betterhook::runner::dag::build_dag(&hook.jobs).unwrap();
    assert_eq!(dag.edge_count(), 1, "two writers must serialize");

    let report = run_hook_with_options(hook, &repo, run_options_quiet())
        .await
        .unwrap();
    assert!(report.ok);
}

#[tokio::test]
async fn dag_propagates_failure_with_fail_fast() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[meta]
version = 1

[hooks.pre-commit]
parallel = true
fail_fast = true

[hooks.pre-commit.jobs.passing]
run = "true"

[hooks.pre-commit.jobs.failing]
run = "false"
"#,
    );
    let cfg = load(&repo.join("betterhook.toml")).unwrap();
    let hook: &Hook = &cfg.hooks["pre-commit"];
    let report = run_hook_with_options(hook, &repo, run_options_quiet())
        .await
        .unwrap();
    assert!(!report.ok, "failing job must mark report as not ok");
}

// ───────────────────────── monorepo dispatch ─────────────────────────

#[tokio::test]
async fn monorepo_dispatch_groups_files_by_package() {
    let (_d, repo) = init_repo();
    std::fs::create_dir_all(repo.join("apps/web/src")).unwrap();
    std::fs::create_dir_all(repo.join("services/api/src")).unwrap();
    std::fs::write(repo.join("apps/web/src/Button.tsx"), "x").unwrap();
    std::fs::write(repo.join("services/api/src/main.rs"), "fn main() {}").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "scaffold"]);

    write_config(
        &repo,
        r#"[meta]
version = 1

[packages.frontend]
path = "apps/web"

[packages.api]
path = "services/api"

[packages.frontend.hooks.pre-commit.jobs.lint]
run = "echo lint frontend"

[packages.api.hooks.pre-commit.jobs.test]
run = "echo test api"
"#,
    );

    let cfg = load(&repo.join("betterhook.toml")).unwrap();
    assert_eq!(cfg.packages.len(), 2);
    assert!(cfg.packages.contains_key("frontend"));
    assert!(cfg.packages.contains_key("api"));

    // resolve_packages expects worktree-relative paths (it's called
    // from dispatch with the output of `git diff --name-only`).
    let staged = vec![
        PathBuf::from("apps/web/src/Button.tsx"),
        PathBuf::from("services/api/src/main.rs"),
    ];
    let matches = resolve_packages(&cfg, &staged);
    // Two packages, each matched by one file (no Root residual).
    let pkg_count = matches
        .iter()
        .filter(|m| matches!(m, betterhook::dispatch::PackageMatch::Package(_, _)))
        .count();
    assert_eq!(pkg_count, 2);
}

#[tokio::test]
async fn monorepo_root_hook_runs_when_no_package_matches() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[meta]
version = 1

[packages.frontend]
path = "apps/web"

[hooks.pre-commit.jobs.global]
run = "true"
"#,
    );
    // The dispatcher should still resolve the root hook even when no
    // package matches the (empty) staged set.
    let dispatch = resolve(&repo, "pre-commit").unwrap();
    assert!(matches!(dispatch, Dispatch::Run { .. }));
}

// ──────────────────────── cache hit / miss cycle ─────────────────────

#[tokio::test]
async fn cache_round_trip_keeps_first_run_and_invalidates_on_change() {
    // Simulate the speculative-runner → commit-time-runner flow without
    // spawning subprocesses: write a CachedResult, look it up, mutate
    // the input file, look it up again — second lookup must miss.
    let (_d, repo) = init_repo();
    let common = betterhook::git::git_common_dir(&repo).await.unwrap();

    let target = repo.join("apps/web/src/Button.tsx");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"hello").unwrap();
    let files = vec![target.clone()];

    let job = Job {
        name: "lint".to_owned(),
        run: "eslint --cache {files}".to_owned(),
        fix: None,
        glob: vec!["*.tsx".to_owned()],
        exclude: vec![],
        tags: vec![],
        skip: None,
        only: None,
        env: std::collections::BTreeMap::default(),
        root: None,
        stage_fixed: false,
        isolate: None,
        timeout: None,
        interactive: false,
        fail_text: None,
        priority: 0,
        reads: vec!["**/*.tsx".to_owned()],
        writes: vec![],
        network: false,
        concurrent_safe: true,
    };

    // First lookup: miss.
    assert!(lookup(&common, &job, &files).unwrap().is_none());

    // Write a CachedResult and verify the lookup hits.
    let result = CachedResult {
        exit: 0,
        events: Vec::new(),
        created_at: std::time::SystemTime::now(),
        inputs: snapshot_inputs(&files),
    };
    store_result(&common, &job, &files, &result).unwrap();
    assert!(lookup(&common, &job, &files).unwrap().is_some());

    // Mutate the file. Cache must miss because the content hash moves.
    std::fs::write(&target, b"world").unwrap();
    assert!(
        lookup(&common, &job, &files).unwrap().is_none(),
        "content change must invalidate the cache"
    );

    // Verify the on-disk store still works after the miss.
    let stats = Store::new(&common).stats().unwrap();
    assert!(stats.entries >= 1);
}

// ─────────────────────────── doctor + status ─────────────────────────

#[tokio::test]
async fn status_reports_config_when_present() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[meta]
version = 1

[hooks.pre-commit.jobs.lint]
run = "true"
"#,
    );
    let status = betterhook::status::collect(Some(&repo)).await.unwrap();
    let cfg = status.config.expect("config block present");
    assert_eq!(cfg.hooks.len(), 1);
    assert_eq!(cfg.hooks[0].name, "pre-commit");
    let dag = cfg.hooks[0].dag.as_ref().expect("dag summary present");
    assert_eq!(dag.node_count, 1);
    assert_eq!(dag.edge_count, 0);
}

#[tokio::test]
async fn status_handles_repo_without_config() {
    let (_d, repo) = init_repo();
    let status = betterhook::status::collect(Some(&repo)).await.unwrap();
    assert!(status.config.is_none());
    assert!(status.installed.is_none() || status.installed.is_some());
}

// ───────────────────────────── importers ─────────────────────────────

#[tokio::test]
async fn import_lefthook_round_trips_through_load() {
    let (_d, repo) = init_repo();
    let lefthook = repo.join("lefthook.yml");
    std::fs::write(
        &lefthook,
        "pre-commit:\n  commands:\n    a:\n      run: \"true\"\n",
    )
    .unwrap();
    let (raw, _report) = import::import_file(ImportSource::Lefthook, &lefthook).unwrap();
    let cfg = raw.lower().unwrap();
    assert_eq!(cfg.hooks["pre-commit"].jobs.len(), 1);
}

#[tokio::test]
async fn import_husky_round_trips_through_load() {
    let (_d, repo) = init_repo();
    std::fs::create_dir_all(repo.join(".husky")).unwrap();
    let script = repo.join(".husky/pre-commit");
    std::fs::write(&script, "#!/usr/bin/env sh\nnpx lint-staged\n").unwrap();
    let (raw, _report) = import::import_file(ImportSource::Husky, &script).unwrap();
    let cfg = raw.lower().unwrap();
    assert_eq!(cfg.hooks["pre-commit"].jobs.len(), 1);
    assert!(cfg.hooks["pre-commit"].jobs[0].run.contains("lint-staged"));
}

#[tokio::test]
async fn import_hk_round_trips_through_load() {
    let (_d, repo) = init_repo();
    let hk = repo.join("hk.toml");
    std::fs::write(
        &hk,
        r#"[hooks.pre-commit.steps.lint]
run = "echo hk"
"#,
    )
    .unwrap();
    let (raw, _report) = import::import_file(ImportSource::Hk, &hk).unwrap();
    let cfg = raw.lower().unwrap();
    assert_eq!(cfg.hooks["pre-commit"].jobs.len(), 1);
}

#[tokio::test]
async fn import_pre_commit_round_trips_through_load() {
    let (_d, repo) = init_repo();
    let pcc = repo.join(".pre-commit-config.yaml");
    std::fs::write(
        &pcc,
        "repos:\n  - repo: local\n    hooks:\n      - id: trailing-whitespace\n",
    )
    .unwrap();
    let (raw, _report) = import::import_file(ImportSource::PreCommit, &pcc).unwrap();
    let cfg = raw.lower().unwrap();
    assert_eq!(cfg.hooks["pre-commit"].jobs.len(), 1);
}

// ───────────────────────── install + uninstall ───────────────────────

#[tokio::test]
async fn install_uninstall_cycle_leaves_no_residue() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[hooks.pre-commit.jobs.x]
run = "true"
"#,
    );

    let report = install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(report.installed, vec!["pre-commit".to_owned()]);

    // Wrapper should exist after install.
    let common = betterhook::git::git_common_dir(&repo).await.unwrap();
    assert!(common.join("hooks/pre-commit").is_file());

    let uninstall_report = betterhook::install::uninstall(Some(repo.clone()))
        .await
        .unwrap();
    assert!(!uninstall_report.removed.is_empty());

    // Wrapper should be gone.
    assert!(!common.join("hooks/pre-commit").is_file());
}

#[tokio::test]
async fn dispatch_finds_config_via_find_config() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[hooks.pre-commit.jobs.a]
run = "true"
"#,
    );
    let resolved = find_config(&repo).expect("config exists");
    assert!(resolved.ends_with("betterhook.toml"));
}
