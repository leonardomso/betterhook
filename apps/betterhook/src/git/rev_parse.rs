//! Async `git rev-parse` and `git worktree list` wrappers.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use miette::Diagnostic;
use thiserror::Error;
use tokio::process::Command;

#[derive(Debug, Error, Diagnostic)]
pub enum GitError {
    #[error("failed to spawn git at {cwd}: {source}")]
    #[diagnostic(
        code(betterhook::git::spawn),
        help("make sure the `git` binary is installed and on PATH")
    )]
    Spawn {
        cwd: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("git {args} (cwd {cwd}) exited with status {status}: {stderr}")]
    #[diagnostic(code(betterhook::git::non_zero_exit))]
    NonZero {
        args: String,
        cwd: PathBuf,
        status: i32,
        stderr: String,
    },

    #[error("git produced invalid UTF-8 output")]
    #[diagnostic(code(betterhook::git::utf8))]
    Utf8(#[from] std::string::FromUtf8Error),

    #[error("unexpected git porcelain output: {0}")]
    #[diagnostic(code(betterhook::git::porcelain))]
    Porcelain(String),
}

pub type GitResult<T> = Result<T, GitError>;

/// Spawn a `git` subprocess in `cwd` with the given args, collect stdout,
/// and return it on exit-0. On non-zero exit, returns a `NonZero` error
/// with the collected stderr for diagnostics.
pub async fn run_git<I, S>(cwd: &Path, args: I) -> GitResult<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<std::ffi::OsString> = args
        .into_iter()
        .map(|a| a.as_ref().to_os_string())
        .collect();
    let output = Command::new("git")
        .current_dir(cwd)
        .args(&args)
        .output()
        .await
        .map_err(|source| GitError::Spawn {
            cwd: cwd.to_path_buf(),
            source,
        })?;

    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(GitError::NonZero {
            args: args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" "),
            cwd: cwd.to_path_buf(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

async fn git_stdout_line(cwd: &Path, args: &[&str]) -> GitResult<String> {
    let bytes = run_git(cwd, args).await?;
    let text = String::from_utf8(bytes)?;
    Ok(text.trim_end().to_string())
}

/// Return the absolute path to this worktree's private git dir.
///
/// For the primary worktree this is `<repo>/.git`; for a linked worktree
/// this is `<repo>/.git/worktrees/<id>`.
pub async fn git_dir(cwd: &Path) -> GitResult<PathBuf> {
    Ok(PathBuf::from(
        git_stdout_line(cwd, &["rev-parse", "--absolute-git-dir"]).await?,
    ))
}

/// Return the absolute path to the shared git common dir.
///
/// This is the directory git consults for `hooks/`, `config`, `refs/`,
/// and is the same across every worktree of a repo. Our wrapper scripts
/// live here.
pub async fn git_common_dir(cwd: &Path) -> GitResult<PathBuf> {
    let raw = git_stdout_line(cwd, &["rev-parse", "--git-common-dir"]).await?;
    let p = PathBuf::from(&raw);
    let absolute = if p.is_absolute() { p } else { cwd.join(p) };
    // Canonicalize best-effort — we don't want to fail just because some
    // intermediate symlink doesn't resolve.
    Ok(std::fs::canonicalize(&absolute).unwrap_or(absolute))
}

/// Return the absolute path of the worktree's root.
///
/// Inside a hook that was invoked by git, `--show-toplevel` reliably
/// returns the current worktree even though the wrapper script lives in
/// the shared common dir. This is the git feature lefthook fails to use.
pub async fn show_toplevel(cwd: &Path) -> GitResult<PathBuf> {
    Ok(PathBuf::from(
        git_stdout_line(cwd, &["rev-parse", "--show-toplevel"]).await?,
    ))
}

/// Boolean state flags from `git worktree list --porcelain`. These map
/// 1:1 to git's porcelain keys so we suppress the many-bools lint.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WorktreeFlags {
    pub bare: bool,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// Info about a single worktree, parsed from `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub flags: WorktreeFlags,
}

/// Enumerate every worktree attached to the repo containing `cwd`.
pub async fn worktrees(cwd: &Path) -> GitResult<Vec<WorktreeInfo>> {
    let bytes = run_git(cwd, ["worktree", "list", "--porcelain"]).await?;
    let text = String::from_utf8(bytes)?;
    parse_worktree_porcelain(&text)
}

fn parse_worktree_porcelain(text: &str) -> GitResult<Vec<WorktreeInfo>> {
    let mut out = Vec::new();
    let mut current: Option<WorktreeInfo> = None;

    for line in text.lines() {
        if line.is_empty() {
            if let Some(wt) = current.take() {
                out.push(wt);
            }
            continue;
        }
        let (key, value) = match line.split_once(' ') {
            Some((k, v)) => (k, v),
            None => (line, ""),
        };
        match key {
            "worktree" => {
                if let Some(wt) = current.take() {
                    out.push(wt);
                }
                current = Some(WorktreeInfo {
                    path: PathBuf::from(value),
                    head: None,
                    branch: None,
                    flags: WorktreeFlags::default(),
                });
            }
            "HEAD" => {
                let wt = current
                    .as_mut()
                    .ok_or_else(|| GitError::Porcelain(format!("HEAD without worktree: {line}")))?;
                wt.head = Some(value.to_owned());
            }
            "branch" => {
                let wt = current.as_mut().ok_or_else(|| {
                    GitError::Porcelain(format!("branch without worktree: {line}"))
                })?;
                wt.branch = Some(value.to_owned());
            }
            "bare" => {
                if let Some(wt) = current.as_mut() {
                    wt.flags.bare = true;
                }
            }
            "detached" => {
                if let Some(wt) = current.as_mut() {
                    wt.flags.detached = true;
                }
            }
            "locked" => {
                if let Some(wt) = current.as_mut() {
                    wt.flags.locked = true;
                }
            }
            "prunable" => {
                if let Some(wt) = current.as_mut() {
                    wt.flags.prunable = true;
                }
            }
            _ => { /* ignore unknown keys for forward-compat */ }
        }
    }
    if let Some(wt) = current.take() {
        out.push(wt);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::new_git_repo_with_file;
    use std::process::Command as StdCommand;

    #[tokio::test]
    async fn rev_parse_primary_worktree() {
        let (_dir, root) = new_git_repo_with_file("README.md", "hi\n");

        let toplevel = show_toplevel(&root).await.unwrap();
        // macOS tempdirs can be under /private/var/... symlink
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        let canonical_toplevel = std::fs::canonicalize(&toplevel).unwrap();
        assert_eq!(canonical_toplevel, canonical_root);

        let git_dir = git_dir(&root).await.unwrap();
        assert!(git_dir.ends_with(".git"), "git_dir = {git_dir:?}");

        let common = git_common_dir(&root).await.unwrap();
        assert!(common.ends_with(".git"), "common = {common:?}");
    }

    #[tokio::test]
    async fn worktree_list_shows_primary() {
        let (_dir, root) = new_git_repo_with_file("README.md", "hi\n");
        let wts = worktrees(&root).await.unwrap();
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].branch.as_deref(), Some("refs/heads/main"));
    }

    #[tokio::test]
    async fn linked_worktree_shares_common_dir_but_has_own_git_dir() {
        let (dir, root) = new_git_repo_with_file("README.md", "hi\n");
        let linked = dir.path().join("wt-a");
        let status = StdCommand::new("git")
            .current_dir(&root)
            .args([
                "worktree",
                "add",
                linked.to_str().unwrap(),
                "-b",
                "feature-a",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "worktree add failed");

        let primary_common = git_common_dir(&root).await.unwrap();
        let linked_common = git_common_dir(&linked).await.unwrap();
        assert_eq!(
            primary_common, linked_common,
            "common dir must be shared across worktrees"
        );

        let primary_git_dir = git_dir(&root).await.unwrap();
        let linked_git_dir = git_dir(&linked).await.unwrap();
        assert_ne!(
            primary_git_dir, linked_git_dir,
            "each worktree must have its own private git-dir"
        );
        assert!(linked_git_dir.to_string_lossy().contains("worktrees/wt-a"));

        let wts = worktrees(&root).await.unwrap();
        assert_eq!(wts.len(), 2, "primary + 1 linked = 2");

        // show_toplevel must return the *current* worktree root when
        // invoked from within a linked worktree — this is the property
        // our wrapper relies on at runtime.
        let linked_top = show_toplevel(&linked).await.unwrap();
        assert_eq!(
            std::fs::canonicalize(&linked_top).unwrap(),
            std::fs::canonicalize(&linked).unwrap()
        );
    }

    #[test]
    fn parse_porcelain_handles_primary_and_detached() {
        let text = "\
worktree /tmp/a
HEAD abc123
branch refs/heads/main

worktree /tmp/b
HEAD def456
detached

";
        let wts = parse_worktree_porcelain(text).unwrap();
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].path, PathBuf::from("/tmp/a"));
        assert_eq!(wts[0].branch.as_deref(), Some("refs/heads/main"));
        assert!(!wts[0].flags.detached);
        assert_eq!(wts[1].path, PathBuf::from("/tmp/b"));
        assert!(wts[1].flags.detached);
        assert!(wts[1].branch.is_none());
    }
}
