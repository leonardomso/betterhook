//! Thin lock client used by the runner.
//!
//! Phase 15 ships the fallback half: every lock acquisition goes
//! through `FileLock` advisory flock on
//! `<common-dir>/betterhook/locks/<key>.lock`. The daemon-backed
//! socket path is phase 16+ — this file already handles the daemon
//! being unavailable, which is the common case until the runner
//! starts spawning one.
//!
//! The runner uses this via [`acquire_job_lock`], which takes a
//! [`crate::config::IsolateSpec`], converts it to a key, and returns
//! a [`LockGuard`] holding the flock for the duration of the job.

use std::path::{Path, PathBuf};

use crate::config::{IsolateSpec, ToolPathScope};

use super::flock::FileLock;

/// A held lock. Drops release whichever backend acquired it.
#[derive(Debug)]
pub struct LockGuard {
    _file: Option<FileLock>,
    /// For `ToolPathScope::PerWorktree` isolation the runner also
    /// injects the right env var (e.g. `CARGO_TARGET_DIR`) so
    /// concurrent builds don't collide.
    pub extra_env: Vec<(String, String)>,
}

/// Convert an `IsolateSpec` into the lock key the daemon / flock use.
///
/// Returns `Some((key, permits, extra_env))` for every variant that
/// needs coordination. Returns `None` for specs that use purely env-
/// var-based isolation and don't need a mutual-exclusion primitive.
#[must_use]
pub fn key_for_spec(spec: &IsolateSpec, worktree: &Path) -> (String, u32, Vec<(String, String)>) {
    match spec {
        IsolateSpec::Tool { name } => (format!("tool:{name}"), 1, Vec::new()),
        IsolateSpec::Sharded { name, slots } => (
            format!("sharded:{name}"),
            u32::try_from(*slots).unwrap_or(1),
            Vec::new(),
        ),
        IsolateSpec::ToolPath { tool, target_dir } => {
            let (key_suffix, env) = match target_dir {
                ToolPathScope::PerWorktree => {
                    let wt = worktree.display();
                    let key = format!("tool-path:{tool}:{wt}");
                    // Per-tool known env-var injection. Extend over time.
                    let env = match tool.as_str() {
                        "cargo" => vec![(
                            "CARGO_TARGET_DIR".to_owned(),
                            worktree.join("target").display().to_string(),
                        )],
                        _ => Vec::new(),
                    };
                    (key, env)
                }
                ToolPathScope::Path(p) => (format!("tool-path:{tool}:{}", p.display()), Vec::new()),
            };
            // Per-worktree keys effectively mean "one per path": permit
            // count is always 1, but two different worktrees have
            // different keys so they never contend.
            (key_suffix, 1, env)
        }
    }
}

/// Acquire a lock for this job. Phase 15 goes straight to flock; a
/// later phase adds a daemon fast path.
pub fn acquire_job_lock(
    common_dir: &Path,
    spec: &IsolateSpec,
    worktree: &Path,
) -> std::io::Result<LockGuard> {
    let (key, _permits, extra_env) = key_for_spec(spec, worktree);
    let file = FileLock::acquire(common_dir, &key)?;
    Ok(LockGuard {
        _file: Some(file),
        extra_env,
    })
}

/// Return the path under which advisory lockfiles live, for testing
/// and `betterhook status` introspection.
#[must_use]
pub fn lock_dir(common_dir: &Path) -> PathBuf {
    common_dir.join("betterhook").join("locks")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ToolPathScope;
    use std::path::PathBuf;

    #[test]
    fn key_for_tool_mutex() {
        let (k, permits, env) = key_for_spec(
            &IsolateSpec::Tool {
                name: "eslint".to_owned(),
            },
            Path::new("/tmp/wt"),
        );
        assert_eq!(k, "tool:eslint");
        assert_eq!(permits, 1);
        assert!(env.is_empty());
    }

    #[test]
    fn key_for_sharded() {
        let (k, permits, _env) = key_for_spec(
            &IsolateSpec::Sharded {
                name: "tsc".to_owned(),
                slots: 4,
            },
            Path::new("/tmp/wt"),
        );
        assert_eq!(k, "sharded:tsc");
        assert_eq!(permits, 4);
    }

    #[test]
    fn key_for_cargo_injects_target_dir() {
        let (_k, _p, env) = key_for_spec(
            &IsolateSpec::ToolPath {
                tool: "cargo".to_owned(),
                target_dir: ToolPathScope::PerWorktree,
            },
            Path::new("/tmp/wt-a"),
        );
        assert_eq!(
            env,
            vec![("CARGO_TARGET_DIR".to_owned(), "/tmp/wt-a/target".to_owned())]
        );
    }

    #[test]
    fn per_worktree_keys_are_distinct() {
        let spec = IsolateSpec::ToolPath {
            tool: "cargo".to_owned(),
            target_dir: ToolPathScope::PerWorktree,
        };
        let (k1, _, _) = key_for_spec(&spec, Path::new("/tmp/wt-a"));
        let (k2, _, _) = key_for_spec(&spec, Path::new("/tmp/wt-b"));
        assert_ne!(k1, k2);
    }

    // Unused import guard.
    #[allow(dead_code)]
    fn _path_buf_sanity(_: PathBuf) {}
}
