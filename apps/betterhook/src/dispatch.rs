//! Runtime resolution for the `betterhook __dispatch` subcommand — the
//! internal target the wrapper script exec's into on every hook fire.
//!
//! The wrapper script lives in the shared `<common-dir>/hooks/` dir and
//! calls us with `--hook <name> --worktree <path>`. This module is
//! responsible for finding the right config file for that worktree,
//! lowering it, and returning the jobs to execute. It exits 0 silently
//! on any "soft" miss (no config file, config without that hook, hook
//! with no jobs) — this is the agent-friendly default so a worktree
//! that doesn't opt into betterhook never blocks a commit.

use std::path::{Path, PathBuf};

use crate::config::{self, Config, Hook};
use crate::error::ConfigResult;

/// Candidate config filenames, in lookup order. First match wins.
pub const CONFIG_CANDIDATES: &[&str] = &[
    "betterhook.toml",
    "betterhook.yml",
    "betterhook.yaml",
    "betterhook.json",
    "betterhook.kdl",
];

/// Find the first `betterhook.*` config file in `worktree`. Returns
/// `None` if no candidate exists.
#[must_use]
pub fn find_config(worktree: &Path) -> Option<PathBuf> {
    for name in CONFIG_CANDIDATES {
        let candidate = worktree.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Outcome of resolving a dispatch invocation.
pub enum Dispatch {
    /// No config file found — this worktree hasn't opted into betterhook.
    NoConfig,
    /// Config exists but has no definition for this hook type.
    HookNotConfigured,
    /// Config exists and has the hook, but with zero jobs.
    NoJobs,
    /// Ready to execute.
    Run { config: Config, hook_name: String },
}

impl Dispatch {
    /// Return a reference to the matching `Hook` when `Run`, else `None`.
    #[must_use]
    pub fn hook(&self) -> Option<&Hook> {
        match self {
            Self::Run { config, hook_name } => config.hooks.get(hook_name),
            _ => None,
        }
    }

    /// True when this dispatch should take no action and exit 0.
    #[must_use]
    pub const fn is_noop(&self) -> bool {
        matches!(
            self,
            Self::NoConfig | Self::HookNotConfigured | Self::NoJobs
        )
    }
}

/// Resolve a dispatch invocation for `worktree` + `hook_name`. Soft
/// misses (no config, hook not configured, hook with empty jobs) are
/// returned as non-`Run` variants; only true parse errors propagate.
pub fn resolve(worktree: &Path, hook_name: &str) -> ConfigResult<Dispatch> {
    let Some(config_path) = find_config(worktree) else {
        return Ok(Dispatch::NoConfig);
    };
    let config = config::load(&config_path)?;
    let Some(hook) = config.hooks.get(hook_name) else {
        return Ok(Dispatch::HookNotConfigured);
    };
    if hook.jobs.is_empty() {
        return Ok(Dispatch::NoJobs);
    }
    Ok(Dispatch::Run {
        config,
        hook_name: hook_name.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn find_config_returns_none_when_absent() {
        let dir = tempdir().unwrap();
        assert!(find_config(dir.path()).is_none());
    }

    #[test]
    fn find_config_prefers_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("betterhook.toml"), "[meta]\nversion=1\n").unwrap();
        std::fs::write(dir.path().join("betterhook.yml"), "meta:\n  version: 1\n").unwrap();
        let found = find_config(dir.path()).unwrap();
        assert!(found.ends_with("betterhook.toml"));
    }

    #[test]
    fn resolve_no_config_is_soft_miss() {
        let dir = tempdir().unwrap();
        let out = resolve(dir.path(), "pre-commit").unwrap();
        assert!(matches!(out, Dispatch::NoConfig));
        assert!(out.is_noop());
    }

    #[test]
    fn resolve_hook_not_configured_is_soft_miss() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("betterhook.toml"),
            "[hooks.pre-commit.jobs.a]\nrun = \"true\"\n",
        )
        .unwrap();
        let out = resolve(dir.path(), "pre-push").unwrap();
        assert!(matches!(out, Dispatch::HookNotConfigured));
        assert!(out.is_noop());
    }

    #[test]
    fn resolve_run_populates_config() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("betterhook.toml"),
            "[hooks.pre-commit.jobs.a]\nrun = \"true\"\n",
        )
        .unwrap();
        let out = resolve(dir.path(), "pre-commit").unwrap();
        match out {
            Dispatch::Run { hook_name, .. } => assert_eq!(hook_name, "pre-commit"),
            _ => panic!("expected Dispatch::Run"),
        }
    }
}
