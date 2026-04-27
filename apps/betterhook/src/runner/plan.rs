use std::path::{Path, PathBuf};

use crate::config::{Hook, Job};
use crate::git::{
    all_files, build_globset, expand_template, filter_files, has_template, push_files, staged_files,
};

use super::RunResult;

#[derive(Clone)]
pub(super) struct JobPlan {
    pub commands: Vec<String>,
    pub cwd: PathBuf,
    /// Files the job would operate on after `glob` and `exclude`
    /// filtering. Used as the content input for cache keys.
    pub files: Vec<PathBuf>,
}

/// Resolved per-job plan: the commands to run (already template-expanded
/// and chunked) plus the cwd and extra env bag. `None` means the job
/// had a template but no files matched and we should emit `JobSkipped`.
pub(super) struct ResolvedJob {
    pub job: Job,
    pub plan: Option<JobPlan>,
}

/// Default parallel limit when the hook doesn't specify one.
pub(super) fn default_parallel_limit() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
}

/// Resolve the plan for one job: file set → filter → template expansion.
///
/// Returns `None` when the job has a template but zero files matched,
/// signaling "skip with reason 'no files matched glob'" to the caller.
pub(super) async fn resolve_job_plan(
    hook: &Hook,
    job: &Job,
    worktree: &Path,
) -> RunResult<Option<JobPlan>> {
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
    } else if job.run.contains("{push_files}") || hook.name.as_str() == "pre-push" {
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
