//! `betterhook doctor` — pre-flight health check.
//!
//! Walks a matrix of checks every betterhook install should pass and
//! returns a JSON report. Exit code is non-zero if any check fails so
//! shell scripts and CI can gate on the output.
//!
//! Not every check blocks a commit; some (launchd unit, cache writable)
//! are warnings that degrade functionality but don't break the core
//! `run-hook → dispatch → execute` path.

use std::path::{Path, PathBuf};

use betterhook::builtins;
use tokio::process::Command;
use betterhook::daemon::speculative::read_stats;
use betterhook::git::{git_common_dir, show_toplevel};
use betterhook::install::{InstalledManifest, MANIFEST_FILENAME};
use miette::{IntoDiagnostic, miette};
use serde::Serialize;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Worktree to inspect. Defaults to the current directory.
    #[arg(long)]
    pub worktree: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

pub async fn run(args: Args) -> miette::Result<()> {
    let worktree = args
        .worktree
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    let toplevel = show_toplevel(&worktree)
        .await
        .map_err(|e| miette!("not inside a git worktree: {e}"))?;
    let common_dir = git_common_dir(&toplevel).await.map_err(|e| miette!("{e}"))?;

    let checks: Vec<Check> = vec![
        check_installed(&common_dir),
        check_config(&toplevel),
        check_builtin_tools(&toplevel),
        check_cache_writable(&common_dir),
        check_watcher(&common_dir),
        check_orphan_stashes(&toplevel).await,
        check_core_hookspath(&toplevel).await,
    ];

    let ok = checks
        .iter()
        .all(|c| !matches!(c.status, Status::Fail));
    let payload = serde_json::json!({
        "ok": ok,
        "worktree": toplevel.display().to_string(),
        "checks": checks,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).into_diagnostic()?
    );
    if !ok {
        std::process::exit(crate::exit_codes::GIT_ERROR);
    }
    Ok(())
}

fn check_installed(common_dir: &Path) -> Check {
    let manifest_path = common_dir.join("betterhook").join(MANIFEST_FILENAME);
    let Ok(bytes) = std::fs::read(&manifest_path) else {
        return Check {
            name: "installed",
            status: Status::Warn,
            detail: format!(
                "no manifest at {} — run `betterhook install`",
                manifest_path.display()
            ),
        };
    };
    let Ok(manifest) = serde_json::from_slice::<InstalledManifest>(&bytes) else {
        return Check {
            name: "installed",
            status: Status::Fail,
            detail: format!("corrupt manifest at {}", manifest_path.display()),
        };
    };
    let hooks_dir = common_dir.join("hooks");
    let mut mismatches = Vec::new();
    for (name, expected) in &manifest.hooks {
        let target = hooks_dir.join(name);
        match std::fs::read(&target) {
            Ok(bytes) => {
                let actual = betterhook::install::sha256_hex(&bytes);
                if &actual != expected {
                    mismatches.push(name.clone());
                }
            }
            Err(_) => mismatches.push(name.clone()),
        }
    }
    if mismatches.is_empty() {
        Check {
            name: "installed",
            status: Status::Pass,
            detail: format!("{} hook(s) match the manifest", manifest.hooks.len()),
        }
    } else {
        Check {
            name: "installed",
            status: Status::Fail,
            detail: format!("wrapper drift in {mismatches:?}; re-run `betterhook install`"),
        }
    }
}

fn check_config(worktree: &Path) -> Check {
    let Some(path) = betterhook::dispatch::find_config(worktree) else {
        return Check {
            name: "config",
            status: Status::Warn,
            detail: "no betterhook.{toml,yaml,kdl,json} in repo".to_owned(),
        };
    };
    match betterhook::config::load(&path) {
        Ok(_) => Check {
            name: "config",
            status: Status::Pass,
            detail: format!("parsed {}", path.display()),
        },
        Err(e) => Check {
            name: "config",
            status: Status::Fail,
            detail: format!("parse error at {}: {e}", path.display()),
        },
    }
}

fn check_builtin_tools(worktree: &Path) -> Check {
    let Some(path) = betterhook::dispatch::find_config(worktree) else {
        return Check {
            name: "builtin_tools",
            status: Status::Pass,
            detail: "no config — nothing to probe".to_owned(),
        };
    };
    let Ok(_config) = betterhook::config::load(&path) else {
        return Check {
            name: "builtin_tools",
            status: Status::Warn,
            detail: "config did not parse; skipping tool probe".to_owned(),
        };
    };
    // For now probe every registered builtin's tool binary. Future work
    // can narrow this to only builtins actually referenced in the user's
    // config once `Job::builtin` is wired through.
    let mut missing = Vec::new();
    for meta in betterhook::builtins::registry().values() {
        if which::which(meta.tool_binary).is_err() {
            missing.push(meta.tool_binary);
        }
    }
    if missing.is_empty() {
        Check {
            name: "builtin_tools",
            status: Status::Pass,
            detail: format!(
                "every builtin tool is on PATH ({} registered)",
                builtins::registry().len()
            ),
        }
    } else {
        Check {
            name: "builtin_tools",
            status: Status::Warn,
            detail: format!("missing on PATH: {missing:?}"),
        }
    }
}

fn check_cache_writable(common_dir: &Path) -> Check {
    let cache_dir = betterhook::cache::cache_dir(common_dir);
    if std::fs::create_dir_all(&cache_dir).is_err() {
        return Check {
            name: "cache_writable",
            status: Status::Fail,
            detail: format!("cannot create {}", cache_dir.display()),
        };
    }
    let probe = cache_dir.join(".doctor-probe");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Check {
                name: "cache_writable",
                status: Status::Pass,
                detail: format!("wrote to {}", cache_dir.display()),
            }
        }
        Err(e) => Check {
            name: "cache_writable",
            status: Status::Fail,
            detail: format!("write failed at {}: {e}", cache_dir.display()),
        },
    }
}

fn check_watcher(common_dir: &Path) -> Check {
    match read_stats(common_dir) {
        Some(stats) if stats.disabled_reason.is_none() => Check {
            name: "watcher",
            status: Status::Pass,
            detail: format!(
                "{} worktree(s), {} watches",
                stats.watched_worktrees, stats.watch_count
            ),
        },
        Some(stats) => Check {
            name: "watcher",
            status: Status::Warn,
            detail: stats
                .disabled_reason
                .unwrap_or_else(|| "watcher disabled".to_owned()),
        },
        None => Check {
            name: "watcher",
            status: Status::Warn,
            detail: "no speculative-stats sidecar yet — daemon not running".to_owned(),
        },
    }
}

async fn check_orphan_stashes(worktree: &Path) -> Check {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["stash", "list"])
        .output()
        .await;
    let Ok(out) = out else {
        return Check {
            name: "orphan_stashes",
            status: Status::Warn,
            detail: "git stash list failed".to_owned(),
        };
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let orphans: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("betterhook"))
        .collect();
    if orphans.is_empty() {
        Check {
            name: "orphan_stashes",
            status: Status::Pass,
            detail: "no betterhook stashes lingering".to_owned(),
        }
    } else {
        Check {
            name: "orphan_stashes",
            status: Status::Warn,
            detail: format!(
                "{} orphan stash(es); `git stash drop` to clean up",
                orphans.len()
            ),
        }
    }
}

async fn check_core_hookspath(worktree: &Path) -> Check {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["config", "--get", "core.hooksPath"])
        .output()
        .await;
    let Ok(out) = out else {
        return Check {
            name: "core_hookspath",
            status: Status::Pass,
            detail: "no conflicting core.hooksPath".to_owned(),
        };
    };
    let value = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if value.is_empty() {
        Check {
            name: "core_hookspath",
            status: Status::Pass,
            detail: "unset".to_owned(),
        }
    } else {
        Check {
            name: "core_hookspath",
            status: Status::Warn,
            detail: format!(
                "core.hooksPath={value} — betterhook writes to `$GIT_DIR/hooks` \
                 and will be bypassed unless you unset this"
            ),
        }
    }
}
