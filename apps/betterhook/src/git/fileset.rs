//! File-set computation and template expansion.
//!
//! Every git call here uses `-z` so output is NUL-delimited — this
//! sidesteps lefthook's quoting bugs with spaces, unicode, and leading
//! dashes. Outputs are parsed via `OsStr::from_bytes` so non-UTF-8
//! filenames round-trip cleanly on macOS and Linux.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use super::rev_parse::{GitResult, run_git};

/// Conservative upper bound on bytes we'll cram into a single
/// template-substituted command. macOS `ARG_MAX` is 256 KB; we aim for
/// half to leave headroom for env vars and the surrounding shell.
pub const MAX_ARG_BYTES: usize = 128 * 1024;

/// Run `git diff --name-only --cached -z` and return the staged paths.
pub async fn staged_files(cwd: &Path) -> GitResult<Vec<PathBuf>> {
    let out = run_git(cwd, ["diff", "--name-only", "--cached", "-z"]).await?;
    Ok(parse_nul_delimited(&out))
}

/// Run `git ls-files -z` and return every tracked path.
pub async fn all_files(cwd: &Path) -> GitResult<Vec<PathBuf>> {
    let out = run_git(cwd, ["ls-files", "-z"]).await?;
    Ok(parse_nul_delimited(&out))
}

/// Run `git diff --name-only -z <base>...HEAD` and return the changed
/// paths relative to `base_ref`. Used by pre-push hooks.
pub async fn push_files(cwd: &Path, base_ref: &str) -> GitResult<Vec<PathBuf>> {
    let range = format!("{base_ref}...HEAD");
    let out = run_git(cwd, ["diff", "--name-only", "-z", &range]).await?;
    Ok(parse_nul_delimited(&out))
}

/// Snapshot unstaged but tracked file changes — used by `stage_fixed`
/// to compute which files a job touched during its run.
pub async fn unstaged_files(cwd: &Path) -> GitResult<Vec<PathBuf>> {
    let out = run_git(cwd, ["diff", "--name-only", "-z"]).await?;
    Ok(parse_nul_delimited(&out))
}

fn parse_nul_delimited(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(OsStr::from_bytes(s)))
        .collect()
}

/// Build a `GlobSet` from a list of patterns. An empty pattern list
/// returns `None`, meaning "match everything".
pub fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>, globset::Error> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for pat in patterns {
        b.add(Glob::new(pat)?);
    }
    Ok(Some(b.build()?))
}

/// Apply include/exclude globs to a file list.
#[must_use]
pub fn filter_files(
    files: Vec<PathBuf>,
    include: Option<&GlobSet>,
    exclude: Option<&GlobSet>,
) -> Vec<PathBuf> {
    files
        .into_iter()
        .filter(|p| include.is_none_or(|g| g.is_match(p)))
        .filter(|p| exclude.is_none_or(|g| !g.is_match(p)))
        .collect()
}

/// POSIX-safe shell escaping. Returns the original string if it only
/// contains characters that don't need quoting, otherwise wraps it in
/// single quotes and escapes any embedded single quote.
#[must_use]
pub fn shell_escape(s: &str) -> String {
    let safe = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '_' | '-' | ':' | '@' | '+'));
    if safe && !s.is_empty() {
        s.to_string()
    } else {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('\'');
        for c in s.chars() {
            if c == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(c);
            }
        }
        out.push('\'');
        out
    }
}

/// The four template variables we recognize.
pub const TEMPLATE_VARS: &[&str] = &["{staged_files}", "{push_files}", "{all_files}", "{files}"];

/// Return `true` if `command` references any of the fileset templates.
#[must_use]
pub fn has_template(command: &str) -> bool {
    TEMPLATE_VARS.iter().any(|v| command.contains(v))
}

fn substitute_once(command: &str, expansion: &str) -> String {
    let mut out = command.to_string();
    for v in TEMPLATE_VARS {
        if out.contains(v) {
            out = out.replace(v, expansion);
        }
    }
    out
}

/// Expand a template, splitting the file list across multiple command
/// invocations if it would otherwise blow through `ARG_MAX`.
///
/// Returns at least one command. An empty `files` list and a template
/// produce a single command with the template replaced by the empty
/// string — callers that want to skip empty runs should check beforehand.
#[must_use]
pub fn expand_template(command: &str, files: &[PathBuf]) -> Vec<String> {
    if !has_template(command) {
        return vec![command.to_string()];
    }
    if files.is_empty() {
        return vec![substitute_once(command, "")];
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_bytes: usize = 0;
    let fixed_overhead = command.len();

    for f in files {
        let escaped = shell_escape(&f.to_string_lossy());
        let delta = escaped.len() + 1; // +1 for the joining space
        if !current.is_empty() && fixed_overhead + current_bytes + delta > MAX_ARG_BYTES {
            chunks.push(substitute_once(command, &current.join(" ")));
            current.clear();
            current_bytes = 0;
        }
        current.push(escaped);
        current_bytes += delta;
    }
    if !current.is_empty() {
        chunks.push(substitute_once(command, &current.join(" ")));
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;
    use tempfile::TempDir;

    fn new_git_repo_with_files() -> (TempDir, PathBuf) {
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
            assert!(s.success(), "git {args:?} failed");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t.t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.ts"), "1\n").unwrap();
        std::fs::write(root.join("b.ts"), "2\n").unwrap();
        std::fs::write(root.join("README.md"), "hi\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        (dir, root)
    }

    #[tokio::test]
    async fn all_files_lists_tracked_entries() {
        let (_d, root) = new_git_repo_with_files();
        let files = all_files(&root).await.unwrap();
        let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        assert!(names.contains(&"a.ts".to_string()));
        assert!(names.contains(&"b.ts".to_string()));
        assert!(names.contains(&"README.md".to_string()));
    }

    #[tokio::test]
    async fn staged_files_returns_only_index() {
        let (_d, root) = new_git_repo_with_files();
        assert!(staged_files(&root).await.unwrap().is_empty());

        std::fs::write(root.join("a.ts"), "changed\n").unwrap();
        // Unstaged modification: staged_files still empty.
        assert!(staged_files(&root).await.unwrap().is_empty());

        let s = StdCommand::new("git")
            .current_dir(&root)
            .args(["add", "a.ts"])
            .status()
            .unwrap();
        assert!(s.success());
        let staged = staged_files(&root).await.unwrap();
        assert_eq!(staged, vec![PathBuf::from("a.ts")]);
    }

    #[tokio::test]
    async fn handles_filenames_with_spaces() {
        let (_d, root) = new_git_repo_with_files();
        let weird = root.join("with space.ts");
        std::fs::write(&weird, "x\n").unwrap();
        let s = StdCommand::new("git")
            .current_dir(&root)
            .args(["add", "with space.ts"])
            .status()
            .unwrap();
        assert!(s.success());
        let staged = staged_files(&root).await.unwrap();
        assert_eq!(staged, vec![PathBuf::from("with space.ts")]);
    }

    #[test]
    fn filter_with_include_and_exclude() {
        let files = vec![
            PathBuf::from("a.ts"),
            PathBuf::from("a.gen.ts"),
            PathBuf::from("b.js"),
        ];
        let include = build_globset(&["*.ts".to_string(), "*.tsx".to_string()])
            .unwrap()
            .unwrap();
        let exclude = build_globset(&["**/*.gen.ts".to_string()])
            .unwrap()
            .unwrap();
        let out = filter_files(files, Some(&include), Some(&exclude));
        assert_eq!(out, vec![PathBuf::from("a.ts")]);
    }

    #[test]
    fn shell_escape_safe_and_unsafe() {
        assert_eq!(shell_escape("plain.ts"), "plain.ts");
        assert_eq!(shell_escape("with space.ts"), "'with space.ts'");
        assert_eq!(shell_escape("it's.ts"), "'it'\\''s.ts'");
    }

    #[test]
    fn expand_template_handles_empty_and_no_template() {
        assert_eq!(expand_template("true", &[]), vec!["true".to_string()]);
        assert_eq!(
            expand_template("eslint {files}", &[]),
            vec!["eslint ".to_string()]
        );
        assert_eq!(
            expand_template("eslint {files}", &[PathBuf::from("a.ts")]),
            vec!["eslint a.ts".to_string()]
        );
    }

    #[test]
    fn expand_template_chunks_on_arg_max() {
        // 10_000 files each ~20 chars → > 128 KB; must chunk.
        let files: Vec<PathBuf> = (0..10_000)
            .map(|i| PathBuf::from(format!("path/to/file-{i:06}.ts")))
            .collect();
        let chunks = expand_template("eslint {files}", &files);
        assert!(chunks.len() >= 2, "expected chunking, got {}", chunks.len());
        for c in &chunks {
            assert!(c.len() <= MAX_ARG_BYTES, "chunk too big: {}", c.len());
            assert!(c.starts_with("eslint "));
        }
    }

    #[test]
    fn expand_template_replaces_all_aliases() {
        let files = vec![PathBuf::from("a.ts")];
        assert_eq!(
            expand_template("eslint {staged_files}", &files),
            vec!["eslint a.ts".to_string()]
        );
        assert_eq!(
            expand_template("tsc {all_files}", &files),
            vec!["tsc a.ts".to_string()]
        );
        assert_eq!(
            expand_template("check {push_files}", &files),
            vec!["check a.ts".to_string()]
        );
    }
}
