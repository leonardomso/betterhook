use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

use crate::config::Job;
use crate::git::{run_git, unstaged_files};
use crate::lock::{FileLock, LockGuard, acquire_job_lock};

use super::output::OutputEvent;
use super::{RunError, RunResult};

/// Async mutex that serializes `git add` / `git stash` / other index
/// operations across parallel jobs so concurrent writes to `.git/index`
/// don't trip the built-in `index.lock`.
pub(super) type GitIndexLock = Arc<Mutex<()>>;

pub(super) async fn acquire_repo_stash_lock(common_dir: &Path) -> RunResult<FileLock> {
    let common_dir = common_dir.to_path_buf();
    let lock_dir = common_dir.join("betterhook").join("locks");
    tokio::task::spawn_blocking(move || FileLock::acquire(&common_dir, "repo-stash"))
        .await
        .map_err(|source| RunError::Io {
            path: lock_dir.clone(),
            source: std::io::Error::other(format!("repo-stash lock task failed: {source}")),
        })?
        .map_err(|source| RunError::Io {
            path: lock_dir,
            source,
        })
}

/// If `job` declares an `isolate` spec, acquire the appropriate lock
/// (flock via the client) and return a guard holding it for the
/// duration of the job. On `no_locks`, print a one-line warning and
/// return without locking.
pub(super) async fn acquire_if_isolated(
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
                job: job.name.to_string(),
                reason: "BETTERHOOK_NO_LOCKS set — running unlocked".to_owned(),
            })
            .await;
        return (None, Vec::new());
    }
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
pub(super) async fn snapshot_unstaged_if_needed(
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
/// that weren't dirty before.
pub(super) async fn apply_stage_fixed(
    worktree: &Path,
    before: &HashSet<PathBuf>,
    git_lock: &GitIndexLock,
) -> RunResult<()> {
    let after: HashSet<PathBuf> = unstaged_files(worktree).await?.into_iter().collect();
    let newly: Vec<PathBuf> = after.difference(before).cloned().collect();
    if newly.is_empty() {
        return Ok(());
    }
    let mut args: Vec<OsString> = Vec::with_capacity(2 + newly.len());
    args.push("add".into());
    args.push("--".into());
    for p in &newly {
        args.push(p.as_os_str().to_os_string());
    }
    let _g = git_lock.lock().await;
    run_git(worktree, args).await?;
    Ok(())
}
