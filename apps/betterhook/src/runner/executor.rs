//! Hook execution orchestration.
//!
//! Both the sequential and parallel paths live here behind the single
//! `run_hook` entry point. Parallel scheduling is priority-aware
//! (directly fixing lefthook #846): jobs are lowered into priority
//! order, and the scheduler spawns them in that order against
//! a tokio Semaphore so higher-priority jobs always acquire their permit
//! first when there is contention.

use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;

use super::RunError;
use super::RunResult;
use super::dag::{JobGraph, build_dag};
use super::diagnostics::emit_builtin_diagnostics;
use super::output::{OutputEvent, SinkKind, sink};
use super::plan::{JobPlan, ResolvedJob, default_parallel_limit, resolve_job_plan};
use super::proc::{Cancel, run_command};
use super::stage::{
    GitIndexLock, acquire_if_isolated, acquire_repo_stash_lock, apply_stage_fixed,
    snapshot_unstaged_if_needed,
};
use crate::config::{Hook, Job};
use crate::git::StashGuard;

/// Summary of a hook run, returned to the CLI for exit-code mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    pub hook_name: String,
    pub ok: bool,
    pub jobs_run: usize,
    pub jobs_skipped: usize,
    pub duration_ms: u128,
}

/// Runtime filters applied to the hook's job list before execution.
#[derive(Debug, Default, Clone)]
pub struct RunOptions {
    /// Job names to skip (`BETTERHOOK_SKIP` + CLI `--skip`).
    pub skip: Vec<String>,
    /// If non-empty, run only these jobs (`BETTERHOOK_ONLY` + CLI `--only`).
    pub only: Vec<String>,
    /// Which output sink to use (TTY vs NDJSON).
    pub sink: SinkKind,
    /// Bypass the coordinator and file-lock entirely. Jobs declaring
    /// `isolate` run unlocked with a warning to stderr.
    pub no_locks: bool,
}

impl RunOptions {
    /// Read `BETTERHOOK_SKIP` and `BETTERHOOK_ONLY` comma-separated env
    /// vars. Empty vars are ignored.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            skip: parse_env_list("BETTERHOOK_SKIP"),
            only: parse_env_list("BETTERHOOK_ONLY"),
            sink: SinkKind::Tty,
            no_locks: std::env::var("BETTERHOOK_NO_LOCKS").is_ok(),
        }
    }

    fn is_filtered(&self, job_name: &str) -> bool {
        if !self.only.is_empty() && !self.only.iter().any(|n| n == job_name) {
            return true;
        }
        self.skip.iter().any(|n| n == job_name)
    }
}

fn parse_env_list(var: &str) -> Vec<String> {
    std::env::var(var)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Top-level entrypoint. Dispatches to the sequential or parallel
/// implementation based on `hook.parallel`. Reads skip/only filters
/// from the `BETTERHOOK_SKIP` and `BETTERHOOK_ONLY` env vars.
pub async fn run_hook(hook: &Hook, worktree: &Path) -> RunResult<ExecutionReport> {
    run_hook_with_options(hook, worktree, RunOptions::from_env()).await
}

/// Variant of [`run_hook`] that takes explicit [`RunOptions`] — used by
/// the CLI when `--skip`/`--only` flags override the env vars.
pub async fn run_hook_with_options(
    hook: &Hook,
    worktree: &Path,
    options: RunOptions,
) -> RunResult<ExecutionReport> {
    let (tx, writer) = sink(options.sink);
    let start = Instant::now();

    // Resolve the common dir once up-front — the lock client stores
    // advisory files under <common-dir>/betterhook/locks/.
    let common_dir = crate::git::git_common_dir(worktree).await?;
    let no_locks = options.no_locks;

    // Single per-hook lock covering every git index mutation (stash,
    // add, unstash). Parallel jobs share it.
    let git_lock: GitIndexLock = Arc::new(Mutex::new(()));
    // `git stash` mutates repo-global state shared by every linked
    // worktree, so hold a cross-worktree advisory lock for the full
    // push→run→pop lifecycle.
    let _stash_lock = if hook.stash_untracked {
        Some(acquire_repo_stash_lock(&common_dir).await?)
    } else {
        None
    };

    // Push an untracked+unstaged stash before the first job so formatters
    // don't see files that aren't about to be committed (lefthook #833).
    let stash = if hook.stash_untracked {
        let _guard = git_lock.lock().await;
        Some(StashGuard::push(worktree).await?)
    } else {
        None
    };

    let mut resolved: Vec<ResolvedJob> = Vec::with_capacity(hook.jobs.len());
    let mut filtered_out = 0usize;
    for job in &hook.jobs {
        if options.is_filtered(job.name.as_str()) {
            let _ = tx
                .send(OutputEvent::JobSkipped {
                    job: job.name.to_string(),
                    reason: "filtered by --skip/--only".to_owned(),
                })
                .await;
            filtered_out += 1;
            continue;
        }
        let plan = resolve_job_plan(hook, job, worktree).await?;
        resolved.push(ResolvedJob {
            job: job.clone(),
            plan,
        });
    }

    let ctx = ExecutionContext {
        hook,
        tx: &tx,
        git_lock: &git_lock,
        worktree,
        common_dir: &common_dir,
        no_locks,
    };

    let exec_res: RunResult<RunSummary> = if hook.parallel {
        run_parallel(&ctx, resolved).await
    } else {
        run_sequential(&ctx, resolved).await
    };

    // Always try to pop the stash, even on error. A stash-pop failure
    // is reported to stderr but does not override the primary error.
    let mut stash_restore_err = None;
    if let Some(guard) = stash {
        let _guard = git_lock.lock().await;
        if let Err(e) = guard.pop().await {
            eprintln!("betterhook: WARNING — failed to pop untracked stash: {e}");
            eprintln!(
                "betterhook: your stash is still in `git stash list`; run `git stash pop` manually."
            );
            stash_restore_err = Some(e);
        }
    }

    let report = exec_res?;
    if let Some(err) = stash_restore_err {
        return Err(err.into());
    }
    let jobs_skipped = report.jobs_skipped + filtered_out;

    let total = start.elapsed();
    let _ = tx
        .send(OutputEvent::Summary {
            ok: report.ok,
            jobs_run: report.jobs_run,
            jobs_skipped,
            total,
        })
        .await;
    drop(tx);
    let _ = writer.await;

    Ok(ExecutionReport {
        hook_name: hook.name.to_string(),
        ok: report.ok,
        jobs_run: report.jobs_run,
        jobs_skipped,
        duration_ms: total.as_millis(),
    })
}

#[derive(Debug, Clone, Copy)]
struct RunSummary {
    ok: bool,
    jobs_run: usize,
    jobs_skipped: usize,
}

/// Per-hook execution state shared across the sequential and parallel
/// schedulers.
struct ExecutionContext<'a> {
    hook: &'a Hook,
    tx: &'a mpsc::Sender<OutputEvent>,
    git_lock: &'a GitIndexLock,
    worktree: &'a Path,
    common_dir: &'a Path,
    no_locks: bool,
}

async fn run_sequential(
    ctx: &ExecutionContext<'_>,
    jobs: Vec<ResolvedJob>,
) -> RunResult<RunSummary> {
    let mut jobs_run = 0usize;
    let mut jobs_skipped = 0usize;
    let mut failed = false;

    for rj in jobs {
        let Some(plan) = rj.plan else {
            let _ = ctx
                .tx
                .send(OutputEvent::JobSkipped {
                    job: rj.job.name.to_string(),
                    reason: "no files matched glob".to_owned(),
                })
                .await;
            jobs_skipped += 1;
            continue;
        };

        let before_unstaged = snapshot_unstaged_if_needed(&rj.job, ctx.worktree).await?;

        let (_guard, lock_env) =
            acquire_if_isolated(&rj.job, ctx.common_dir, ctx.worktree, ctx.no_locks, ctx.tx).await;
        let mut extra_env = vec![("BETTERHOOK_HOOK".to_owned(), ctx.hook.name.to_string())];
        extra_env.extend(lock_env);
        let mut job_failed = false;
        for cmd in &plan.commands {
            let exit = run_command(
                rj.job.name.as_str(),
                cmd,
                &plan.cwd,
                &rj.job.env,
                &extra_env,
                rj.job.timeout,
                None,
                ctx.tx,
            )
            .await?;
            if exit != 0 {
                failed = true;
                job_failed = true;
                if ctx.hook.fail_fast {
                    break;
                }
            }
        }

        if let Some(before) = before_unstaged {
            apply_stage_fixed(ctx.worktree, &before, ctx.git_lock).await?;
        }

        jobs_run += 1;

        if job_failed && ctx.hook.fail_fast {
            break;
        }
    }

    Ok(RunSummary {
        ok: !failed,
        jobs_run,
        jobs_skipped,
    })
}

/// Parallel executor driven by the capability DAG. Jobs whose
/// capability sets are disjoint run in parallel; conflicting jobs
/// serialize in priority order.
// The scheduler loop is one cohesive state machine: ready heap,
// join-set drain, DAG child release, fail-fast cascade. Splitting
// further would spread mutable local state across functions and hurt
// readability more than it helps.
#[allow(clippy::too_many_lines)]
async fn run_parallel(ctx: &ExecutionContext<'_>, jobs: Vec<ResolvedJob>) -> RunResult<RunSummary> {
    let limit = ctx
        .hook
        .parallel_limit
        .unwrap_or_else(default_parallel_limit)
        .max(1);
    let hook_name = ctx.hook.name.to_string();
    let fail_fast = ctx.hook.fail_fast;

    // Align plans with the DAG's node indices by reusing the same job
    // ordering (`jobs` came from `hook.jobs`).
    let job_list: Vec<Job> = jobs.iter().map(|rj| rj.job.clone()).collect();
    let graph: JobGraph = build_dag(&job_list).map_err(|source| RunError::Dag { source })?;
    let plans: Vec<Option<JobPlan>> = jobs.into_iter().map(|rj| rj.plan).collect();

    // Pending-parent counts per node — a node is ready when this hits 0.
    let mut pending: Vec<usize> = graph.nodes.iter().map(|n| n.parents.len()).collect();
    let mut started = vec![false; graph.nodes.len()];

    // Priority-ordered ready heap. BinaryHeap is a max-heap, so we
    // reverse the key to get lowest-priority-first (lowest value =
    // earliest in `hook.priority`).
    let mut ready: BinaryHeap<std::cmp::Reverse<(u32, usize)>> = BinaryHeap::new();
    for (idx, node) in graph.nodes.iter().enumerate() {
        if pending[idx] == 0 {
            ready.push(std::cmp::Reverse((node.job.priority, idx)));
        }
    }

    let cancel = Cancel::new();
    let mut set: JoinSet<(usize, Result<JobOutcome, RunError>)> = JoinSet::new();
    let mut running = 0usize;
    let mut failed = false;
    let mut jobs_run = 0usize;
    let mut jobs_skipped = 0usize;

    loop {
        // Drain the ready heap into spawns or synchronous skips,
        // bounded by `limit`.
        while running < limit {
            let Some(std::cmp::Reverse((_, idx))) = ready.pop() else {
                break;
            };
            if started[idx] {
                continue;
            }
            started[idx] = true;

            let plan_opt = plans[idx].clone();
            let job = job_list[idx].clone();

            // Missing plan → the job was skipped at resolve time
            // (template with no matching files). Transition it as if
            // it had finished instantly so children can become ready.
            let Some(plan) = plan_opt else {
                let _ = ctx
                    .tx
                    .send(OutputEvent::JobSkipped {
                        job: job.name.to_string(),
                        reason: "no files matched glob".to_owned(),
                    })
                    .await;
                jobs_skipped += 1;
                release_children(&graph, idx, &mut pending, &started, &mut ready);
                continue;
            };

            // Only `concurrent_safe` jobs are cacheable. Jobs with
            // side effects must run again so the behavior is real, not
            // replayed from prior output.
            if job.concurrent_safe {
                if let Ok(Some(cached)) =
                    crate::cache::lookup(ctx.common_dir, &job, &plan.files).await
                {
                    let _ = ctx
                        .tx
                        .send(OutputEvent::JobCacheHit {
                            job: job.name.to_string(),
                            files: plan.files.len(),
                        })
                        .await;
                    for event in cached.events {
                        let _ = ctx.tx.send(event).await;
                    }
                    if cached.exit != 0 {
                        failed = true;
                        if fail_fast {
                            cancel.cancel();
                        }
                    }
                    jobs_run += 1;
                    release_children(&graph, idx, &mut pending, &started, &mut ready);
                    continue;
                }
            }

            // Spawn the real job as a named async fn. Naming the body
            // improves stack traces, shortens the outer scheduler, and
            // removes four levels of closure nesting.
            let spawn_ctx = SpawnedJobContext {
                job,
                plan: plan.clone(),
                tx: ctx.tx.clone(),
                hook_name: hook_name.clone(),
                git_lock: ctx.git_lock.clone(),
                worktree: ctx.worktree.to_path_buf(),
                common_dir: ctx.common_dir.to_path_buf(),
                no_locks: ctx.no_locks,
                cancel: cancel.clone(),
            };
            running += 1;
            set.spawn(async move {
                let result = execute_job_in_dag(spawn_ctx).await;
                (idx, result)
            });
        }

        if set.is_empty() {
            // Nothing in flight and nothing ready means every node
            // has been processed. A true deadlock cannot happen here
            // because the DAG is acyclic by construction.
            break;
        }

        let join_res = set.join_next().await.expect("set non-empty");
        let (idx, outcome_res) = join_res.expect("joinset task panicked");
        let outcome = outcome_res?;
        running -= 1;
        jobs_run += 1;

        if outcome.failed {
            failed = true;
            if fail_fast {
                cancel.cancel();
                while set.join_next().await.is_some() {}
                break;
            }
        }

        // Release children of the finished node.
        for &child in &graph.nodes[idx].children {
            release_child(&graph, child, &mut pending, &started, &mut ready);
        }
    }

    Ok(RunSummary {
        ok: !failed,
        jobs_run,
        jobs_skipped,
    })
}

/// Private helper that releases children's pending counters when a
/// skipped node transitions synchronously. Extracted into a function
/// just to keep `run_parallel` readable.
fn release_children(
    graph: &JobGraph,
    idx: usize,
    pending: &mut [usize],
    started: &[bool],
    ready: &mut BinaryHeap<std::cmp::Reverse<(u32, usize)>>,
) {
    for &child in &graph.nodes[idx].children {
        release_child(graph, child, pending, started, ready);
    }
}

fn release_child(
    graph: &JobGraph,
    child: usize,
    pending: &mut [usize],
    started: &[bool],
    ready: &mut BinaryHeap<std::cmp::Reverse<(u32, usize)>>,
) {
    debug_assert!(
        pending[child] > 0,
        "child pending count must stay positive until release"
    );
    if pending[child] == 0 {
        return;
    }
    pending[child] -= 1;
    if pending[child] != 0 {
        return;
    }
    debug_assert!(
        !started[child],
        "child should not already be started when it becomes ready"
    );
    if started[child] {
        return;
    }
    let pri = graph.nodes[child].job.priority;
    ready.push(std::cmp::Reverse((pri, child)));
}

struct JobOutcome {
    failed: bool,
}

/// Owned context handed to [`execute_job_in_dag`] when the scheduler
/// spawns a DAG node. Every field is owned because the closure runs
/// on a detached tokio task and outlives the calling scheduler loop.
struct SpawnedJobContext {
    job: Job,
    plan: JobPlan,
    tx: mpsc::Sender<OutputEvent>,
    hook_name: String,
    git_lock: GitIndexLock,
    worktree: PathBuf,
    common_dir: PathBuf,
    no_locks: bool,
    cancel: Cancel,
}

/// Run a single DAG node to completion: acquire isolation lock, spawn
/// the tee channel for cache capture, run every command, apply
/// `stage_fixed`, and persist the cache entry on success.
async fn execute_job_in_dag(ctx: SpawnedJobContext) -> Result<JobOutcome, RunError> {
    let SpawnedJobContext {
        job,
        plan,
        tx,
        hook_name,
        git_lock,
        worktree,
        common_dir,
        no_locks,
        cancel,
    } = ctx;

    let before_unstaged = snapshot_unstaged_if_needed(&job, &worktree).await?;
    let (_lock, lock_env) = acquire_if_isolated(&job, &common_dir, &worktree, no_locks, &tx).await;
    let mut extra_env = vec![("BETTERHOOK_HOOK".to_owned(), hook_name)];
    extra_env.extend(lock_env);

    // Capture events as they fire so we can persist them into the CA
    // cache on success. The tee task forwards each event to `tx`
    // unchanged and also pushes into a local Vec we drain later.
    let (local_tx, mut local_rx) = mpsc::channel::<OutputEvent>(256);
    let tx_forward = tx.clone();
    let tee = tokio::spawn(async move {
        let mut captured: Vec<OutputEvent> = Vec::new();
        while let Some(ev) = local_rx.recv().await {
            captured.push(ev.clone());
            let _ = tx_forward.send(ev).await;
        }
        captured
    });

    let mut job_failed = false;
    for cmd in &plan.commands {
        let exit = run_command(
            job.name.as_str(),
            cmd,
            &plan.cwd,
            &job.env,
            &extra_env,
            job.timeout,
            Some(&cancel),
            &local_tx,
        )
        .await?;
        if exit != 0 {
            job_failed = true;
            break;
        }
    }
    drop(local_tx);
    let captured = tee.await.unwrap_or_default();

    // If this job references a builtin, parse the captured stdout lines
    // through the builtin's parser and emit Diagnostic events. This is
    // what makes `--json` output structured for agents — the raw line
    // stream is augmented with typed file/line/severity diagnostics.
    if let Some(ref builtin_id) = job.builtin {
        emit_builtin_diagnostics(builtin_id, job.name.as_str(), &captured, &tx).await;
    }

    if let Some(before) = before_unstaged {
        apply_stage_fixed(&worktree, &before, &git_lock).await?;
    }

    // Cache the events on a clean run of a concurrent_safe job.
    // Best-effort: cache write failures log but don't fail the hook.
    if job.concurrent_safe && !job_failed && !plan.files.is_empty() {
        let inputs = crate::cache::snapshot_inputs(&plan.files);
        let result = crate::cache::CachedResult {
            exit: 0,
            events: captured,
            created_at: std::time::SystemTime::now(),
            inputs,
        };
        if let Err(e) = crate::cache::store_result(&common_dir, &job, &plan.files, &result).await {
            eprintln!(
                "betterhook: WARNING — cache write for '{}' failed: {e}",
                job.name
            );
        }
    }

    Ok(JobOutcome { failed: job_failed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IsolateSpec;
    use crate::config::ToolPathScope;
    use crate::git::GitError;
    use crate::runner::output::Stream;
    use crate::test_support::new_git_repo_with_file;
    use std::collections::BTreeMap;
    use std::process::Command as StdCommand;

    fn new_git_repo() -> (tempfile::TempDir, PathBuf) {
        new_git_repo_with_file("a.ts", "1\n")
    }

    fn stub_job(name: &str, run: &str) -> Job {
        Job {
            name: name.into(),
            run: run.to_owned(),
            fix: None,
            glob: Vec::new(),
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
            concurrent_safe: false,
            builtin: None,
        }
    }

    fn stub_hook(name: &str, jobs: Vec<Job>) -> Hook {
        Hook {
            name: name.into(),
            parallel: false,
            parallel_explicit: false,
            fail_fast: false,
            fail_fast_explicit: false,
            parallel_limit: None,
            stash_untracked: false,
            stash_untracked_explicit: false,
            jobs,
        }
    }

    #[tokio::test]
    async fn run_hook_succeeds_when_every_job_exits_zero() {
        let (_d, root) = new_git_repo_with_file("a.ts", "1\n");
        let hook = stub_hook(
            "pre-commit",
            vec![
                stub_job("echo1", "echo hello"),
                stub_job("echo2", "echo world"),
            ],
        );
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);
        assert_eq!(rep.jobs_run, 2);
        assert_eq!(rep.jobs_skipped, 0);
    }

    #[test]
    fn default_parallel_limit_matches_runtime_parallelism() {
        let expected = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
        assert_eq!(default_parallel_limit(), expected);
        assert!(default_parallel_limit() > 0);
    }

    #[test]
    fn parse_env_list_trims_and_discards_empty_entries() {
        unsafe {
            std::env::set_var("BETTERHOOK_TEST_LIST", " lint , ,test,, fmt ");
        }
        let values = parse_env_list("BETTERHOOK_TEST_LIST");
        unsafe {
            std::env::remove_var("BETTERHOOK_TEST_LIST");
        }
        assert_eq!(values, vec!["lint", "test", "fmt"]);
    }

    #[test]
    fn run_options_from_env_reads_skip_only_and_no_locks() {
        unsafe {
            std::env::set_var("BETTERHOOK_SKIP", "lint, test");
            std::env::set_var("BETTERHOOK_ONLY", "fmt");
            std::env::set_var("BETTERHOOK_NO_LOCKS", "1");
        }
        let options = RunOptions::from_env();
        unsafe {
            std::env::remove_var("BETTERHOOK_SKIP");
            std::env::remove_var("BETTERHOOK_ONLY");
            std::env::remove_var("BETTERHOOK_NO_LOCKS");
        }

        assert_eq!(options.skip, vec!["lint", "test"]);
        assert_eq!(options.only, vec!["fmt"]);
        assert!(matches!(options.sink, SinkKind::Tty));
        assert!(options.no_locks);
    }

    #[test]
    fn run_options_filtering_honors_only_then_skip() {
        let options = RunOptions {
            skip: vec!["fmt".to_owned()],
            only: vec!["lint".to_owned(), "fmt".to_owned()],
            sink: SinkKind::Tty,
            no_locks: false,
        };

        assert!(!options.is_filtered("lint"));
        assert!(options.is_filtered("fmt"));
        assert!(options.is_filtered("test"));
    }

    #[tokio::test]
    async fn run_hook_reports_failure_on_nonzero_exit() {
        let (_d, root) = new_git_repo();
        let hook = stub_hook(
            "pre-commit",
            vec![stub_job("pass", "true"), stub_job("fail", "exit 1")],
        );
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(!rep.ok);
        assert_eq!(rep.jobs_run, 2, "both jobs run when fail_fast is off");
    }

    #[tokio::test]
    async fn fail_fast_stops_after_first_failure() {
        let (_d, root) = new_git_repo();
        let mut hook = stub_hook(
            "pre-commit",
            vec![
                stub_job("fail", "exit 1"),
                stub_job("never", "echo should-not-run"),
            ],
        );
        hook.fail_fast = true;
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(!rep.ok);
        assert_eq!(
            rep.jobs_run, 1,
            "fail_fast counts the failed job before bailing"
        );
    }

    #[tokio::test]
    async fn template_with_no_staged_files_is_skipped() {
        let (_d, root) = new_git_repo();
        let mut job = stub_job("fmt", "prettier --write {staged_files}");
        job.glob = vec!["*.ts".to_owned()];
        let hook = stub_hook("pre-commit", vec![job]);
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);
        assert_eq!(rep.jobs_skipped, 1);
        assert_eq!(rep.jobs_run, 0);
    }

    #[tokio::test]
    async fn snapshot_unstaged_skips_when_stage_fixed_is_disabled() {
        let (_d, root) = new_git_repo();
        let job = stub_job("fmt", "prettier --write a.ts");

        let before = snapshot_unstaged_if_needed(&job, &root).await.unwrap();

        assert!(before.is_none());
    }

    #[tokio::test]
    async fn snapshot_unstaged_skips_interactive_jobs() {
        let (_d, root) = new_git_repo();
        let mut job = stub_job("fmt", "prettier --write a.ts");
        job.stage_fixed = true;
        job.interactive = true;

        let before = snapshot_unstaged_if_needed(&job, &root).await.unwrap();

        assert!(before.is_none());
    }

    #[tokio::test]
    async fn resolve_job_plan_non_template_ignores_staged_files() {
        let (_d, root) = new_git_repo();
        std::fs::write(root.join("a.ts"), "2\n").unwrap();
        let status = StdCommand::new("git")
            .current_dir(&root)
            .args(["add", "a.ts"])
            .status()
            .unwrap();
        assert!(status.success());

        let hook = stub_hook("pre-commit", Vec::new());
        let job = stub_job("lint", "eslint .");

        let plan = resolve_job_plan(&hook, &job, &root).await.unwrap().unwrap();

        assert_eq!(plan.commands, vec!["eslint .".to_owned()]);
        assert_eq!(plan.cwd, root);
        assert!(
            plan.files.is_empty(),
            "non-template jobs should not snapshot staged files"
        );
    }

    #[tokio::test]
    async fn resolve_job_plan_pre_push_uses_push_diff_without_template() {
        let (_d, root) = new_git_repo();
        std::fs::write(root.join("a.ts"), "2\n").unwrap();
        let git = |args: &[&str]| {
            let status = StdCommand::new("git")
                .current_dir(&root)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t.t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t.t")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["add", "a.ts"]);
        git(&["commit", "-q", "-m", "second"]);

        let mut job = stub_job("lint", "eslint .");
        job.glob = vec!["*.ts".to_owned()];
        let hook = stub_hook("pre-push", Vec::new());

        let plan = resolve_job_plan(&hook, &job, &root).await.unwrap().unwrap();

        assert_eq!(plan.commands, vec!["eslint .".to_owned()]);
        assert_eq!(plan.cwd, root);
        assert_eq!(plan.files, vec![root.join("a.ts")]);
    }

    #[tokio::test]
    async fn parallel_hook_runs_all_jobs_concurrently() {
        // Prove parallelism structurally: each job writes to a shared
        // counter file while sleeping. If the jobs ran serially, the
        // max counter value would be 1; if parallel, it would hit 4.
        // This is deterministic regardless of machine load — no
        // wall-clock thresholds.
        let (_d, root) = new_git_repo();
        let counter_file = root.join("bh-parallel-counter");
        std::fs::write(&counter_file, "0").unwrap();
        let cf = counter_file.display();
        let mut hook = stub_hook(
            "pre-commit",
            (0..4)
                .map(|i| {
                    // Each job: increment, sleep briefly, then decrement.
                    // A concurrent run will see the counter above 1.
                    stub_job(
                        &format!("j{i}"),
                        &format!(
                            "v=$(cat {cf}); echo $((v+1)) > {cf}; sleep 0.1; v=$(cat {cf}); echo $((v-1)) > {cf}; true"
                        ),
                    )
                })
                .collect(),
        );
        hook.parallel = true;
        hook.parallel_limit = Some(4);
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);
        assert_eq!(rep.jobs_run, 4);
    }

    #[tokio::test]
    async fn parallel_limit_caps_max_in_flight_jobs() {
        let (_d, root) = new_git_repo();
        let current = root.join("bh-current");
        let max_seen = root.join("bh-max");
        let lock_dir = root.join("bh-counter-lock");
        std::fs::write(&current, "0\n").unwrap();
        std::fs::write(&max_seen, "0\n").unwrap();

        let make_job = |name: &str| {
            stub_job(
                name,
                &format!(
                    "while ! mkdir {lock} 2>/dev/null; do sleep 0.01; done; \
                     v=$(cat {current}); v=$((v+1)); echo $v > {current}; \
                     m=$(cat {max_seen}); if [ \"$v\" -gt \"$m\" ]; then echo $v > {max_seen}; fi; \
                     rmdir {lock}; \
                     sleep 0.15; \
                     while ! mkdir {lock} 2>/dev/null; do sleep 0.01; done; \
                     v=$(cat {current}); echo $((v-1)) > {current}; rmdir {lock}",
                    lock = lock_dir.display(),
                    current = current.display(),
                    max_seen = max_seen.display(),
                ),
            )
        };

        let mut hook = stub_hook(
            "pre-commit",
            vec![
                make_job("one"),
                make_job("two"),
                make_job("three"),
                make_job("four"),
            ],
        );
        hook.parallel = true;
        hook.parallel_limit = Some(2);

        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);
        assert_eq!(rep.jobs_run, 4);
        assert_eq!(std::fs::read_to_string(&max_seen).unwrap().trim(), "2");
    }

    #[tokio::test]
    async fn parallel_fail_fast_aborts_remaining_jobs() {
        // Prove abort structurally: the slow job writes a sentinel
        // file after 2s; if fail_fast works, the sentinel should NOT
        // exist after the run completes because the job was killed
        // before it got there.
        let (_d, root) = new_git_repo();
        let sentinel = root.join("bh-slow-reached");
        let sp = sentinel.display();
        let mut hook = stub_hook(
            "pre-commit",
            vec![
                stub_job("fail", "exit 1"),
                stub_job("slow", &format!("sleep 2 && touch {sp}")),
            ],
        );
        hook.parallel = true;
        hook.parallel_limit = Some(2);
        hook.fail_fast = true;
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(!rep.ok);
        assert!(
            !sentinel.exists(),
            "fail_fast should have aborted the slow job before it wrote the sentinel"
        );
    }

    #[tokio::test]
    async fn per_job_timeout_kills_child_and_reports_124() {
        // Prove timeout structurally: the job writes a sentinel after
        // 2s; the timeout is 200ms, so the sentinel should never appear.
        let (_d, root) = new_git_repo();
        let sentinel = root.join("bh-timeout-reached");
        let sp = sentinel.display();
        let mut job = stub_job("slow", &format!("sleep 2 && touch {sp}"));
        job.timeout = Some(std::time::Duration::from_millis(200));
        let hook = stub_hook("pre-commit", vec![job]);
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(!rep.ok, "timed-out job reports failure");
        assert!(
            !sentinel.exists(),
            "timeout should have killed the job before the sentinel was written"
        );
    }

    #[tokio::test]
    async fn run_options_skip_filters_out_job() {
        let (_d, root) = new_git_repo();
        let hook = stub_hook(
            "pre-commit",
            vec![
                stub_job("should-run", "true"),
                stub_job("should-skip", "exit 1"),
            ],
        );
        let opts = RunOptions {
            skip: vec!["should-skip".to_owned()],
            only: Vec::new(),
            sink: SinkKind::Tty,
            no_locks: false,
        };
        let rep = run_hook_with_options(&hook, &root, opts).await.unwrap();
        assert!(rep.ok, "should-skip was filtered out, hook should pass");
        assert_eq!(rep.jobs_run, 1);
        assert_eq!(rep.jobs_skipped, 1);
    }

    #[tokio::test]
    async fn run_options_only_runs_named_job() {
        let (_d, root) = new_git_repo();
        let hook = stub_hook(
            "pre-commit",
            vec![
                stub_job("lint", "true"),
                stub_job("test", "exit 1"),
                stub_job("fmt", "true"),
            ],
        );
        let opts = RunOptions {
            skip: Vec::new(),
            only: vec!["lint".to_owned()],
            sink: SinkKind::Tty,
            no_locks: false,
        };
        let rep = run_hook_with_options(&hook, &root, opts).await.unwrap();
        assert!(rep.ok);
        assert_eq!(rep.jobs_run, 1);
        assert_eq!(rep.jobs_skipped, 2);
    }

    #[tokio::test]
    async fn parallel_skip_is_counted_when_template_matches_no_files() {
        let (_d, root) = new_git_repo();

        let mut skipped = stub_job("fmt", "prettier --write {staged_files}");
        skipped.glob = vec!["*.py".to_owned()];
        let ran = stub_job("lint", "true");

        let mut hook = stub_hook("pre-commit", vec![skipped, ran]);
        hook.parallel = true;
        hook.parallel_limit = Some(2);

        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);
        assert_eq!(rep.jobs_run, 1);
        assert_eq!(rep.jobs_skipped, 1);
    }

    #[tokio::test]
    async fn parallel_cached_failure_fails_hook_and_counts_as_run() {
        let (_d, root) = new_git_repo();
        std::fs::write(root.join("a.ts"), "cached\n").unwrap();
        let status = StdCommand::new("git")
            .current_dir(&root)
            .args(["add", "a.ts"])
            .status()
            .unwrap();
        assert!(status.success());

        let common = crate::git::git_common_dir(&root).await.unwrap();
        let mut job = stub_job("lint", "eslint {staged_files}");
        job.glob = vec!["*.ts".to_owned()];
        job.concurrent_safe = true;
        let files = vec![root.join("a.ts")];
        let cached = crate::cache::CachedResult {
            exit: 1,
            events: Vec::new(),
            created_at: std::time::SystemTime::now(),
            inputs: crate::cache::snapshot_inputs(&files),
        };
        crate::cache::store_result(&common, &job, &files, &cached)
            .await
            .unwrap();

        let mut hook = stub_hook("pre-commit", vec![job]);
        hook.parallel = true;
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(!rep.ok, "cached nonzero exit should fail the hook");
        assert_eq!(rep.jobs_run, 1);
        assert_eq!(rep.jobs_skipped, 0);
    }

    #[tokio::test]
    async fn successful_non_concurrent_safe_job_does_not_write_cache() {
        let (_d, root) = new_git_repo();
        std::fs::write(root.join("a.ts"), "uncached\n").unwrap();
        let status = StdCommand::new("git")
            .current_dir(&root)
            .args(["add", "a.ts"])
            .status()
            .unwrap();
        assert!(status.success());

        let common = crate::git::git_common_dir(&root).await.unwrap();
        let mut job = stub_job("lint", "true");
        job.glob = vec!["*.ts".to_owned()];
        let files = vec![root.join("a.ts")];

        let mut hook = stub_hook("pre-commit", vec![job.clone()]);
        hook.parallel = true;
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);
        assert!(
            crate::cache::lookup(&common, &job, &files)
                .await
                .unwrap()
                .is_none(),
            "non-concurrent-safe jobs must not populate the cache"
        );
    }

    #[tokio::test]
    async fn failed_concurrent_safe_job_does_not_write_cache() {
        let (_d, root) = new_git_repo();
        std::fs::write(root.join("a.ts"), "failed-cache\n").unwrap();
        let status = StdCommand::new("git")
            .current_dir(&root)
            .args(["add", "a.ts"])
            .status()
            .unwrap();
        assert!(status.success());

        let common = crate::git::git_common_dir(&root).await.unwrap();
        let mut job = stub_job("lint", "false");
        job.glob = vec!["*.ts".to_owned()];
        job.concurrent_safe = true;
        let files = vec![root.join("a.ts")];

        let mut hook = stub_hook("pre-commit", vec![job.clone()]);
        hook.parallel = true;
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(!rep.ok);
        assert!(
            crate::cache::lookup(&common, &job, &files)
                .await
                .unwrap()
                .is_none(),
            "failed jobs must not populate the cache"
        );
    }

    #[tokio::test]
    async fn successful_concurrent_safe_job_writes_cache() {
        let (_d, root) = new_git_repo();
        std::fs::write(root.join("a.ts"), "cache-me\n").unwrap();
        let status = StdCommand::new("git")
            .current_dir(&root)
            .args(["add", "a.ts"])
            .status()
            .unwrap();
        assert!(status.success());

        let common = crate::git::git_common_dir(&root).await.unwrap();
        let mut job = stub_job("lint", "printf 'ok\\n'");
        job.glob = vec!["*.ts".to_owned()];
        job.concurrent_safe = true;
        let files = vec![root.join("a.ts")];

        let mut hook = stub_hook("pre-commit", vec![job.clone()]);
        hook.parallel = true;
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);
        assert!(
            crate::cache::lookup(&common, &job, &files)
                .await
                .unwrap()
                .is_some(),
            "successful concurrent-safe jobs should populate the cache"
        );
    }

    #[tokio::test]
    async fn parallel_parent_completion_releases_blocked_child() {
        let (_d, root) = new_git_repo();
        let marker = root.join("parent-done");

        let mut parent = stub_job("parent", &format!("printf done > {}", marker.display()));
        parent.writes = vec!["shared".to_owned()];
        let mut child = stub_job("child", &format!("[ -f {} ]", marker.display()));
        child.reads = vec!["shared".to_owned()];

        let mut hook = stub_hook("pre-commit", vec![parent, child]);
        hook.parallel = true;
        hook.parallel_limit = Some(2);

        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);
        assert_eq!(rep.jobs_run, 2);
        assert_eq!(rep.jobs_skipped, 0);
        assert!(marker.exists(), "parent must have run before child");
    }

    #[tokio::test]
    async fn skipped_parallel_parent_releases_blocked_child() {
        let (_d, root) = new_git_repo();
        let marker = root.join("child-after-skip");

        let mut parent = stub_job("parent", "prettier --write {staged_files}");
        parent.glob = vec!["*.py".to_owned()];
        parent.writes = vec!["shared".to_owned()];

        let mut child = stub_job("child", &format!("printf ready > {}", marker.display()));
        child.reads = vec!["shared".to_owned()];

        let mut hook = stub_hook("pre-commit", vec![parent, child]);
        hook.parallel = true;
        hook.parallel_limit = Some(2);

        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);
        assert_eq!(rep.jobs_run, 1);
        assert_eq!(rep.jobs_skipped, 1);
        assert!(
            marker.exists(),
            "child must run after skipped parent releases it"
        );
    }

    #[tokio::test]
    async fn emit_builtin_diagnostics_emits_rustfmt_findings() {
        let (tx, mut rx) = mpsc::channel(8);
        let captured = vec![OutputEvent::Line {
            job: "fmt".to_owned(),
            stream: Stream::Stdout,
            line: "Diff in /repo/src/main.rs at line 12:".to_owned(),
        }];

        emit_builtin_diagnostics("rustfmt", "fmt", &captured, &tx).await;
        drop(tx);

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        assert!(events.iter().any(|event| matches!(
            event,
            OutputEvent::Diagnostic { file, line, .. }
            if file == "/repo/src/main.rs" && *line == Some(12)
        )));
    }

    #[tokio::test]
    async fn emit_builtin_diagnostics_emits_clippy_findings() {
        let (tx, mut rx) = mpsc::channel(8);
        let captured = vec![OutputEvent::Line {
            job: "lint".to_owned(),
            stream: Stream::Stdout,
            line: r#"{"reason":"compiler-message","message":{"level":"warning","message":"unused variable: `x`","code":{"code":"unused_variables"},"spans":[{"file_name":"src/main.rs","line_start":3,"column_start":9,"is_primary":true}]}}"#.to_owned(),
        }];

        emit_builtin_diagnostics("clippy", "lint", &captured, &tx).await;
        drop(tx);

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        assert!(events.iter().any(|event| matches!(
            event,
            OutputEvent::Diagnostic { file, line, rule, .. }
            if file == "src/main.rs" && *line == Some(3) && rule.as_deref() == Some("unused_variables")
        )));
    }

    #[tokio::test]
    async fn emit_builtin_diagnostics_emits_prettier_findings() {
        let (tx, mut rx) = mpsc::channel(8);
        let captured = vec![OutputEvent::Line {
            job: "fmt".to_owned(),
            stream: Stream::Stdout,
            line: "Checking formatting...\n[warn] src/main.ts\n[warn] src/Button.tsx\n[warn] Code style issues found in 2 files.".to_owned(),
        }];

        emit_builtin_diagnostics("prettier", "fmt", &captured, &tx).await;
        drop(tx);

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let files: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                OutputEvent::Diagnostic { file, .. } => Some(file.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(files, vec!["src/main.ts", "src/Button.tsx"]);
    }

    #[tokio::test]
    async fn emit_builtin_diagnostics_emits_remaining_builtin_findings() {
        let cases = [
            (
                "eslint",
                r#"[{"filePath":"/a.ts","messages":[{"ruleId":"no-unused-vars","severity":2,"message":"x unused","line":3,"column":7}]}]"#,
                "/a.ts",
            ),
            (
                "ruff",
                r#"[{"code":"F401","message":"unused import","filename":"/a.py","location":{"row":3,"column":1}}]"#,
                "/a.py",
            ),
            (
                "black",
                "would reformat src/main.py\nwould reformat src/cli.py\nOh no! 2 files.\n",
                "src/main.py",
            ),
            ("gofmt", "cmd/main.go\ninternal/foo.go\n", "cmd/main.go"),
            (
                "govet",
                "./cmd/main.go:12:4: printf call has possible formatting\n",
                "./cmd/main.go",
            ),
            (
                "biome",
                r#"{"diagnostics":[{"category":"lint/suspicious/noDoubleEquals","severity":"error","description":"Use ===","location":{"path":{"file":"src/main.ts"}}}]}"#,
                "src/main.ts",
            ),
            (
                "oxlint",
                r#"[{"filePath":"/a.ts","messages":[{"ruleId":"no-unused-vars","severity":2,"message":"unused","line":1,"column":1}]}]"#,
                "/a.ts",
            ),
            (
                "shellcheck",
                r#"[{"file":"a.sh","line":14,"column":5,"level":"warning","code":2086,"message":"Double quote to prevent globbing."}]"#,
                "a.sh",
            ),
            (
                "gitleaks",
                r#"[{"RuleID":"aws-access-key","Description":"AWS Access Key","File":"deploy/secrets.env","StartLine":4,"StartColumn":9}]"#,
                "deploy/secrets.env",
            ),
        ];

        for (builtin_id, line, expected_file) in cases {
            let (tx, mut rx) = mpsc::channel(8);
            let captured = vec![OutputEvent::Line {
                job: "lint".to_owned(),
                stream: Stream::Stdout,
                line: line.to_owned(),
            }];

            emit_builtin_diagnostics(builtin_id, "lint", &captured, &tx).await;
            drop(tx);

            let mut files = Vec::new();
            while let Some(event) = rx.recv().await {
                if let OutputEvent::Diagnostic { file, .. } = event {
                    files.push(file);
                }
            }
            assert!(
                files.iter().any(|file| file == expected_file),
                "expected diagnostic for builtin {builtin_id} to include {expected_file}, got {files:?}"
            );
        }
    }

    #[tokio::test]
    async fn acquire_if_isolated_returns_guard_and_extra_env() {
        let (_d, root) = new_git_repo();
        let common = crate::git::git_common_dir(&root).await.unwrap();
        let (tx, mut rx) = mpsc::channel(8);
        let mut job = stub_job("cargo-build", "true");
        job.isolate = Some(IsolateSpec::ToolPath {
            tool: "cargo".to_owned(),
            target_dir: ToolPathScope::PerWorktree,
        });

        let (guard, extra_env) = acquire_if_isolated(&job, &common, &root, false, &tx).await;
        drop(tx);

        assert!(guard.is_some(), "expected isolate lock guard");
        assert_eq!(
            extra_env,
            vec![(
                "CARGO_TARGET_DIR".to_owned(),
                root.join("target").display().to_string()
            )]
        );
        assert!(
            rx.recv().await.is_none(),
            "successful lock should not emit skip events"
        );
    }

    #[tokio::test]
    async fn acquire_if_isolated_respects_no_locks_flag() {
        let (_d, root) = new_git_repo();
        let common = crate::git::git_common_dir(&root).await.unwrap();
        let (tx, mut rx) = mpsc::channel(8);
        let mut job = stub_job("cargo-build", "true");
        job.isolate = Some(IsolateSpec::Tool {
            name: "cargo".to_owned(),
        });

        let (guard, extra_env) = acquire_if_isolated(&job, &common, &root, true, &tx).await;
        drop(tx);

        assert!(guard.is_none());
        assert!(extra_env.is_empty());
        let event = rx.recv().await.expect("expected no-locks warning event");
        assert!(matches!(
            event,
            OutputEvent::JobSkipped { job, reason }
            if job == "cargo-build" && reason.contains("running unlocked")
        ));
    }

    #[tokio::test]
    async fn stage_fixed_re_stages_job_output() {
        let (_d, root) = new_git_repo();
        // Stage a.ts so it's part of the upcoming "commit".
        std::fs::write(root.join("a.ts"), "before\n").unwrap();
        let s = StdCommand::new("git")
            .current_dir(&root)
            .args(["add", "a.ts"])
            .status()
            .unwrap();
        assert!(s.success());

        // Job simulates a formatter: rewrites a.ts in place.
        let mut fmt_job = stub_job("fmt", "printf 'after\\n' > a.ts");
        fmt_job.stage_fixed = true;
        let hook = stub_hook("pre-commit", vec![fmt_job]);
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);

        // After stage_fixed, `git diff --name-only --cached` should show
        // a.ts (it's been re-staged with the formatter's output), and
        // the unstaged diff should be empty.
        let staged = StdCommand::new("git")
            .current_dir(&root)
            .args(["diff", "--name-only", "--cached"])
            .output()
            .unwrap();
        let staged_text = String::from_utf8(staged.stdout).unwrap();
        assert!(staged_text.contains("a.ts"));

        let unstaged = StdCommand::new("git")
            .current_dir(&root)
            .args(["diff", "--name-only"])
            .output()
            .unwrap();
        assert!(unstaged.stdout.is_empty(), "no unstaged remnants");
    }

    #[tokio::test]
    async fn parallel_limit_one_preserves_priority_order() {
        let (_d, root) = new_git_repo();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("order.log");
        let make = |name: &str, idx: usize| {
            let mut j = stub_job(name, &format!("printf '{idx}\\n' >> {}", marker.display()));
            j.priority = u32::try_from(idx).unwrap();
            j
        };
        let mut hook = stub_hook(
            "pre-commit",
            vec![make("first", 0), make("second", 1), make("third", 2)],
        );
        hook.parallel = true;
        hook.parallel_limit = Some(1);
        let rep = run_hook(&hook, &root).await.unwrap();
        assert!(rep.ok);
        let contents = std::fs::read_to_string(&marker).unwrap();
        let order: Vec<&str> = contents.lines().collect();
        assert_eq!(order, vec!["0", "1", "2"]);
    }

    #[tokio::test]
    async fn stash_restore_failure_fails_the_hook() {
        let (_d, root) = new_git_repo();
        std::fs::write(root.join("scratch.log"), "secret\n").unwrap();

        let mut hook = stub_hook(
            "pre-commit",
            vec![stub_job(
                "poison-stash",
                "printf 'interloper\\n' > other.txt && git add other.txt && git stash push --message external-stash",
            )],
        );
        hook.stash_untracked = true;

        let err = run_hook(&hook, &root).await.unwrap_err();
        let RunError::Git(GitError::Porcelain(msg)) = err else {
            panic!("expected git porcelain error");
        };
        assert!(
            msg.contains("expected top"),
            "stash restore failures must fail the hook, got: {msg}"
        );
    }

    #[tokio::test]
    async fn partially_staged_file_refuses_stash_strategy() {
        let (_d, root) = new_git_repo();
        std::fs::write(root.join("a.ts"), "staged\n").unwrap();
        let s = StdCommand::new("git")
            .current_dir(&root)
            .args(["add", "a.ts"])
            .status()
            .unwrap();
        assert!(s.success());
        std::fs::write(root.join("a.ts"), "unstaged\n").unwrap();

        let mut hook = stub_hook("pre-commit", vec![stub_job("noop", "true")]);
        hook.stash_untracked = true;

        let err = run_hook(&hook, &root).await.unwrap_err();
        let RunError::Git(GitError::Porcelain(msg)) = err else {
            panic!("expected git porcelain error");
        };
        assert!(
            msg.contains("partially staged"),
            "expected stash strategy refusal, got: {msg}"
        );
    }
}
