//! `xtask stress` — the 30-second 8-worktree cargo fmt race demo.
//!
//! Creates a fresh repo, attaches eight linked worktrees, drops a
//! trivially unformatted `.rs` file in each, and commits all of them
//! concurrently with `isolate.tool = "cargo"` so the betterhook
//! coordinator daemon serializes the formatter runs across worktrees.
//!
//! The harness asserts:
//! - every commit lands cleanly (zero corruption),
//! - the formatter ran the expected number of times,
//! - peak elapsed time stays under a soft budget (informational, not
//!   a hard gate, since CI runners vary).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const WORKTREE_COUNT: usize = 8;

pub fn run(_args: &[String]) -> ExitCode {
    let betterhook = match ensure_betterhook_binary() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("xtask stress: failed to prepare betterhook binary: {e}");
            return ExitCode::from(1);
        }
    };
    let tmp = match make_root() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("xtask stress: failed to allocate tempdir: {e}");
            return ExitCode::from(1);
        }
    };
    eprintln!("xtask stress: scratch dir at {}", tmp.display());

    let primary = tmp.join("primary");
    let marker_dir = tmp.join("markers");
    if let Err(e) = init_primary(&primary, &betterhook, &marker_dir) {
        eprintln!("xtask stress: init_primary failed: {e}");
        return ExitCode::from(1);
    }

    let worktrees = match attach_worktrees(&primary, WORKTREE_COUNT) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("xtask stress: attach_worktrees failed: {e}");
            return ExitCode::from(1);
        }
    };

    let start = Instant::now();
    let mut handles = Vec::with_capacity(worktrees.len());
    for (idx, wt) in worktrees.iter().cloned().enumerate() {
        let markers = marker_dir.clone();
        handles.push(thread::spawn(move || run_worktree(&wt, idx, &markers)));
    }

    let mut failures = 0usize;
    for h in handles {
        match h.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                eprintln!("xtask stress: worktree run failed: {e}");
                failures += 1;
            }
            Err(_) => {
                eprintln!("xtask stress: worker thread panicked");
                failures += 1;
            }
        }
    }
    let elapsed = start.elapsed();

    eprintln!(
        "xtask stress: {WORKTREE_COUNT} worktrees finished in {} ms",
        elapsed.as_millis()
    );
    if failures > 0 {
        eprintln!("xtask stress: {failures} worktree(s) failed");
        return ExitCode::from(1);
    }
    if let Err(e) = verify_markers(&worktrees, &marker_dir) {
        eprintln!("xtask stress: hook verification failed: {e}");
        return ExitCode::from(1);
    }
    if elapsed > Duration::from_secs(60) {
        eprintln!("xtask stress: WARN — exceeded 60s soft budget");
    }
    ExitCode::SUCCESS
}

fn make_root() -> std::io::Result<PathBuf> {
    let base = std::env::temp_dir().join(format!("betterhook-stress-{}", std::process::id()));
    if base.exists() {
        std::fs::remove_dir_all(&base)?;
    }
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

fn init_primary(dir: &Path, betterhook: &Path, marker_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::create_dir_all(marker_dir)?;
    sh(dir, &["git", "init", "-q"])?;
    sh(dir, &["git", "config", "user.email", "stress@betterhook"])?;
    sh(dir, &["git", "config", "user.name", "stress"])?;

    // Minimal Cargo.toml + lib.rs so `cargo fmt` actually has something
    // to act on.
    std::fs::write(
        dir.join("Cargo.toml"),
        r#"[package]
name = "stress"
version = "0.0.1"
edition = "2024"
"#,
    )?;
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn one() -> i32 {\n    1\n}\n",
    )?;

    std::fs::write(dir.join("betterhook.toml"), stress_config(marker_dir))?;

    sh(dir, &["git", "add", "-A"])?;
    sh(dir, &["git", "commit", "-q", "-m", "init"])?;
    let status = Command::new(betterhook)
        .args(["install", "--no-unit"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "betterhook install failed with {status}"
        )));
    }
    Ok(())
}

fn attach_worktrees(primary: &Path, count: usize) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::with_capacity(count);
    let parent = primary
        .parent()
        .expect("primary always has a parent (the scratch root)");
    for i in 0..count {
        let wt = parent.join(format!("wt-{i}"));
        sh(
            primary,
            &[
                "git",
                "worktree",
                "add",
                "-b",
                &format!("stress-{i}"),
                wt.to_str().unwrap(),
            ],
        )?;
        out.push(wt);
    }
    Ok(out)
}

fn run_worktree(dir: &Path, idx: usize, marker_dir: &Path) -> std::io::Result<()> {
    // Drop a trivially correct .rs file (cargo fmt --check should pass).
    let file = dir.join(format!("src/wt_{idx:02}.rs"));
    std::fs::write(&file, format!("pub fn id_{idx}() -> usize {{ {idx} }}\n"))?;
    sh(dir, &["git", "add", "-A"])?;
    sh(dir, &["git", "commit", "-q", "-m", &format!("wt {idx}")])?;
    let marker = marker_file(marker_dir, dir)?;
    if !marker.is_file() {
        return Err(std::io::Error::other(format!(
            "expected hook marker at {}",
            marker.display()
        )));
    }
    Ok(())
}

fn verify_markers(worktrees: &[PathBuf], marker_dir: &Path) -> std::io::Result<()> {
    for wt in worktrees {
        let marker = marker_file(marker_dir, wt)?;
        if !marker.is_file() {
            return Err(std::io::Error::other(format!(
                "missing hook marker for {}",
                wt.display()
            )));
        }
    }
    Ok(())
}

fn marker_file(marker_dir: &Path, worktree: &Path) -> std::io::Result<PathBuf> {
    let Some(name) = worktree.file_name() else {
        return Err(std::io::Error::other(format!(
            "worktree {} has no basename",
            worktree.display()
        )));
    };
    Ok(marker_dir.join(format!("{}.ran", name.to_string_lossy())))
}

fn stress_config(marker_dir: &Path) -> String {
    format!(
        r#"[meta]
version = 1

[hooks.pre-commit.jobs.fmt]
run = "sh -c 'cargo fmt --all -- --check && touch \"$MARKER_DIR/$(basename \"$PWD\").ran\"'"
glob = ["*.rs"]
concurrent_safe = true
isolate = "cargo"
env = {{ MARKER_DIR = "{}" }}
"#,
        marker_dir.display()
    )
}

fn ensure_betterhook_binary() -> std::io::Result<PathBuf> {
    let root = workspace_root();
    let status = Command::new("cargo")
        .args(["build", "-q", "-p", "betterhook-cli"])
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "cargo build -p betterhook-cli failed with {status}"
        )));
    }
    let bin = root.join("target").join("debug").join("betterhook");
    if !bin.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("missing betterhook binary at {}", bin.display()),
        ));
    }
    Ok(bin)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest lives under the workspace root")
        .to_path_buf()
}

fn sh(dir: &Path, argv: &[&str]) -> std::io::Result<()> {
    let status = Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "{argv:?} failed with {status}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_file_uses_worktree_basename() {
        let marker = marker_file(Path::new("/tmp/markers"), Path::new("/tmp/wt-3")).unwrap();
        assert_eq!(marker, PathBuf::from("/tmp/markers/wt-3.ran"));
    }

    #[test]
    fn stress_config_keeps_cargo_and_marker_command() {
        let cfg = stress_config(Path::new("/tmp/markers"));
        assert!(cfg.contains("cargo fmt --all -- --check"));
        assert!(cfg.contains("MARKER_DIR = \"/tmp/markers\""));
        assert!(cfg.contains("touch \\\"$MARKER_DIR/$(basename \\\"$PWD\\\").ran\\\""));
    }
}
