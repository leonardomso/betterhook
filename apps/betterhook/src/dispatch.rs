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

use crate::config::{self, Config, Hook, Package};
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
    let has_root_hook = config
        .hooks
        .get(hook_name)
        .is_some_and(|h| !h.jobs.is_empty());
    let has_package_hook = config
        .packages
        .values()
        .any(|p| p.hooks.get(hook_name).is_some_and(|h| !h.jobs.is_empty()));
    if !has_root_hook && !has_package_hook {
        let has_empty_hook = config.hooks.contains_key(hook_name)
            || config
                .packages
                .values()
                .any(|p| p.hooks.contains_key(hook_name));
        if has_empty_hook {
            return Ok(Dispatch::NoJobs);
        }
        return Ok(Dispatch::HookNotConfigured);
    }
    Ok(Dispatch::Run {
        config,
        hook_name: hook_name.to_owned(),
    })
}

/// Monorepo dispatch: group `staged_files` by the longest matching
/// package path prefix. `PackageMatch::Root` is the residual bucket
/// of files that didn't match any declared package.
#[derive(Debug, Clone)]
pub enum PackageMatch<'a> {
    Root(Vec<PathBuf>),
    Package(&'a Package, Vec<PathBuf>),
}

#[must_use]
pub fn resolve_packages<'a>(
    config: &'a Config,
    staged_files: &[PathBuf],
) -> Vec<PackageMatch<'a>> {
    if config.packages.is_empty() {
        return vec![PackageMatch::Root(staged_files.to_vec())];
    }

    // Longest-prefix-wins: sort package paths by length descending.
    let mut packages: Vec<&Package> = config.packages.values().collect();
    packages.sort_by_key(|p| std::cmp::Reverse(p.path.as_os_str().len()));

    let mut buckets: std::collections::BTreeMap<String, Vec<PathBuf>> =
        std::collections::BTreeMap::new();
    let mut root_bucket: Vec<PathBuf> = Vec::new();

    for file in staged_files {
        let mut matched = false;
        for pkg in &packages {
            if file.starts_with(&pkg.path) {
                buckets
                    .entry(pkg.name.clone())
                    .or_default()
                    .push(file.clone());
                matched = true;
                break;
            }
        }
        if !matched {
            root_bucket.push(file.clone());
        }
    }

    let mut out: Vec<PackageMatch<'a>> = Vec::new();
    if !root_bucket.is_empty() {
        out.push(PackageMatch::Root(root_bucket));
    }
    for pkg in config.packages.values() {
        if let Some(files) = buckets.get(&pkg.name) {
            out.push(PackageMatch::Package(pkg, files.clone()));
        }
    }
    out
}

/// Pick the right hook for a [`PackageMatch`] on a given name.
/// Package-level hooks win; otherwise fall back to the root hook.
/// Phase 35 layers per-job overrides on top of the root instead of
/// picking one or the other wholesale.
#[must_use]
pub fn hook_for_match<'a>(
    config: &'a Config,
    m: &PackageMatch<'a>,
    hook_name: &str,
) -> Option<&'a Hook> {
    match m {
        PackageMatch::Root(_) => config.hooks.get(hook_name),
        PackageMatch::Package(pkg, _) => pkg
            .hooks
            .get(hook_name)
            .or_else(|| config.hooks.get(hook_name)),
    }
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
