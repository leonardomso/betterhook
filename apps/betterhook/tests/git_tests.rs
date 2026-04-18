//! Git operations: stash safety, fileset helpers, and rev-parse tests.

mod common;

use betterhook::git::{
    StashGuard, all_files, build_globset, expand_template, filter_files, git_common_dir, git_dir,
    has_template, run_git, shell_escape, show_toplevel, staged_files, unstaged_files, worktrees,
};
use common::{git, init_repo, new_repo_with_worktrees};

// ---------------------------------------------------------------------------
// rev-parse / toplevel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn show_toplevel_returns_repo_root() {
    let (_d, repo) = init_repo();
    let top = show_toplevel(&repo).await.unwrap();
    assert_eq!(top.file_name().unwrap(), repo.file_name().unwrap());
}

#[tokio::test]
async fn git_dir_returns_dot_git() {
    let (_d, repo) = init_repo();
    let gd = git_dir(&repo).await.unwrap();
    assert!(gd.ends_with(".git"));
}

#[tokio::test]
async fn git_common_dir_returns_dot_git() {
    let (_d, repo) = init_repo();
    let cd = git_common_dir(&repo).await.unwrap();
    assert!(cd.ends_with(".git"));
}

#[tokio::test]
async fn git_common_dir_same_as_git_dir_in_primary() {
    let (_d, repo) = init_repo();
    let gd = git_dir(&repo).await.unwrap();
    let cd = git_common_dir(&repo).await.unwrap();
    assert_eq!(gd, cd);
}

#[tokio::test]
async fn run_git_simple_command() {
    let (_d, repo) = init_repo();
    let out = run_git(&repo, ["rev-parse", "HEAD"]).await.unwrap();
    let sha = String::from_utf8_lossy(&out);
    assert_eq!(sha.trim().len(), 40, "HEAD should be a 40-char hex SHA");
}

// ---------------------------------------------------------------------------
// Worktree listing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn worktrees_lists_primary() {
    let (_d, repo) = init_repo();
    let wts = worktrees(&repo).await.unwrap();
    assert!(!wts.is_empty(), "should list at least the primary worktree");
}

#[tokio::test]
async fn worktrees_lists_linked() {
    let (_d, primary, linked) = new_repo_with_worktrees(2);
    let wts = worktrees(&primary).await.unwrap();
    assert!(wts.len() >= 3, "should list primary + 2 linked worktrees");
    for lw in &linked {
        let canonical = lw.canonicalize().unwrap_or_else(|_| lw.clone());
        assert!(
            wts.iter().any(|w| {
                let wc = w.path.canonicalize().unwrap_or_else(|_| w.path.clone());
                wc == canonical
            }),
            "linked worktree {} should appear in the list",
            lw.display()
        );
    }
}

// ---------------------------------------------------------------------------
// Staged / unstaged files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn staged_files_after_add() {
    let (_d, repo) = init_repo();
    std::fs::write(repo.join("new.ts"), "export {};").unwrap();
    git(&repo, &["add", "new.ts"]);
    let files = staged_files(&repo).await.unwrap();
    assert!(
        files
            .iter()
            .any(|f| f.file_name().is_some_and(|n| n == "new.ts")),
        "staged files should include new.ts"
    );
}

#[tokio::test]
async fn unstaged_files_after_modify() {
    let (_d, repo) = init_repo();
    std::fs::write(repo.join("README.md"), "modified").unwrap();
    let files = unstaged_files(&repo).await.unwrap();
    assert!(
        files
            .iter()
            .any(|f| f.file_name().is_some_and(|n| n == "README.md")),
        "unstaged files should include modified README.md"
    );
}

#[tokio::test]
async fn all_files_returns_tracked() {
    let (_d, repo) = init_repo();
    let files = all_files(&repo).await.unwrap();
    assert!(
        files
            .iter()
            .any(|f| f.file_name().is_some_and(|n| n == "README.md")),
        "all_files should include committed README.md"
    );
}

#[tokio::test]
async fn staged_files_empty_on_clean_index() {
    let (_d, repo) = init_repo();
    let files = staged_files(&repo).await.unwrap();
    assert!(files.is_empty(), "clean repo should have no staged files");
}

// ---------------------------------------------------------------------------
// StashGuard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stash_guard_clean_tree_is_noop() {
    let (_d, repo) = init_repo();
    let guard = StashGuard::push(&repo).await.unwrap();
    assert!(!guard.created());
    guard.pop().await.unwrap();
}

#[tokio::test]
async fn stash_guard_stashes_untracked_file() {
    let (_d, repo) = init_repo();
    std::fs::write(repo.join("scratch.log"), "secret").unwrap();

    let guard = StashGuard::push(&repo).await.unwrap();
    assert!(guard.created());
    assert!(!repo.join("scratch.log").exists(), "should be stashed");

    guard.pop().await.unwrap();
    assert!(repo.join("scratch.log").exists(), "should be restored");
    assert_eq!(
        std::fs::read_to_string(repo.join("scratch.log")).unwrap(),
        "secret"
    );
}

#[tokio::test]
async fn stash_guard_message_contains_betterhook() {
    let (_d, repo) = init_repo();
    std::fs::write(repo.join("tmp.log"), "x").unwrap();
    let guard = StashGuard::push(&repo).await.unwrap();
    assert!(
        guard.message().contains("betterhook"),
        "stash message should identify betterhook"
    );
    guard.pop().await.unwrap();
}

// ---------------------------------------------------------------------------
// StashGuard safety — stash poisoning
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stash_guard_message_matches_expected_format() {
    let (_d, repo) = init_repo();
    std::fs::write(repo.join("tmp.log"), "x").unwrap();
    let guard = StashGuard::push(&repo).await.unwrap();
    let msg = guard.message();
    let re = regex::Regex::new(r"^betterhook-stash-\d+-\d+$").unwrap();
    assert!(re.is_match(msg), "message '{msg}' should match betterhook-stash-<pid>-<nanos>");
    guard.pop().await.unwrap();
}

#[tokio::test]
async fn stash_displaced_by_external_push_refuses_to_pop() {
    let (_d, repo) = init_repo();
    std::fs::write(repo.join("scratch.log"), "data").unwrap();
    let guard = StashGuard::push(&repo).await.unwrap();
    assert!(guard.created());

    // Push another stash on top to displace ours to stash@{1}.
    std::fs::write(repo.join("another.txt"), "interloper").unwrap();
    git(&repo, &["add", "another.txt"]);
    git(&repo, &["stash", "push", "--message", "external-stash"]);

    let err = guard.pop().await.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("expected top"),
        "should refuse to pop displaced stash, got: {msg}"
    );

    // Clean up: pop both stashes manually.
    git(&repo, &["stash", "pop"]);
    git(&repo, &["stash", "pop"]);
}

#[tokio::test]
async fn stash_disappeared_refuses_to_pop() {
    let (_d, repo) = init_repo();
    std::fs::write(repo.join("scratch.log"), "data").unwrap();
    let guard = StashGuard::push(&repo).await.unwrap();
    assert!(guard.created());

    // Drop the stash entry from under the guard.
    git(&repo, &["stash", "drop", "stash@{0}"]);

    let err = guard.pop().await.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("disappeared"),
        "should report disappeared stash, got: {msg}"
    );
}

#[tokio::test]
async fn stash_guard_staged_only_changes() {
    let (_d, repo) = init_repo();
    std::fs::write(repo.join("README.md"), "updated content").unwrap();
    git(&repo, &["add", "README.md"]);

    let guard = StashGuard::push(&repo).await.unwrap();
    assert!(guard.created());
    guard.pop().await.unwrap();

    let content = std::fs::read_to_string(repo.join("README.md")).unwrap();
    assert_eq!(content, "updated content");
}

#[tokio::test]
async fn stash_guard_has_no_drop_cleanup() {
    let (_d, repo) = init_repo();
    std::fs::write(repo.join("tmp.log"), "data").unwrap();
    let guard = StashGuard::push(&repo).await.unwrap();
    assert!(guard.created());
    let msg = guard.message().to_owned();

    // Drop the guard without calling pop — the stash should remain.
    drop(guard);

    let output = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["stash", "list"])
        .output()
        .unwrap();
    let stash_list = String::from_utf8_lossy(&output.stdout);
    assert!(
        stash_list.contains(&msg),
        "stash should remain after guard drop (no Drop impl)"
    );

    // Manual cleanup.
    git(&repo, &["stash", "drop", "stash@{0}"]);
}

// ---------------------------------------------------------------------------
// rev-parse error paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_git_nonzero_exit_returns_error() {
    let (_d, repo) = init_repo();
    let err = run_git(&repo, ["rev-parse", "--verify", "refs/heads/nonexistent"]).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("exited with status"), "got: {msg}");
}

#[tokio::test]
async fn run_git_bad_cwd_returns_spawn_or_nonzero() {
    let result = run_git(
        std::path::Path::new("/nonexistent/path/that/does/not/exist"),
        ["status"],
    )
    .await;
    assert!(result.is_err(), "should fail with bad cwd");
}

// ---------------------------------------------------------------------------
// Fileset template helpers
// ---------------------------------------------------------------------------

#[test]
fn has_template_detects_staged_files() {
    assert!(has_template("eslint {staged_files}"));
}

#[test]
fn has_template_detects_files() {
    assert!(has_template("eslint {files}"));
}

#[test]
fn has_template_rejects_plain_command() {
    assert!(!has_template("eslint ."));
}

#[test]
fn expand_template_replaces_files() {
    let cmd = "eslint {files}";
    let files = vec![
        std::path::PathBuf::from("a.ts"),
        std::path::PathBuf::from("b.ts"),
    ];
    let expanded = expand_template(cmd, &files);
    let joined = expanded.join(" ");
    assert!(joined.contains("a.ts"));
    assert!(joined.contains("b.ts"));
    assert!(!joined.contains("{files}"));
}

#[test]
fn expand_template_no_placeholder_returns_original() {
    let cmd = "eslint .";
    let files = vec![std::path::PathBuf::from("a.ts")];
    let expanded = expand_template(cmd, &files);
    assert_eq!(expanded[0], "eslint .");
}

#[test]
fn shell_escape_plain_string_unchanged() {
    assert_eq!(shell_escape("hello"), "hello");
}

#[test]
fn shell_escape_quotes_spaces() {
    let escaped = shell_escape("path with spaces");
    assert!(escaped.contains('\'') || escaped.contains('"') || escaped.contains('\\'));
}

// ---------------------------------------------------------------------------
// Glob filtering
// ---------------------------------------------------------------------------

#[test]
fn build_globset_empty_returns_none() {
    let gs = build_globset(&[]).unwrap();
    assert!(gs.is_none());
}

#[test]
fn build_globset_valid_patterns() {
    let gs = build_globset(&["*.ts".to_owned(), "*.tsx".to_owned()]).unwrap();
    assert!(gs.is_some());
}

#[test]
fn filter_files_matches_glob() {
    let files = vec![
        std::path::PathBuf::from("a.ts"),
        std::path::PathBuf::from("b.rs"),
        std::path::PathBuf::from("c.tsx"),
    ];
    let include = build_globset(&["*.ts".to_owned(), "*.tsx".to_owned()]).unwrap();
    let filtered = filter_files(files, include.as_ref(), None);
    assert_eq!(filtered.len(), 2);
}

#[test]
fn filter_files_excludes_patterns() {
    let files = vec![
        std::path::PathBuf::from("a.ts"),
        std::path::PathBuf::from("b.d.ts"),
    ];
    let include = build_globset(&["*.ts".to_owned()]).unwrap();
    let exclude = build_globset(&["*.d.ts".to_owned()]).unwrap();
    let filtered = filter_files(files, include.as_ref(), exclude.as_ref());
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].file_name().unwrap(), "a.ts");
}

#[test]
fn filter_files_no_glob_returns_all() {
    let files = vec![
        std::path::PathBuf::from("a.ts"),
        std::path::PathBuf::from("b.rs"),
    ];
    let filtered = filter_files(files, None, None);
    assert_eq!(filtered.len(), 2);
}
