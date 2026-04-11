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
use std::path::PathBuf;
use std::time::{Duration, Instant};

use globset::{Glob, GlobSet, GlobSetBuilder};
use tokio::sync::mpsc;

use crate::config::{Config, Hook, Job};

use super::watcher::WatcherEvent;

/// A debounced file change plus the set of jobs that would run
/// against it. The orchestrator hands these to the scheduler.
#[derive(Debug, Clone)]
pub struct SpeculativeTask {
    pub file: PathBuf,
    pub hook_name: String,
    pub jobs: Vec<Job>,
}

/// Per-hook compiled glob state. Cached across events so we only
/// pay the globset build cost once per config load.
pub struct HookMatcher {
    pub hook_name: String,
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
            let mut builder = GlobSetBuilder::new();
            for pat in &job.glob {
                builder.add(Glob::new(pat)?);
            }
            let set = builder.build()?;
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
        })
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
        tasks
    }

    /// Long-running loop that reads every watcher event and yields
    /// tasks through an mpsc sender. Returns when `rx` is closed.
    pub async fn run(
        mut self,
        mut rx: mpsc::Receiver<WatcherEvent>,
        tasks_tx: mpsc::Sender<SpeculativeTask>,
    ) {
        while let Some(ev) = rx.recv().await {
            for task in self.handle_event(ev) {
                if tasks_tx.send(task).await.is_err() {
                    return;
                }
            }
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
