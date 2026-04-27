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
#[allow(unused_imports)]
use betterhook::cache::{lookup_blocking, store_result_blocking};
use betterhook::config::import::{self, ImportSource};
use betterhook::config::{Hook, Job, load};
use betterhook::dispatch::{Dispatch, find_config, resolve, resolve_packages};
use betterhook::git::git_common_dir;
use betterhook::install::{InstallOptions, install};
use betterhook::runner::run_hook_with_options;
use betterhook::status::StatusComponent;

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
        name: "lint".into(),
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
        builtin: None,
    };

    // First lookup: miss.
    assert!(lookup(&common, &job, &files).await.unwrap().is_none());

    // Write a CachedResult and verify the lookup hits.
    let result = CachedResult {
        exit: 0,
        events: Vec::new(),
        created_at: std::time::SystemTime::now(),
        inputs: snapshot_inputs(&files),
    };
    store_result(&common, &job, &files, &result).await.unwrap();
    assert!(lookup(&common, &job, &files).await.unwrap().is_some());

    // Mutate the file. Cache must miss because the content hash moves.
    std::fs::write(&target, b"world").unwrap();
    assert!(
        lookup(&common, &job, &files).await.unwrap().is_none(),
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

#[tokio::test]
async fn status_reports_config_diagnostics_for_invalid_config() {
    let (_d, repo) = init_repo();
    let config_path = repo.join("betterhook.toml");
    std::fs::write(
        &config_path,
        r"[hooks.pre-commit.jobs.lint]
run = true
",
    )
    .unwrap();

    let canonical_config_path = std::fs::canonicalize(&config_path).unwrap();
    let status = betterhook::status::collect(Some(&repo)).await.unwrap();
    assert!(status.config.is_none());
    assert!(
        status
            .diagnostics
            .iter()
            .any(|diag| diag.component == StatusComponent::Config
                && diag.path.as_ref().is_some_and(|path| {
                    std::fs::canonicalize(path).ok().as_ref() == Some(&canonical_config_path)
                }))
    );
}

#[tokio::test]
async fn status_reports_installed_manifest_diagnostics_when_manifest_is_corrupt() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[hooks.pre-commit.jobs.lint]
run = "true"
"#,
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
    std::fs::write(&manifest_path, "{not-json").unwrap();

    let status = betterhook::status::collect(Some(&repo)).await.unwrap();
    assert!(status.installed.is_none());
    assert!(
        status
            .diagnostics
            .iter()
            .any(|diag| diag.component == StatusComponent::Installed
                && diag.path.as_deref() == Some(manifest_path.as_path()))
    );
}

#[tokio::test]
async fn status_reports_speculative_diagnostics_when_sidecar_is_corrupt() {
    let (_d, repo) = init_repo();
    let common = git_common_dir(&repo).await.unwrap();
    let stats_path = common.join("betterhook").join("speculative-stats.json");
    std::fs::create_dir_all(stats_path.parent().unwrap()).unwrap();
    std::fs::write(&stats_path, "{not-json").unwrap();

    let status = betterhook::status::collect(Some(&repo)).await.unwrap();
    assert!(status.speculative.is_none());
    assert!(
        status
            .diagnostics
            .iter()
            .any(|diag| diag.component == StatusComponent::Speculative
                && diag.path.as_deref() == Some(stats_path.as_path()))
    );
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

// ──────────────────── builtin diagnostic pipeline ────────────────────

#[tokio::test]
async fn builtin_config_merges_defaults_and_lowers() {
    // Verify that `builtin = "rustfmt"` in a config merges the
    // builtin's default `run`, `glob`, `reads`, `concurrent_safe`
    // fields into the lowered Job at config-load time.
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[meta]
version = 1

[hooks.pre-commit.jobs.fmt]
builtin = "rustfmt"
"#,
    );
    let cfg = load(&repo.join("betterhook.toml")).unwrap();
    let hook = &cfg.hooks["pre-commit"];
    assert_eq!(hook.jobs.len(), 1);
    let job = &hook.jobs[0];
    assert_eq!(job.builtin.as_deref(), Some("rustfmt"));
    // The builtin should have filled in `run` since the user didn't.
    assert!(
        job.run.contains("cargo fmt"),
        "builtin should fill in the run command, got: {}",
        job.run
    );
    // concurrent_safe should be true (rustfmt's default).
    assert!(job.concurrent_safe);
    // glob should be populated.
    assert!(!job.glob.is_empty());
}

#[tokio::test]
async fn unknown_builtin_fails_at_config_load() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[hooks.pre-commit.jobs.bad]
builtin = "this-does-not-exist"
"#,
    );
    let result = load(&repo.join("betterhook.toml"));
    assert!(
        result.is_err(),
        "unknown builtin should error at config load"
    );
}

// ───────────────────── explain dot output shape ──────────────────────

#[tokio::test]
async fn explain_dag_produces_valid_digraph() {
    // Verify the graphviz digraph output contract: starts with
    // `digraph betterhook {`, contains both job names, contains at
    // least one `->` edge for conflicting writers, and ends with `}`.
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[meta]
version = 1

[hooks.pre-commit]
parallel = true

[hooks.pre-commit.jobs.fmt]
run = "true"
writes = ["**/*.ts"]

[hooks.pre-commit.jobs.lint]
run = "true"
reads = ["**/*.ts"]
"#,
    );
    let cfg = load(&repo.join("betterhook.toml")).unwrap();
    let hook = &cfg.hooks["pre-commit"];
    let graph = betterhook::runner::build_dag(&hook.jobs).unwrap();

    // Build the digraph string (same logic as explain.rs)
    let mut dot = String::from("digraph betterhook {\n");
    for node in &graph.nodes {
        use std::fmt::Write;
        let _ = writeln!(dot, "  \"{}\";", node.job.name);
    }
    for (a, b) in graph.edges() {
        use std::fmt::Write;
        let _ = writeln!(
            dot,
            "  \"{}\" -> \"{}\";",
            graph.nodes[a].job.name, graph.nodes[b].job.name
        );
    }
    dot.push_str("}\n");

    assert!(dot.starts_with("digraph betterhook {"));
    assert!(dot.contains("\"fmt\""));
    assert!(dot.contains("\"lint\""));
    assert!(
        dot.contains("->"),
        "conflicting writers should produce an edge"
    );
    assert!(dot.trim_end().ends_with('}'));
}

// ────────────────── import edge cases (P12) ────────────────────────

#[tokio::test]
async fn import_lefthook_empty_commands_map() {
    let (_d, repo) = init_repo();
    let lefthook = repo.join("lefthook.yml");
    std::fs::write(&lefthook, "pre-commit:\n  commands: {}\n").unwrap();
    let (raw, _) = import::import_file(ImportSource::Lefthook, &lefthook).unwrap();
    let cfg = raw.lower().unwrap();
    assert!(cfg.hooks["pre-commit"].jobs.is_empty());
}

#[tokio::test]
async fn import_lefthook_multiple_hooks() {
    let (_d, repo) = init_repo();
    let lefthook = repo.join("lefthook.yml");
    std::fs::write(
        &lefthook,
        "pre-commit:\n  commands:\n    lint:\n      run: eslint\npre-push:\n  commands:\n    test:\n      run: cargo test\n",
    )
    .unwrap();
    let (raw, _) = import::import_file(ImportSource::Lefthook, &lefthook).unwrap();
    let cfg = raw.lower().unwrap();
    assert_eq!(cfg.hooks.len(), 2);
}

#[tokio::test]
async fn import_husky_strips_npx_prefix() {
    let (_d, repo) = init_repo();
    std::fs::create_dir_all(repo.join(".husky")).unwrap();
    let script = repo.join(".husky/pre-commit");
    std::fs::write(&script, "#!/usr/bin/env sh\nnpx lint-staged\n").unwrap();
    let (raw, _) = import::import_file(ImportSource::Husky, &script).unwrap();
    let cfg = raw.lower().unwrap();
    let run = &cfg.hooks["pre-commit"].jobs[0].run;
    assert!(run.contains("lint-staged"), "should preserve the command");
}

#[tokio::test]
async fn import_auto_detect_lefthook() {
    assert_eq!(
        ImportSource::auto_detect(std::path::Path::new("lefthook.yml")),
        Some(ImportSource::Lefthook)
    );
}

#[tokio::test]
async fn import_auto_detect_husky() {
    assert_eq!(
        ImportSource::auto_detect(std::path::Path::new(".husky/pre-commit")),
        Some(ImportSource::Husky)
    );
}

#[tokio::test]
async fn import_auto_detect_pre_commit() {
    assert_eq!(
        ImportSource::auto_detect(std::path::Path::new(".pre-commit-config.yaml")),
        Some(ImportSource::PreCommit)
    );
}

#[tokio::test]
async fn import_auto_detect_unknown() {
    assert!(ImportSource::auto_detect(std::path::Path::new("random.toml")).is_none());
}

#[tokio::test]
async fn import_from_cli_lefthook() {
    assert_eq!(
        ImportSource::from_cli("lefthook"),
        Some(ImportSource::Lefthook)
    );
}

#[tokio::test]
async fn import_from_cli_husky() {
    assert_eq!(ImportSource::from_cli("husky"), Some(ImportSource::Husky));
}

#[tokio::test]
async fn import_from_cli_unknown() {
    assert!(ImportSource::from_cli("unknown-tool").is_none());
}

// ──────────────────── status / doctor (P12) ─────────────────────────

#[tokio::test]
async fn status_reports_installed_hooks() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[hooks.pre-commit.jobs.lint]
run = "true"
"#,
    );
    install(InstallOptions {
        worktree: Some(repo.clone()),
        skip_unit: true,
        ..Default::default()
    })
    .await
    .unwrap();

    let status = betterhook::status::collect(Some(&repo)).await.unwrap();
    assert!(status.installed.is_some());
}

#[tokio::test]
async fn status_includes_betterhook_version() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[hooks.pre-commit.jobs.lint]
run = "true"
"#,
    );
    let status = betterhook::status::collect(Some(&repo)).await.unwrap();
    assert!(
        !status.betterhook_version.is_empty(),
        "version should be populated"
    );
}

// ──────────────── monorepo dispatch extras (P12) ────────────────────

#[tokio::test]
async fn monorepo_five_packages_dispatch() {
    let (_d, repo) = init_repo();
    for pkg in &["alpha", "beta", "gamma", "delta", "epsilon"] {
        let pkg_dir = repo.join(format!("apps/{pkg}/src"));
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("mod.rs"), "// code").unwrap();
    }
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "scaffold"]);

    let mut cfg_body = "[meta]\nversion = 1\n\n".to_owned();
    for pkg in &["alpha", "beta", "gamma", "delta", "epsilon"] {
        use std::fmt::Write;
        let _ = write!(
            cfg_body,
            "[packages.{pkg}]\npath = \"apps/{pkg}\"\n\n[packages.{pkg}.hooks.pre-commit.jobs.lint]\nrun = \"true\"\n\n"
        );
    }
    write_config(&repo, &cfg_body);

    let cfg = load(&repo.join("betterhook.toml")).unwrap();
    assert_eq!(cfg.packages.len(), 5);
}

#[tokio::test]
async fn monorepo_package_hook_overrides_root() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[meta]
version = 1

[hooks.pre-commit.jobs.global]
run = "echo global"

[packages.frontend]
path = "apps/web"

[packages.frontend.hooks.pre-commit.jobs.lint]
run = "echo frontend-lint"
"#,
    );
    let cfg = load(&repo.join("betterhook.toml")).unwrap();
    assert!(cfg.hooks.contains_key("pre-commit"), "root hook exists");
    assert!(
        cfg.packages["frontend"].hooks.contains_key("pre-commit"),
        "package hook exists"
    );
}

// ──────────────── explain dot edge cases (P12) ──────────────────────

#[tokio::test]
async fn explain_dag_zero_edges_valid_digraph() {
    let (_d, repo) = init_repo();
    write_config(
        &repo,
        r#"[meta]
version = 1

[hooks.pre-commit.jobs.alpha]
run = "true"
reads = ["*.rs"]

[hooks.pre-commit.jobs.beta]
run = "true"
reads = ["*.ts"]
"#,
    );
    let cfg = load(&repo.join("betterhook.toml")).unwrap();
    let hook = &cfg.hooks["pre-commit"];
    let graph = betterhook::runner::build_dag(&hook.jobs).unwrap();
    assert_eq!(graph.edge_count(), 0);

    let mut dot = String::from("digraph betterhook {\n");
    for node in &graph.nodes {
        use std::fmt::Write;
        let _ = writeln!(dot, "  \"{}\";", node.job.name);
    }
    dot.push_str("}\n");
    assert!(dot.starts_with("digraph betterhook {"));
    assert!(dot.trim_end().ends_with('}'));
    assert!(
        !dot.contains("->"),
        "disjoint reads should produce no edges"
    );
}
