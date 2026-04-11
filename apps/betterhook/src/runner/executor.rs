//! Hook execution orchestration.
//!
//! Phase 8 ships only the sequential path — jobs run one after another
//! in priority order, respecting `fail_fast`. Phase 9 swaps in the
//! parallel scheduler behind the same `run_hook` entry point.

use std::path::{Path, PathBuf};
use std::time::Instant;

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

/// Run every job in `hook` sequentially in priority order.
pub async fn run_hook(hook: &Hook, worktree: &Path) -> RunResult<ExecutionReport> {
    let (tx, writer) = tty_sink();
    let start = Instant::now();

    let mut jobs_run = 0usize;
    let mut jobs_skipped = 0usize;
    let mut failed = false;

    'outer: for job in &hook.jobs {
        let files = resolve_files(hook, job, worktree).await?;
        let commands = build_commands(job, &files);

        if commands.is_empty() {
            let _ = tx
                .send(OutputEvent::JobSkipped {
                    job: job.name.clone(),
                    reason: "no files matched glob".to_owned(),
                })
                .await;
            jobs_skipped += 1;
            continue;
        }

        let extra_env = vec![("BETTERHOOK_HOOK".to_owned(), hook.name.clone())];

        let job_cwd = job
            .root
            .as_ref()
            .map_or_else(|| worktree.to_path_buf(), |r| worktree.join(r));

        for cmd in &commands {
            let exit = run_command(&job.name, cmd, &job_cwd, &job.env, &extra_env, &tx).await?;
            if exit != 0 {
                failed = true;
                if hook.fail_fast {
                    break 'outer;
                }
            }
        }
        jobs_run += 1;
    }

    let total = start.elapsed();
    let _ = tx
        .send(OutputEvent::Summary {
            ok: !failed,
            jobs_run,
            jobs_skipped,
            total,
        })
        .await;
    drop(tx);
    let _ = writer.await;

    Ok(ExecutionReport {
        hook_name: hook.name.clone(),
        ok: !failed,
        jobs_run,
        jobs_skipped,
        duration_ms: total.as_millis(),
    })
}

/// Fetch the right file set for `job` based on which template variable
/// its run command references (and on the hook type for pre-push).
async fn resolve_files(hook: &Hook, job: &Job, worktree: &Path) -> RunResult<Vec<PathBuf>> {
    if !has_template(&job.run) && job.glob.is_empty() && job.exclude.is_empty() {
        return Ok(Vec::new());
    }
    let raw = if job.run.contains("{all_files}") {
        all_files(worktree).await?
    } else if job.run.contains("{push_files}") || hook.name == "pre-push" {
        // Phase 8 uses HEAD~1 as a coarse default; phase 11+ reads the
        // actual remote ref from the hook args.
        match push_files(worktree, "HEAD~1").await {
            Ok(files) => files,
            Err(_) => all_files(worktree).await?,
        }
    } else {
        staged_files(worktree).await?
    };

    let include = build_globset(&job.glob)?;
    let exclude = build_globset(&job.exclude)?;
    Ok(filter_files(raw, include.as_ref(), exclude.as_ref()))
}

/// Turn a job's `run` + resolved files into one or more concrete shell
/// commands. No-template jobs run exactly once. Template jobs with zero
/// matching files return an empty vec (the caller turns that into a
/// "skipped: no files matched glob").
fn build_commands(job: &Job, files: &[PathBuf]) -> Vec<String> {
    if has_template(&job.run) {
        if files.is_empty() {
            return Vec::new();
        }
        expand_template(&job.run, files)
    } else {
        vec![job.run.clone()]
    }
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
}
