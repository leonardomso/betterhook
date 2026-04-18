//! Comprehensive tests for the speculative runner orchestrator.

use std::collections::BTreeMap;
use std::path::PathBuf;

use betterhook::config::{Config, Hook, IsolateSpec, Job, Meta};
use betterhook::daemon::speculative::{
    Debouncer, HookMatcher, Speculative, SpeculativeStats, read_stats, stats_path, write_stats,
};
use betterhook::daemon::watcher::WatcherEvent;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn mk_job(name: &str, glob: Vec<&str>, concurrent_safe: bool) -> Job {
    Job {
        name: name.to_owned(),
        run: "true".to_owned(),
        fix: None,
        glob: glob.into_iter().map(str::to_owned).collect(),
        exclude: Vec::new(),
        tags: Vec::new(),
        skip: None,
        only: None,
        env: BTreeMap::new(),
        root: None,
        stage_fixed: false,
        isolate: None::<IsolateSpec>,
        timeout: None,
        interactive: false,
        fail_text: None,
        priority: 0,
        reads: Vec::new(),
        writes: Vec::new(),
        network: false,
        concurrent_safe,
        builtin: None,
    }
}

fn mk_hook(name: &str, jobs: Vec<Job>) -> Hook {
    Hook {
        name: name.to_owned(),
        parallel: false,
        fail_fast: false,
        parallel_limit: None,
        stash_untracked: false,
        jobs,
    }
}

fn mk_config(hook: Hook) -> Config {
    let mut hooks = BTreeMap::new();
    hooks.insert(hook.name.clone(), hook);
    Config {
        meta: Meta {
            version: 1,
            min_betterhook: None,
        },
        hooks,
        packages: BTreeMap::new(),
    }
}

fn watcher_event(paths: Vec<&str>) -> WatcherEvent {
    WatcherEvent {
        kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
        paths: paths.into_iter().map(PathBuf::from).collect(),
    }
}

// ---------------------------------------------------------------------------
// Debouncer tests
// ---------------------------------------------------------------------------

#[test]
fn debouncer_accepts_first_event() {
    let mut d = Debouncer::new(500);
    let p = PathBuf::from("a.ts");
    assert!(d.accept(&p));
}

#[test]
fn debouncer_rejects_within_window() {
    let mut d = Debouncer::new(500);
    let p = PathBuf::from("a.ts");
    assert!(d.accept(&p));
    assert!(!d.accept(&p), "immediate repeat should be rejected");
}

#[test]
fn debouncer_accepts_after_window() {
    // Use a 0ms window so any subsequent call is "after" the window.
    let mut d = Debouncer::new(0);
    let p = PathBuf::from("a.ts");
    assert!(d.accept(&p));
    // With a 0ms window the next call should pass.
    assert!(d.accept(&p));
}

#[test]
fn debouncer_independent_paths() {
    let mut d = Debouncer::new(500);
    let a = PathBuf::from("a.ts");
    let b = PathBuf::from("b.ts");
    assert!(d.accept(&a));
    assert!(d.accept(&b), "different paths should be independent");
}

// ---------------------------------------------------------------------------
// HookMatcher tests
// ---------------------------------------------------------------------------

#[test]
fn matcher_filters_non_concurrent_safe() {
    let hook = mk_hook(
        "pre-commit",
        vec![
            mk_job("lint-safe", vec!["*.ts"], true),
            mk_job("format-unsafe", vec!["*.ts"], false),
        ],
    );
    let m = HookMatcher::from_hook(&hook).unwrap();
    assert_eq!(m.jobs.len(), 1);
    assert_eq!(m.jobs[0].0.name, "lint-safe");
}

#[test]
fn matcher_jobs_for_matching_glob() {
    let hook = mk_hook("pre-commit", vec![mk_job("lint", vec!["*.ts"], true)]);
    let m = HookMatcher::from_hook(&hook).unwrap();
    let jobs = m.jobs_for(&PathBuf::from("a.ts"));
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "lint");
}

#[test]
fn matcher_jobs_for_non_matching_glob() {
    let hook = mk_hook("pre-commit", vec![mk_job("lint", vec!["*.ts"], true)]);
    let m = HookMatcher::from_hook(&hook).unwrap();
    let jobs = m.jobs_for(&PathBuf::from("a.rs"));
    assert!(jobs.is_empty());
}

#[test]
fn matcher_empty_glob_matches_everything() {
    let hook = mk_hook("pre-commit", vec![mk_job("lint", vec![], true)]);
    let m = HookMatcher::from_hook(&hook).unwrap();
    let jobs = m.jobs_for(&PathBuf::from("anything.xyz"));
    assert_eq!(jobs.len(), 1, "empty glob should match any file");
}

// ---------------------------------------------------------------------------
// Speculative orchestrator tests
// ---------------------------------------------------------------------------

#[test]
fn speculative_new_builds_matchers_from_config() {
    let hook = mk_hook(
        "pre-commit",
        vec![
            mk_job("lint", vec!["*.ts"], true),
            mk_job("test", vec!["*.test.ts"], true),
        ],
    );
    let config = mk_config(hook);
    let spec = Speculative::new(&config, "pre-commit", 0).unwrap();
    // The stats snapshot should be in a clean initial state.
    let stats = spec.stats();
    assert_eq!(stats.watched_worktrees, 0);
    assert!(stats.last_prewarm_ms_ago.is_none());
}

#[test]
fn speculative_handle_event_produces_tasks() {
    let hook = mk_hook("pre-commit", vec![mk_job("lint", vec!["*.ts"], true)]);
    let config = mk_config(hook);
    let mut spec = Speculative::new(&config, "pre-commit", 0).unwrap();

    let event = watcher_event(vec!["a.ts"]);
    let tasks = spec.handle_event(event);
    assert!(!tasks.is_empty());
    assert_eq!(tasks[0].jobs[0].name, "lint");
}

#[test]
fn speculative_handle_event_debounces() {
    let hook = mk_hook("pre-commit", vec![mk_job("lint", vec!["*.ts"], true)]);
    let config = mk_config(hook);
    // Use a large window to guarantee debounce.
    let mut spec = Speculative::new(&config, "pre-commit", 60_000).unwrap();

    let event1 = watcher_event(vec!["a.ts"]);
    let tasks1 = spec.handle_event(event1);
    assert!(!tasks1.is_empty());

    let event2 = watcher_event(vec!["a.ts"]);
    let tasks2 = spec.handle_event(event2);
    assert!(tasks2.is_empty(), "rapid duplicate should be debounced");
}

#[test]
fn speculative_handle_event_no_match() {
    let hook = mk_hook("pre-commit", vec![mk_job("lint", vec!["*.ts"], true)]);
    let config = mk_config(hook);
    let mut spec = Speculative::new(&config, "pre-commit", 0).unwrap();

    let event = watcher_event(vec!["a.rs"]);
    let tasks = spec.handle_event(event);
    assert!(
        tasks.is_empty(),
        "non-matching file should produce no tasks"
    );
}

// ---------------------------------------------------------------------------
// Stats sidecar tests
// ---------------------------------------------------------------------------

#[test]
fn stats_sidecar_write_read_round_trip() {
    let dir = tempfile::TempDir::new().unwrap();
    let stats = SpeculativeStats {
        watched_worktrees: 3,
        watch_count: 15,
        queue_depth: 2,
        last_prewarm_ms_ago: Some(100),
        disabled_reason: None,
    };
    write_stats(dir.path(), &stats);

    let back = read_stats(dir.path()).expect("stats file should exist");
    assert_eq!(back.watched_worktrees, 3);
    assert_eq!(back.watch_count, 15);
    assert_eq!(back.queue_depth, 2);
    assert_eq!(back.last_prewarm_ms_ago, Some(100));
}

#[test]
fn stats_read_missing_returns_none() {
    let dir = tempfile::TempDir::new().unwrap();
    assert!(read_stats(dir.path()).is_none());
}

#[test]
fn stats_path_under_betterhook_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = stats_path(dir.path());
    let s = p.to_string_lossy();
    assert!(s.contains("betterhook"), "path should contain 'betterhook'");
    assert!(s.ends_with(".json"), "path should end with .json");
}

#[test]
fn speculative_tracks_last_prewarm() {
    let hook = mk_hook("pre-commit", vec![mk_job("lint", vec!["*.ts"], true)]);
    let config = mk_config(hook);
    let mut spec = Speculative::new(&config, "pre-commit", 0).unwrap();

    assert!(spec.stats().last_prewarm_ms_ago.is_none());

    let event = watcher_event(vec!["a.ts"]);
    let tasks = spec.handle_event(event);
    assert!(!tasks.is_empty());

    assert!(
        spec.stats().last_prewarm_ms_ago.is_some(),
        "last_prewarm_ms_ago should be set after a successful prewarm"
    );
}

#[test]
fn speculative_set_watch_topology() {
    let hook = mk_hook("pre-commit", vec![mk_job("lint", vec!["*.ts"], true)]);
    let config = mk_config(hook);
    let mut spec = Speculative::new(&config, "pre-commit", 0).unwrap();

    spec.set_watch_topology(3, 12);

    let stats = spec.stats();
    assert_eq!(stats.watched_worktrees, 3);
    assert_eq!(stats.watch_count, 12);
}
