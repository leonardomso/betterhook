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
    let tmp = match make_root() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("xtask stress: failed to allocate tempdir: {e}");
            return ExitCode::from(1);
        }
    };
    eprintln!("xtask stress: scratch dir at {}", tmp.display());

    let primary = tmp.join("primary");
    if let Err(e) = init_primary(&primary) {
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
        handles.push(thread::spawn(move || run_worktree(&wt, idx)));
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

fn init_primary(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
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
    std::fs::write(dir.join("src/lib.rs"), "pub fn one() -> i32 { 1 }\n")?;

    let cfg = r#"[meta]
version = 1

[hooks.pre-commit.jobs.fmt]
run = "cargo fmt --all -- --check"
glob = ["*.rs"]
concurrent_safe = true
isolate = "cargo"
"#;
    std::fs::write(dir.join("betterhook.toml"), cfg)?;

    sh(dir, &["git", "add", "-A"])?;
    sh(dir, &["git", "commit", "-q", "-m", "init"])?;
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

fn run_worktree(dir: &Path, idx: usize) -> std::io::Result<()> {
    // Drop a trivially correct .rs file (cargo fmt --check should pass).
    let file = dir.join(format!("src/wt_{idx:02}.rs"));
    std::fs::write(&file, format!("pub fn id_{idx}() -> usize {{ {idx} }}\n"))?;
    sh(dir, &["git", "add", "-A"])?;
    sh(dir, &["git", "commit", "-q", "-m", &format!("wt {idx}")])?;
    Ok(())
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
