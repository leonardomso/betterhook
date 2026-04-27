//! Reversible untracked-file stashing — the lefthook #833 fix.
//!
//! Lefthook's pre-commit hooks sometimes see formatter false positives
//! because untracked files are still in the working tree while the
//! hook runs. The fix is a reversible stash: push everything not in
//! the index (with `--keep-index --include-untracked`), run the hook,
//! then pop.
//!
//! The key safety property: the stash we push carries a unique message
//! (betterhook-<pid>-<nanos>) and we verify the top-of-stack matches
//! that message before popping. If something else touched the stash
//! in between we refuse to pop silently and surface the ref so the
//! user can recover.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::rev_parse::{GitError, GitResult, run_git};
use super::{staged_files, unstaged_files};

/// A live stash entry. `pop` must be called explicitly — there is no
/// Drop-based cleanup because tokio can't await in Drop and we don't
/// want to block on one in a background thread.
#[derive(Debug)]
pub struct StashGuard {
    worktree: PathBuf,
    message: String,
    created: bool,
}

impl StashGuard {
    /// Push untracked + unstaged changes into a named stash.
    ///
    /// Returns a guard even if the working tree was clean — in that
    /// case `created` is `false` and `pop` is a no-op.
    pub async fn push(worktree: &Path) -> GitResult<Self> {
        if !has_dirty_or_untracked(worktree).await? {
            return Ok(Self {
                worktree: worktree.to_path_buf(),
                message: String::new(),
                created: false,
            });
        }
        if has_partially_staged_tracked_files(worktree).await? {
            return Err(GitError::Porcelain(
                "stash_untracked cannot safely run with partially staged tracked files; \
                 commit or unstage the extra edits first"
                    .to_owned(),
            ));
        }

        let message = unique_message();
        run_git(
            worktree,
            [
                "stash",
                "push",
                "--keep-index",
                "--include-untracked",
                "--message",
                &message,
            ],
        )
        .await?;

        Ok(Self {
            worktree: worktree.to_path_buf(),
            message,
            created: true,
        })
    }

    /// Pop the stash we created. No-op when no stash was pushed.
    ///
    /// Verifies that the top-of-stack entry carries our unique message
    /// before running `git stash pop`. A mismatch means another process
    /// or hook touched the stash — we refuse to pop and error out so
    /// the user can recover manually.
    pub async fn pop(self) -> GitResult<()> {
        if !self.created {
            return Ok(());
        }
        let Some(index) = find_stash_index(&self.worktree, &self.message).await? else {
            return Err(GitError::Porcelain(format!(
                "stash entry '{msg}' disappeared — refusing to pop",
                msg = self.message
            )));
        };
        if index != 0 {
            return Err(GitError::Porcelain(format!(
                "our stash '{msg}' is at stash@{{{index}}} (expected top); refusing to pop",
                msg = self.message
            )));
        }
        // Pop without --index: we pushed with --keep-index so the
        // index snapshot inside the stash is identical to what's
        // already staged, and passing --index would conflict with
        // that state. The working-tree portion is what we actually
        // need back.
        run_git(&self.worktree, ["stash", "pop", "stash@{0}"]).await?;
        Ok(())
    }

    /// For diagnostics: the message this stash was pushed under.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn created(&self) -> bool {
        self.created
    }
}

fn unique_message() -> String {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("betterhook-stash-{pid}-{nanos}")
}

async fn has_dirty_or_untracked(worktree: &Path) -> GitResult<bool> {
    let out = run_git(worktree, ["status", "--porcelain", "-uall"]).await?;
    Ok(!out.is_empty())
}

async fn has_partially_staged_tracked_files(worktree: &Path) -> GitResult<bool> {
    let staged: std::collections::HashSet<PathBuf> =
        staged_files(worktree).await?.into_iter().collect();
    if staged.is_empty() {
        return Ok(false);
    }
    let unstaged: std::collections::HashSet<PathBuf> =
        unstaged_files(worktree).await?.into_iter().collect();
    Ok(staged.iter().any(|path| unstaged.contains(path)))
}

async fn find_stash_index(worktree: &Path, needle: &str) -> GitResult<Option<usize>> {
    let out = run_git(worktree, ["stash", "list"]).await?;
    let text = String::from_utf8_lossy(&out);
    for (i, line) in text.lines().enumerate() {
        if line.contains(needle) {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::new_git_repo_with_file;
    use std::process::Command as StdCommand;

    #[tokio::test]
    async fn clean_tree_stash_is_noop() {
        let (_d, root) = new_git_repo_with_file("a.ts", "one\n");
        let guard = StashGuard::push(&root).await.unwrap();
        assert!(!guard.created());
        guard.pop().await.unwrap();
    }

    #[tokio::test]
    async fn untracked_file_is_stashed_and_restored() {
        let (_d, root) = new_git_repo_with_file("a.ts", "one\n");
        std::fs::write(root.join("scratch.log"), "secret\n").unwrap();
        assert!(root.join("scratch.log").exists());

        let guard = StashGuard::push(&root).await.unwrap();
        assert!(guard.created());
        // While stashed, the untracked file should be gone from the worktree.
        assert!(!root.join("scratch.log").exists());

        guard.pop().await.unwrap();
        // After pop, the file is back.
        assert!(root.join("scratch.log").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("scratch.log")).unwrap(),
            "secret\n"
        );
    }

    #[test]
    fn unique_message_is_prefixed_and_unique() {
        let first = unique_message();
        let second = unique_message();
        assert!(first.starts_with("betterhook-stash-"));
        assert!(second.starts_with("betterhook-stash-"));
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn tracked_dirty_file_is_stashed_and_restored() {
        let (_d, root) = new_git_repo_with_file("a.ts", "one\n");
        std::fs::write(root.join("a.ts"), "dirty\n").unwrap();

        let guard = StashGuard::push(&root).await.unwrap();
        assert!(guard.created());
        assert!(guard.message().starts_with("betterhook-stash-"));
        assert_eq!(std::fs::read_to_string(root.join("a.ts")).unwrap(), "one\n");

        guard.pop().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.ts")).unwrap(),
            "dirty\n"
        );
    }

    #[tokio::test]
    async fn pop_refuses_when_stash_disappears() {
        let (_d, root) = new_git_repo_with_file("a.ts", "one\n");
        std::fs::write(root.join("scratch.log"), "secret\n").unwrap();
        let guard = StashGuard::push(&root).await.unwrap();
        assert!(guard.created());

        let status = StdCommand::new("git")
            .current_dir(&root)
            .args(["stash", "drop", "stash@{0}"])
            .status()
            .unwrap();
        assert!(status.success());

        let msg = format!("{}", guard.pop().await.unwrap_err());
        assert!(
            msg.contains("disappeared"),
            "expected disappearance error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn pop_refuses_when_our_stash_is_not_on_top() {
        let (_d, root) = new_git_repo_with_file("a.ts", "one\n");
        std::fs::write(root.join("scratch.log"), "secret\n").unwrap();
        let guard = StashGuard::push(&root).await.unwrap();
        assert!(guard.created());

        std::fs::write(root.join("other.log"), "later\n").unwrap();
        let status = StdCommand::new("git")
            .current_dir(&root)
            .args([
                "stash",
                "push",
                "--include-untracked",
                "--message",
                "other-stash",
            ])
            .status()
            .unwrap();
        assert!(status.success());

        let msg = format!("{}", guard.pop().await.unwrap_err());
        assert!(
            msg.contains("stash@{1}"),
            "expected top-of-stack refusal, got: {msg}"
        );
        assert!(
            msg.contains("refusing to pop"),
            "expected refusal wording, got: {msg}"
        );
    }

    #[tokio::test]
    async fn partially_staged_tracked_file_is_rejected() {
        let (_d, root) = new_git_repo_with_file("a.ts", "one\n");
        std::fs::write(root.join("a.ts"), "staged\n").unwrap();
        StdCommand::new("git")
            .current_dir(&root)
            .args(["add", "a.ts"])
            .status()
            .unwrap();
        std::fs::write(root.join("a.ts"), "unstaged\n").unwrap();

        let err = StashGuard::push(&root).await.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("partially staged"),
            "expected partial-stage refusal, got: {msg}"
        );
    }
}
