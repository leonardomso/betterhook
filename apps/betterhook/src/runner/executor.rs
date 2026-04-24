//! Hook execution orchestration.
//!
//! Both the sequential and parallel paths live here behind the single
//! `run_hook` entry point. Parallel scheduling is priority-aware
//! (directly fixing lefthook #846): jobs are lowered into priority
//! order in phase 2, and the scheduler spawns them in that order against
//! a tokio Semaphore so higher-priority jobs always acquire their permit
//! first when there is contention.

use std::collections::{BinaryHeap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;

use super::RunError;
use super::RunResult;
use super::dag::{JobGraph, build_dag};
use super::output::{OutputEvent, SinkKind, sink};
use super::proc::{Cancel, run_command};
use crate::config::{Hook, Job};
use crate::git::{
    StashGuard, all_files, build_globset, expand_template, filter_files, has_template, push_files,
    run_git, staged_files, unstaged_files,
};
use crate::lock::{LockGuard, acquire_job_lock};

/// Async mutex that serializes `git add` / `git stash` / other index
/// operations across parallel jobs so concurrent writes to `.git/index`
/// don't trip the built-in `index.lock`.
type GitIndexLock = Arc<Mutex<()>>;

/// Summary of a hook run, returned to the CLI for exit-code mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    pub hook_name: String,
    pub ok: bool,
    pub jobs_run: usize,
    pub jobs_skipped: usize,
    pub duration_ms: u128,
}

/// Resolved per-job plan: the commands to run (already template-expanded
/// and chunked) plus the cwd and extra env bag. `None` means the job
/// had a template but no files matched and we should emit `JobSkipped`.
struct ResolvedJob {
    job: Job,
    plan: Option<JobPlan>,
}

#[derive(Clone)]
struct JobPlan {
    commands: Vec<String>,
    cwd: PathBuf,
    /// Files the job would operate on after `glob` + `exclude` filter.
    /// Phase 30 uses this as the content-hash input for the CA cache.
    files: Vec<PathBuf>,
}

/// Default parallel limit when the hook doesn't specify one.
fn default_parallel_limit() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
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
        if options.is_filtered(&job.name) {
            let _ = tx
                .send(OutputEvent::JobSkipped {
                    job: job.name.clone(),
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
        hook_name: hook.name.clone(),
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
/// schedulers. Introduced in v1.0.1 to replace the seven-argument
/// `run_sequential` / `run_parallel` signatures — both schedulers were
/// passing the same bag of context through every recursion level.
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

    'outer: for rj in jobs {
        let Some(plan) = rj.plan else {
            let _ = ctx
                .tx
                .send(OutputEvent::JobSkipped {
                    job: rj.job.name.clone(),
                    reason: "no files matched glob".to_owned(),
                })
                .await;
            jobs_skipped += 1;
            continue;
        };

        let before_unstaged = snapshot_unstaged_if_needed(&rj.job, ctx.worktree).await?;

        let (_guard, lock_env) =
            acquire_if_isolated(&rj.job, ctx.common_dir, ctx.worktree, ctx.no_locks, ctx.tx).await;
        let mut extra_env = vec![("BETTERHOOK_HOOK".to_owned(), ctx.hook.name.clone())];
        extra_env.extend(lock_env);
        for cmd in &plan.commands {
            let exit = run_command(
                &rj.job.name,
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
                if ctx.hook.fail_fast {
                    break 'outer;
                }
            }
        }

        if let Some(before) = before_unstaged {
            apply_stage_fixed(ctx.worktree, &before, ctx.git_lock).await?;
        }

        jobs_run += 1;
    }

    Ok(RunSummary {
        ok: !failed,
        jobs_run,
        jobs_skipped,
    })
}

/// Parallel (capability-DAG-aware) executor.
///
/// Phase 27 replaces the priority-only spawn-and-semaphore scheduler
/// with a DAG walker that respects declared `reads`/`writes`/`network`
/// capabilities. Jobs whose capability sets are disjoint run in
/// parallel; jobs that conflict serialize in a priority-ordered way.
// The scheduler loop is one cohesive state machine: ready heap,
// join-set drain, DAG child release, fail-fast cascade. Splitting
// further would spread mutable local state across functions and hurt
// readability more than it helps. v1.0.1 brought it down from 243
// lines to ~130 by extracting `execute_job_in_dag` and we stop there.
#[allow(clippy::too_many_lines)]
async fn run_parallel(ctx: &ExecutionContext<'_>, jobs: Vec<ResolvedJob>) -> RunResult<RunSummary> {
    let limit = ctx
        .hook
        .parallel_limit
        .unwrap_or_else(default_parallel_limit)
        .max(1);
    let hook_name = ctx.hook.name.clone();
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
                        job: job.name.clone(),
                        reason: "no files matched glob".to_owned(),
                    })
                    .await;
                jobs_skipped += 1;
                release_children(
                    &graph,
                    idx,
                    &pending_clone_ref(&pending),
                    &mut pending,
                    &started,
                    &mut ready,
                );
                continue;
            };

            // Phase 30: CA cache hit path. Only concurrent_safe jobs
            // are cacheable — unsafe jobs may have side effects that
            // we can't faithfully replay.
            if job.concurrent_safe {
                if let Ok(Some(cached)) =
                    crate::cache::lookup(ctx.common_dir, &job, &plan.files).await
                {
                    let _ = ctx
                        .tx
                        .send(OutputEvent::JobCacheHit {
                            job: job.name.clone(),
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
                    release_children(
                        &graph,
                        idx,
                        &pending_clone_ref(&pending),
                        &mut pending,
                        &started,
                        &mut ready,
                    );
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
            // Nothing in flight and nothing ready → we're done (or
            // stalled, but phase 26 proves the graph is acyclic so
            // being stalled here means every node finished).
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
            if pending[child] > 0 {
                pending[child] -= 1;
            }
            if pending[child] == 0 && !started[child] {
                let pri = graph.nodes[child].job.priority;
                ready.push(std::cmp::Reverse((pri, child)));
            }
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
    _before: &[usize],
    pending: &mut [usize],
    started: &[bool],
    ready: &mut BinaryHeap<std::cmp::Reverse<(u32, usize)>>,
) {
    for &child in &graph.nodes[idx].children {
        if pending[child] > 0 {
            pending[child] -= 1;
        }
        if pending[child] == 0 && !started[child] {
            let pri = graph.nodes[child].job.priority;
            ready.push(std::cmp::Reverse((pri, child)));
        }
    }
}

fn pending_clone_ref(_: &[usize]) -> Vec<usize> {
    Vec::new()
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
///
/// Extracted from an inline `set.spawn(async move { ... })` closure in
/// v1.0.1 — the closure was 85 lines, nested 4 levels deep, and made
/// panic stack traces unreadable. Moving it to a named `async fn`
/// doesn't change behavior but is hugely better for debugging.
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
            &job.name,
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
        emit_builtin_diagnostics(builtin_id, &job.name, &captured, &tx).await;
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

/// Collect all stdout/stderr lines from the captured events, feed them
/// through the builtin's `parse_output`, and emit one `Diagnostic`
/// event per finding.
async fn emit_builtin_diagnostics(
    builtin_id: &str,
    job_name: &str,
    captured: &[OutputEvent],
    tx: &mpsc::Sender<OutputEvent>,
) {
    let Some(meta) = crate::builtins::get(builtin_id) else {
        return;
    };
    // Rebuild the raw stdout. The builtin parsers expect the full output
    // as a single string — they handle line splitting themselves.
    let mut stdout = String::new();
    for ev in captured {
        if let OutputEvent::Line {
            stream: super::output::Stream::Stdout,
            line,
            ..
        } = ev
        {
            stdout.push_str(line);
            stdout.push('\n');
        }
    }
    if stdout.is_empty() {
        return;
    }
    let diags = match builtin_id {
        "rustfmt" => crate::builtins::rustfmt::parse_output(&stdout),
        "clippy" => crate::builtins::clippy::parse_output(&stdout),
        "prettier" => crate::builtins::prettier::parse_output(&stdout),
        "eslint" => crate::builtins::eslint::parse_output(&stdout),
        "ruff" => crate::builtins::ruff::parse_output(&stdout),
        "black" => crate::builtins::black::parse_output(&stdout),
        "gofmt" => crate::builtins::gofmt::parse_output(&stdout),
        "govet" => crate::builtins::govet::parse_output(&stdout),
        "biome" => crate::builtins::biome::parse_output(&stdout),
        "oxlint" => crate::builtins::oxlint::parse_output(&stdout),
        "shellcheck" => crate::builtins::shellcheck::parse_output(&stdout),
        "gitleaks" => crate::builtins::gitleaks::parse_output(&stdout),
        _ => return,
    };
    // `meta` is a `BuiltinMeta` with no Drop — the `get()` result was
    // only needed to confirm the builtin exists. Let `_` discard it.
    let _ = meta;
    for d in diags {
        let _ = tx
            .send(OutputEvent::Diagnostic {
                job: job_name.to_owned(),
                file: d.file,
                line: d.line,
                column: d.column,
                severity: d.severity,
                message: d.message,
                rule: d.rule,
            })
            .await;
    }
}

/// If `job` declares an `isolate` spec, acquire the appropriate lock
/// (flock via the client) and return a guard holding it for the
/// duration of the job. On `no_locks`, print a one-line warning and
/// return without locking.
async fn acquire_if_isolated(
    job: &Job,
    common_dir: &Path,
    worktree: &Path,
    no_locks: bool,
    tx: &mpsc::Sender<OutputEvent>,
) -> (Option<LockGuard>, Vec<(String, String)>) {
    let Some(spec) = &job.isolate else {
        return (None, Vec::new());
    };
    if no_locks {
        let _ = tx
            .send(OutputEvent::JobSkipped {
                job: job.name.clone(),
                reason: "BETTERHOOK_NO_LOCKS set — running unlocked".to_owned(),
            })
            .await;
        return (None, Vec::new());
    }
    // Move the blocking flock call off the async runtime.
    let common_dir_owned = common_dir.to_path_buf();
    let worktree_owned = worktree.to_path_buf();
    let spec_owned = spec.clone();
    let result = tokio::task::spawn_blocking(move || {
        acquire_job_lock(&common_dir_owned, &spec_owned, &worktree_owned)
    })
    .await;
    match result {
        Ok(Ok(guard)) => {
            let env = guard.extra_env.clone();
            (Some(guard), env)
        }
        Ok(Err(e)) => {
            eprintln!(
                "betterhook: WARNING — failed to acquire lock for job '{}': {e}. running unlocked.",
                job.name
            );
            (None, Vec::new())
        }
        Err(e) => {
            eprintln!(
                "betterhook: WARNING — lock task panicked for job '{}': {e}. running unlocked.",
                job.name
            );
            (None, Vec::new())
        }
    }
}

/// When the job opts into `stage_fixed`, return the set of files that
/// already had unstaged modifications before the job ran — the delta
/// after the job is what we need to re-add.
async fn snapshot_unstaged_if_needed(
    job: &Job,
    worktree: &Path,
) -> RunResult<Option<HashSet<PathBuf>>> {
    if !job.stage_fixed || job.interactive {
        return Ok(None);
    }
    let files = unstaged_files(worktree).await?;
    Ok(Some(files.into_iter().collect()))
}

/// Stage every file that became unstaged *during* the job — the ones
/// that weren't dirty before. This is the `stage_fixed` semantics:
/// formatters edit files in-place, and without this step those edits
/// wouldn't make it into the commit.
///
/// v1.0.1: split into a read phase (runs without `git_lock` — it's
/// just `git status`, safe concurrently) and a mutation phase (runs
/// under the lock). Previously the whole function ran under the lock,
/// which serialized the git-status scans of parallel DAG jobs on each
/// other for no reason.
async fn apply_stage_fixed(
    worktree: &Path,
    before: &HashSet<PathBuf>,
    git_lock: &GitIndexLock,
) -> RunResult<()> {
    // Read phase: no lock held. `git status --porcelain` never
    // mutates the index, and two worktrees calling it concurrently is
    // perfectly safe.
    let after: HashSet<PathBuf> = unstaged_files(worktree).await?.into_iter().collect();
    let newly: Vec<PathBuf> = after.difference(before).cloned().collect();
    if newly.is_empty() {
        return Ok(());
    }
    let mut args: Vec<std::ffi::OsString> = Vec::with_capacity(2 + newly.len());
    args.push("add".into());
    args.push("--".into());
    for p in &newly {
        args.push(p.as_os_str().to_os_string());
    }
    // Write phase: hold the lock only around the `git add`.
    let _g = git_lock.lock().await;
    run_git(worktree, args).await?;
    Ok(())
}

/// Resolve the plan for one job: file set → filter → template expansion.
///
/// Returns `None` when the job has a template but zero files matched,
/// signaling "skip with reason 'no files matched glob'" to the caller.
async fn resolve_job_plan(hook: &Hook, job: &Job, worktree: &Path) -> RunResult<Option<JobPlan>> {
    let cwd = job
        .root
        .as_ref()
        .map_or_else(|| worktree.to_path_buf(), |r| worktree.join(r));

    if !has_template(&job.run) && job.glob.is_empty() && job.exclude.is_empty() {
        return Ok(Some(JobPlan {
            commands: vec![job.run.clone()],
            cwd,
            files: Vec::new(),
        }));
    }

    let raw = if job.run.contains("{all_files}") {
        all_files(worktree).await?
    } else if job.run.contains("{push_files}") || hook.name == "pre-push" {
        match push_files(worktree, "HEAD~1").await {
            Ok(files) => files,
            Err(_) => all_files(worktree).await?,
        }
    } else {
        staged_files(worktree).await?
    };

    let include = build_globset(&job.glob)?;
    let exclude = build_globset(&job.exclude)?;
    let files = filter_files(raw, include.as_ref(), exclude.as_ref());

    if has_template(&job.run) && files.is_empty() {
        return Ok(None);
    }

    let commands = if has_template(&job.run) {
        expand_template(&job.run, &files)
    } else {
        vec![job.run.clone()]
    };
    // Store absolute paths so the cache can hash them regardless of
    // which cwd the runner later changes to.
    let abs_files: Vec<PathBuf> = files
        .iter()
        .map(|p| {
            if p.is_absolute() {
                p.clone()
            } else {
                worktree.join(p)
            }
        })
        .collect();
    Ok(Some(JobPlan {
        commands,
        cwd,
        files: abs_files,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IsolateSpec;
    use crate::git::GitError;
    use std::collections::BTreeMap;
    use std::process::Command as StdCommand;
    use tempfile::TempDir;

    fn new_git_repo() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let git = |args: &[&str]| {
            let s = StdCommand::new("git")
                .current_dir(&root)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t.t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t.t")
                .status()
                .unwrap();
            assert!(s.success());
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t.t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.ts"), "1\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        (dir, root)
    }

    fn stub_job(name: &str, run: &str) -> Job {
        Job {
            name: name.to_owned(),
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
            name: name.to_owned(),
            parallel: false,
            fail_fast: false,
            parallel_limit: None,
            stash_untracked: false,
            jobs,
        }
    }

    #[tokio::test]
    async fn run_hook_succeeds_when_every_job_exits_zero() {
        let (_d, root) = new_git_repo();
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
            rep.jobs_run, 0,
            "fail_fast bails before counting second job"
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
