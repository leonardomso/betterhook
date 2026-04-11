//! Hook execution orchestration.
//!
//! Both the sequential and parallel paths live here behind the single
//! `run_hook` entry point. Parallel scheduling is priority-aware
//! (directly fixing lefthook #846): jobs are lowered into priority
//! order in phase 2, and the scheduler spawns them in that order against
//! a tokio Semaphore so higher-priority jobs always acquire their permit
//! first when there is contention.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use crate::config::{Hook, Job};
use crate::git::{
    all_files, build_globset, expand_template, filter_files, has_template, push_files, staged_files,
};

use super::RunResult;
use super::output::{OutputEvent, tty_sink};
use super::proc::run_command;

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

/// Top-level entrypoint. Dispatches to the sequential or parallel
/// implementation based on `hook.parallel`.
pub async fn run_hook(hook: &Hook, worktree: &Path) -> RunResult<ExecutionReport> {
    let (tx, writer) = tty_sink();
    let start = Instant::now();

    let mut resolved: Vec<ResolvedJob> = Vec::with_capacity(hook.jobs.len());
    for job in &hook.jobs {
        let plan = resolve_job_plan(hook, job, worktree).await?;
        resolved.push(ResolvedJob {
            job: job.clone(),
            plan,
        });
    }

    let report = if hook.parallel {
        run_parallel(hook, resolved, &tx).await?
    } else {
        run_sequential(hook, resolved, &tx).await?
    };

    let total = start.elapsed();
    let _ = tx
        .send(OutputEvent::Summary {
            ok: report.ok,
            jobs_run: report.jobs_run,
            jobs_skipped: report.jobs_skipped,
            total,
        })
        .await;
    drop(tx);
    let _ = writer.await;

    Ok(ExecutionReport {
        hook_name: hook.name.clone(),
        ok: report.ok,
        jobs_run: report.jobs_run,
        jobs_skipped: report.jobs_skipped,
        duration_ms: total.as_millis(),
    })
}

#[derive(Debug, Clone, Copy)]
struct RunSummary {
    ok: bool,
    jobs_run: usize,
    jobs_skipped: usize,
}

async fn run_sequential(
    hook: &Hook,
    jobs: Vec<ResolvedJob>,
    tx: &mpsc::Sender<OutputEvent>,
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

        let extra_env = vec![("BETTERHOOK_HOOK".to_owned(), hook.name.clone())];
        for cmd in &plan.commands {
            let exit =
                run_command(&rj.job.name, cmd, &plan.cwd, &rj.job.env, &extra_env, tx).await?;
            if exit != 0 {
                failed = true;
                if hook.fail_fast {
                    break 'outer;
                }
            }
        }
        jobs_run += 1;
    }

    Ok(RunSummary {
        ok: !failed,
        jobs_run,
        jobs_skipped,
    })
}

async fn run_parallel(
    hook: &Hook,
    jobs: Vec<ResolvedJob>,
    tx: &mpsc::Sender<OutputEvent>,
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
    let mut set: JoinSet<Result<JobOutcome, crate::runner::RunError>> = JoinSet::new();
    for (job, plan) in runnable {
        let sem = semaphore.clone();
        let tx = tx.clone();
        let hook_name = hook_name.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore closed");
            let extra_env = vec![("BETTERHOOK_HOOK".to_owned(), hook_name)];
            let mut job_failed = false;
            for cmd in &plan.commands {
                let exit =
                    run_command(&job.name, cmd, &plan.cwd, &job.env, &extra_env, &tx).await?;
                if exit != 0 {
                    job_failed = true;
                    break;
                }
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
                set.abort_all();
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
            elapsed.as_millis() < 150,
            "parallel run should finish in well under 4×50ms but took {elapsed:?}"
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
    async fn parallel_limit_one_preserves_priority_order() {
        let (_d, root) = new_git_repo();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("order.log");
        let make = |name: &str, idx: usize| {
            let mut j = stub_job(name, &format!("printf '{idx}\\n' >> {}", marker.display()));
            j.priority = idx as u32;
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
