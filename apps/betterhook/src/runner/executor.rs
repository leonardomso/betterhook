//! Hook execution orchestration.
//!
//! Both the sequential and parallel paths live here behind the single
//! `run_hook` entry point. Parallel scheduling is priority-aware
//! (directly fixing lefthook #846): jobs are lowered into priority
//! order in phase 2, and the scheduler spawns them in that order against
//! a tokio Semaphore so higher-priority jobs always acquire their permit
//! first when there is contention.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio::task::JoinSet;

use crate::config::{Hook, Job};
use crate::git::{
    StashGuard, all_files, build_globset, expand_template, filter_files, has_template, push_files,
    run_git, staged_files, unstaged_files,
};

use super::RunResult;
use super::output::{OutputEvent, SinkKind, sink};
use super::proc::{Cancel, run_command};
use crate::lock::{LockGuard, acquire_job_lock};

/// Async mutex that serializes `git add` / `git stash` / other index
/// operations across parallel jobs so concurrent writes to `.git/index`
/// don't trip the built-in `index.lock`.
type GitIndexLock = Arc<Mutex<()>>;

/// Summary of a hook run, returned to the CLI for exit-code mapping.
#[derive(Debug, Clone)]
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

struct JobPlan {
    commands: Vec<String>,
    cwd: PathBuf,
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

    let exec_res: RunResult<RunSummary> = if hook.parallel {
        run_parallel(
            hook,
            resolved,
            &tx,
            &git_lock,
            worktree,
            &common_dir,
            no_locks,
        )
        .await
    } else {
        run_sequential(
            hook,
            resolved,
            &tx,
            &git_lock,
            worktree,
            &common_dir,
            no_locks,
        )
        .await
    };

    // Always try to pop the stash, even on error. A stash-pop failure
    // is reported to stderr but does not override the primary error.
    if let Some(guard) = stash {
        let _guard = git_lock.lock().await;
        if let Err(e) = guard.pop().await {
            eprintln!("betterhook: WARNING — failed to pop untracked stash: {e}");
            eprintln!(
                "betterhook: your stash is still in `git stash list`; run `git stash pop` manually."
            );
        }
    }

    let report = exec_res?;
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

#[allow(clippy::too_many_arguments)]
async fn run_sequential(
    hook: &Hook,
    jobs: Vec<ResolvedJob>,
    tx: &mpsc::Sender<OutputEvent>,
    git_lock: &GitIndexLock,
    worktree: &Path,
    common_dir: &Path,
    no_locks: bool,
) -> RunResult<RunSummary> {
    let mut jobs_run = 0usize;
    let mut jobs_skipped = 0usize;
    let mut failed = false;

    'outer: for rj in jobs {
        let Some(plan) = rj.plan else {
            let _ = tx
                .send(OutputEvent::JobSkipped {
                    job: rj.job.name.clone(),
                    reason: "no files matched glob".to_owned(),
                })
                .await;
            jobs_skipped += 1;
            continue;
        };

        let before_unstaged = snapshot_unstaged_if_needed(&rj.job, worktree).await?;

        let (_guard, lock_env) =
            acquire_if_isolated(&rj.job, common_dir, worktree, no_locks, tx).await;
        let mut extra_env = vec![("BETTERHOOK_HOOK".to_owned(), hook.name.clone())];
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
                tx,
            )
            .await?;
            if exit != 0 {
                failed = true;
                if hook.fail_fast {
                    break 'outer;
                }
            }
        }

        if let Some(before) = before_unstaged {
            let _g = git_lock.lock().await;
            apply_stage_fixed(worktree, &before).await?;
        }

        jobs_run += 1;
    }

    Ok(RunSummary {
        ok: !failed,
        jobs_run,
        jobs_skipped,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_parallel(
    hook: &Hook,
    jobs: Vec<ResolvedJob>,
    tx: &mpsc::Sender<OutputEvent>,
    git_lock: &GitIndexLock,
    worktree: &Path,
    common_dir: &Path,
    no_locks: bool,
) -> RunResult<RunSummary> {
    let limit = hook
        .parallel_limit
        .unwrap_or_else(default_parallel_limit)
        .max(1);
    let semaphore = Arc::new(Semaphore::new(limit));
    let hook_name = hook.name.clone();
    let fail_fast = hook.fail_fast;

    let mut jobs_skipped = 0usize;
    let mut runnable: Vec<(Job, JobPlan)> = Vec::with_capacity(jobs.len());
    for rj in jobs {
        if let Some(plan) = rj.plan {
            runnable.push((rj.job, plan));
        } else {
            let _ = tx
                .send(OutputEvent::JobSkipped {
                    job: rj.job.name.clone(),
                    reason: "no files matched glob".to_owned(),
                })
                .await;
            jobs_skipped += 1;
        }
    }

    // Spawn every runnable job. Because `hook.jobs` is already sorted
    // by priority, the spawn order is priority-ordered, and the
    // semaphore hands out permits roughly in that order — fixing
    // lefthook #846's "parallel: true ignores priority" complaint.
    let cancel = Cancel::new();
    let mut set: JoinSet<Result<JobOutcome, crate::runner::RunError>> = JoinSet::new();
    for (job, plan) in runnable {
        let sem = semaphore.clone();
        let tx = tx.clone();
        let hook_name = hook_name.clone();
        let git_lock = git_lock.clone();
        let worktree = worktree.to_path_buf();
        let common_dir = common_dir.to_path_buf();
        let cancel = cancel.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore closed");
            let before_unstaged = snapshot_unstaged_if_needed(&job, &worktree).await?;
            let (_lock, lock_env) =
                acquire_if_isolated(&job, &common_dir, &worktree, no_locks, &tx).await;
            let mut extra_env = vec![("BETTERHOOK_HOOK".to_owned(), hook_name)];
            extra_env.extend(lock_env);
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
                    &tx,
                )
                .await?;
                if exit != 0 {
                    job_failed = true;
                    break;
                }
            }
            if let Some(before) = before_unstaged {
                let _g = git_lock.lock().await;
                apply_stage_fixed(&worktree, &before).await?;
            }
            Ok(JobOutcome { failed: job_failed })
        });
    }

    let mut failed = false;
    let mut jobs_run = 0usize;
    while let Some(res) = set.join_next().await {
        let outcome = res.expect("joinset task panicked")?;
        jobs_run += 1;
        if outcome.failed {
            failed = true;
            if fail_fast {
                // Signal every in-flight run_command to kill its child,
                // then drain. This is reliable where `abort_all` isn't:
                // tokio's cancellation may not drop the Child future
                // synchronously, so we use an explicit notify.
                cancel.cancel();
                while set.join_next().await.is_some() {}
                break;
            }
        }
    }

    Ok(RunSummary {
        ok: !failed,
        jobs_run,
        jobs_skipped,
    })
}

struct JobOutcome {
    failed: bool,
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
async fn apply_stage_fixed(worktree: &Path, before: &HashSet<PathBuf>) -> RunResult<()> {
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
    Ok(Some(JobPlan { commands, cwd }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IsolateSpec;
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
        let (_d, root) = new_git_repo();
        let mut hook = stub_hook(
            "pre-commit",
            (0..4)
                .map(|i| stub_job(&format!("j{i}"), "sleep 0.05 && true"))
                .collect(),
        );
        hook.parallel = true;
        hook.parallel_limit = Some(4);
        let t0 = std::time::Instant::now();
        let rep = run_hook(&hook, &root).await.unwrap();
        let elapsed = t0.elapsed();
        assert!(rep.ok);
        assert_eq!(rep.jobs_run, 4);
        assert!(
            elapsed.as_millis() < 250,
            "parallel run should finish in well under 4×50ms serially but took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn parallel_fail_fast_aborts_remaining_jobs() {
        let (_d, root) = new_git_repo();
        let mut hook = stub_hook(
            "pre-commit",
            vec![
                stub_job("fail", "exit 1"),
                stub_job("slow", "sleep 5 && true"),
            ],
        );
        hook.parallel = true;
        hook.parallel_limit = Some(2);
        hook.fail_fast = true;
        let t0 = std::time::Instant::now();
        let rep = run_hook(&hook, &root).await.unwrap();
        let elapsed = t0.elapsed();
        assert!(!rep.ok);
        assert!(
            elapsed.as_millis() < 2_000,
            "fail_fast should abort the slow job, elapsed={elapsed:?}"
        );
    }

    #[tokio::test]
    async fn per_job_timeout_kills_child_and_reports_124() {
        let (_d, root) = new_git_repo();
        let mut job = stub_job("slow", "sleep 5");
        job.timeout = Some(std::time::Duration::from_millis(200));
        let hook = stub_hook("pre-commit", vec![job]);
        let t0 = std::time::Instant::now();
        let rep = run_hook(&hook, &root).await.unwrap();
        let elapsed = t0.elapsed();
        assert!(!rep.ok, "timed-out job reports failure");
        assert!(
            elapsed.as_millis() < 1_000,
            "timeout should fire in ~200ms, took {elapsed:?}"
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
}
