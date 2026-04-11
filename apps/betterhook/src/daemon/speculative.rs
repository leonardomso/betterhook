//! Speculative runner orchestrator.
//!
//! Phase 38 consumes [`WatcherEvent`](super::watcher::WatcherEvent)s
//! from the file watcher, debounces them per path, filters to the
//! set of `concurrent_safe` jobs whose file globs match, and emits
//! `SpeculativeTask`s to the runner.
//!
//! The actual "run job + store in CA cache" step stays in
//! `runner::executor` — this module is just the glue between the
//! watcher and the executor. That keeps the implementation small
//! and easy to reason about in isolation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use globset::GlobSet;

use crate::runner::glob_util::build_globset_always;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::config::{Config, Hook, Job};

use super::watcher::WatcherEvent;

/// Snapshot of the speculative runner's health. Phase 40 writes this
/// to `<common>/betterhook/speculative-stats.json` after every handled
/// event so `betterhook status` can read it without a socket round-trip.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpeculativeStats {
    /// Number of worktree roots the daemon is currently watching.
    pub watched_worktrees: usize,
    /// Total `notify`-level watches registered across all worktrees.
    pub watch_count: usize,
    /// Pending speculative tasks not yet drained by the runner.
    pub queue_depth: usize,
    /// Milliseconds since the last `handle_event` produced at least
    /// one task. `None` if the runner has not yet prewarmed anything.
    pub last_prewarm_ms_ago: Option<u64>,
    /// When non-empty, the speculative runner is disabled for one of
    /// the reasons listed here (e.g. `notify` init failure on NFS).
    pub disabled_reason: Option<String>,
}

/// Sidecar file name written under `<common-dir>/betterhook/`.
pub const STATS_FILENAME: &str = "speculative-stats.json";

/// Absolute path to the stats sidecar file for a given common-dir.
#[must_use]
pub fn stats_path(common_dir: &Path) -> PathBuf {
    common_dir.join("betterhook").join(STATS_FILENAME)
}

/// Best-effort read of the stats sidecar. Returns `None` when the file
/// doesn't exist or can't be decoded — both are normal for a freshly
/// installed worktree where the daemon hasn't run yet.
#[must_use]
pub fn read_stats(common_dir: &Path) -> Option<SpeculativeStats> {
    let bytes = std::fs::read(stats_path(common_dir)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Atomic write of the stats sidecar. Errors are intentionally
/// swallowed — the stats file is purely observational and must never
/// fail a commit.
pub fn write_stats(common_dir: &Path, stats: &SpeculativeStats) {
    let path = stats_path(common_dir);
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(stats) else {
        return;
    };
    let Ok(tmp) = tempfile::NamedTempFile::new_in(parent) else {
        return;
    };
    if std::fs::write(tmp.path(), &bytes).is_err() {
        return;
    }
    let _ = tmp.persist(&path);
}

/// A debounced file change plus the set of jobs that would run
/// against it. The orchestrator hands these to the scheduler.
#[derive(Debug, Clone)]
pub struct SpeculativeTask {
    /// Absolute path of the file that changed and triggered this task.
    pub file: PathBuf,
    /// Hook name (usually `pre-commit`) whose jobs should prewarm.
    pub hook_name: String,
    /// Jobs to run against `file`. Only `concurrent_safe` jobs are
    /// ever emitted here.
    pub jobs: Vec<Job>,
}

/// Per-hook compiled glob state. Cached across events so we only
/// pay the globset build cost once per config load.
pub struct HookMatcher {
    /// Hook name this matcher belongs to.
    pub hook_name: String,
    /// `(job, compiled_include_globset)` for every `concurrent_safe`
    /// job in the hook.
    pub jobs: Vec<(Job, GlobSet)>,
}

impl HookMatcher {
    /// Build matchers only for `concurrent_safe` jobs — the rest
    /// never run speculatively.
    pub fn from_hook(hook: &Hook) -> Result<Self, globset::Error> {
        let mut jobs = Vec::new();
        for job in &hook.jobs {
            if !job.concurrent_safe {
                continue;
            }
            let set = build_globset_always(&job.glob)?;
            jobs.push((job.clone(), set));
        }
        Ok(Self {
            hook_name: hook.name.clone(),
            jobs,
        })
    }

    /// Jobs whose include glob matches `file`.
    #[must_use]
    pub fn jobs_for(&self, file: &PathBuf) -> Vec<Job> {
        self.jobs
            .iter()
            .filter(|(_, set)| set.is_empty() || set.is_match(file))
            .map(|(j, _)| j.clone())
            .collect()
    }
}

/// Simple per-file time-based debounce so a burst of writes from an
/// editor (e.g. pnpm-install churn) collapses to one speculative run.
pub struct Debouncer {
    last_seen: HashMap<PathBuf, Instant>,
    window: Duration,
}

impl Debouncer {
    #[must_use]
    pub fn new(window_ms: u64) -> Self {
        Self {
            last_seen: HashMap::new(),
            window: Duration::from_millis(window_ms),
        }
    }

    /// Returns `true` if the caller should act on this path.
    pub fn accept(&mut self, path: &PathBuf) -> bool {
        let now = Instant::now();
        match self.last_seen.get(path) {
            Some(prev) if now.duration_since(*prev) < self.window => false,
            _ => {
                self.last_seen.insert(path.clone(), now);
                true
            }
        }
    }
}

/// Orchestrator that ties a file-watcher stream to a hook config and
/// emits debounced `SpeculativeTask`s.
pub struct Speculative {
    matchers: Vec<HookMatcher>,
    debouncer: Debouncer,
    stats: SpeculativeStats,
    last_prewarm: Option<SystemTime>,
}

impl Speculative {
    /// Build an orchestrator that watches for the given hook name
    /// (typically `pre-commit`) and debounces at `debounce_ms`.
    pub fn new(config: &Config, hook_name: &str, debounce_ms: u64) -> Result<Self, globset::Error> {
        let mut matchers = Vec::new();
        if let Some(hook) = config.hooks.get(hook_name) {
            matchers.push(HookMatcher::from_hook(hook)?);
        }
        for package in config.packages.values() {
            if let Some(hook) = package.hooks.get(hook_name) {
                matchers.push(HookMatcher::from_hook(hook)?);
            }
        }
        Ok(Self {
            matchers,
            debouncer: Debouncer::new(debounce_ms),
            stats: SpeculativeStats::default(),
            last_prewarm: None,
        })
    }

    /// Return a clone of the runner's current stats snapshot. Callers
    /// typically combine this with [`write_stats`] to publish the
    /// sidecar the CLI reads.
    #[must_use]
    pub fn stats(&self) -> SpeculativeStats {
        let mut out = self.stats.clone();
        out.last_prewarm_ms_ago = self
            .last_prewarm
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        out
    }

    /// Record that the daemon is watching `worktrees` distinct roots
    /// with `watches` individual OS-level watches. Mutates the snapshot
    /// but does not publish it — pair with [`write_stats`].
    pub fn set_watch_topology(&mut self, worktrees: usize, watches: usize) {
        self.stats.watched_worktrees = worktrees;
        self.stats.watch_count = watches;
    }

    /// Mark the runner as disabled with a human-readable reason.
    pub fn disable(&mut self, reason: impl Into<String>) {
        self.stats.disabled_reason = Some(reason.into());
    }

    /// Drain one watcher event and turn it into zero or more
    /// speculative tasks. Returns an empty vec if every path in
    /// the event was debounced or had no matching job.
    pub fn handle_event(&mut self, event: WatcherEvent) -> Vec<SpeculativeTask> {
        let mut tasks = Vec::new();
        for path in event.paths {
            if !self.debouncer.accept(&path) {
                continue;
            }
            for matcher in &self.matchers {
                let jobs = matcher.jobs_for(&path);
                if jobs.is_empty() {
                    continue;
                }
                tasks.push(SpeculativeTask {
                    file: path.clone(),
                    hook_name: matcher.hook_name.clone(),
                    jobs,
                });
            }
        }
        if !tasks.is_empty() {
            self.last_prewarm = Some(SystemTime::now());
        }
        self.stats.queue_depth = self.stats.queue_depth.saturating_add(tasks.len());
        tasks
    }

    /// Long-running loop that reads every watcher event and yields
    /// tasks through an mpsc sender. Returns when `rx` is closed.
    /// After every batch, the runner decrements the tracked queue
    /// depth and rewrites `<common>/betterhook/speculative-stats.json`
    /// so `betterhook status` can observe progress.
    pub async fn run(
        mut self,
        mut rx: mpsc::Receiver<WatcherEvent>,
        tasks_tx: mpsc::Sender<SpeculativeTask>,
        common_dir: PathBuf,
    ) {
        write_stats(&common_dir, &self.stats());
        while let Some(ev) = rx.recv().await {
            for task in self.handle_event(ev) {
                if tasks_tx.send(task).await.is_err() {
                    return;
                }
                self.stats.queue_depth = self.stats.queue_depth.saturating_sub(1);
            }
            write_stats(&common_dir, &self.stats());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{IsolateSpec, Job, Meta};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

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

    fn mk_config(root: Hook) -> Config {
        let mut hooks = BTreeMap::new();
        hooks.insert(root.name.clone(), root);
        Config {
            meta: Meta {
                version: 1,
                min_betterhook: None,
            },
            hooks,
            packages: BTreeMap::new(),
        }
    }

    #[test]
    fn matcher_only_collects_concurrent_safe_jobs() {
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
    fn debouncer_drops_rapid_repeats() {
        let mut d = Debouncer::new(250);
        let p = PathBuf::from("a.ts");
        assert!(d.accept(&p));
        assert!(!d.accept(&p)); // immediate repeat
    }

    #[test]
    fn stats_sidecar_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read_stats(dir.path()).is_none());
        let stats = SpeculativeStats {
            watched_worktrees: 2,
            watch_count: 8,
            queue_depth: 1,
            last_prewarm_ms_ago: Some(42),
            disabled_reason: None,
        };
        write_stats(dir.path(), &stats);
        let back = read_stats(dir.path()).expect("stats file exists");
        assert_eq!(back.watched_worktrees, 2);
        assert_eq!(back.watch_count, 8);
        assert_eq!(back.queue_depth, 1);
        assert_eq!(back.last_prewarm_ms_ago, Some(42));
    }

    #[test]
    fn stats_tracks_last_prewarm() {
        let hook = mk_hook("pre-commit", vec![mk_job("lint", vec!["*.ts"], true)]);
        let config = mk_config(hook);
        let mut spec = Speculative::new(&config, "pre-commit", 0).unwrap();
        assert!(spec.stats().last_prewarm_ms_ago.is_none());
        let event = WatcherEvent {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![PathBuf::from("a.ts")],
        };
        let tasks = spec.handle_event(event);
        assert_eq!(tasks.len(), 1);
        assert!(spec.stats().last_prewarm_ms_ago.is_some());
    }

    #[test]
    fn speculative_produces_task_per_match() {
        let hook = mk_hook("pre-commit", vec![mk_job("lint", vec!["*.ts"], true)]);
        let config = mk_config(hook);
        let mut spec = Speculative::new(&config, "pre-commit", 0).unwrap();
        let event = WatcherEvent {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![PathBuf::from("a.ts")],
        };
        let tasks = spec.handle_event(event);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].jobs[0].name, "lint");
    }
}
