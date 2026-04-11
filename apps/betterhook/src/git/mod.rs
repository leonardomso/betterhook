//! Git introspection via async subprocess.
//!
//! Every function here shells out to the user's `git` binary through
//! `tokio::process::Command`. This is a deliberate design choice — it
//! sidesteps the class of worktree bug that libgit2 and gix have both
//! shared (confusing `$GIT_DIR` with `$GIT_COMMON_DIR`), inherits the
//! user's git config naturally, and costs zero binary size.

pub mod fileset;
pub mod rev_parse;

pub use fileset::{
    MAX_ARG_BYTES, TEMPLATE_VARS, all_files, build_globset, expand_template, filter_files,
    has_template, push_files, shell_escape, staged_files, unstaged_files,
};
pub use rev_parse::{
    GitError, WorktreeFlags, WorktreeInfo, git_common_dir, git_dir, run_git, show_toplevel,
    worktrees,
};
