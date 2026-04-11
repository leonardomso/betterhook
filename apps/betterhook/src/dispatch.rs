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
///
/// Phase 35 semantics:
/// - `Root` match → the config's root hook for that name
/// - `Package` match with no package-level hook → the root hook
/// - `Package` match with a package-level hook → a **merged** hook
///   that layers the package's jobs on top of the root's jobs:
///   same-named jobs are replaced wholesale, new jobs are added,
///   and hook-level flags (`parallel`, `fail_fast`, `priority`, etc.)
///   come from the package when declared.
///
/// v1.0.1: returns `Cow<'a, Hook>` so the common "no overlay" path
/// doesn't clone the whole `Hook` (including every nested `Job`).
/// Only the merge branch allocates. Saves ~0.5-1 ms per hook on a
/// config with five jobs.
#[must_use]
pub fn hook_for_match<'a>(
    config: &'a Config,
    m: &PackageMatch<'a>,
    hook_name: &str,
) -> Option<std::borrow::Cow<'a, Hook>> {
    use std::borrow::Cow;
    match m {
        PackageMatch::Root(_) => config.hooks.get(hook_name).map(Cow::Borrowed),
        PackageMatch::Package(pkg, _) => {
            let package_hook = pkg.hooks.get(hook_name);
            let root_hook = config.hooks.get(hook_name);
            match (package_hook, root_hook) {
                (None, None) => None,
                (Some(p), None) => Some(Cow::Borrowed(p)),
                (None, Some(r)) => Some(Cow::Borrowed(r)),
                (Some(p), Some(r)) => Some(Cow::Owned(merge_hooks(r, p))),
            }
        }
    }
}

/// Overlay package hook `overlay` on top of root hook `base`.
/// - Hook-level flags: overlay wins
/// - Jobs: overlay's jobs replace same-named root jobs; otherwise
///   root jobs are kept as-is
/// - Order: root jobs first in priority order, then overlay-only
///   jobs, then sort by priority so the final result is stable
fn merge_hooks(base: &Hook, overlay: &Hook) -> Hook {
    let mut jobs: Vec<crate::config::Job> = Vec::new();
    // Overlay job names that replace base entries.
    let overlay_names: std::collections::BTreeSet<&str> =
        overlay.jobs.iter().map(|j| j.name.as_str()).collect();
    for base_job in &base.jobs {
        if overlay_names.contains(base_job.name.as_str()) {
            continue;
        }
        jobs.push(base_job.clone());
    }
    for overlay_job in &overlay.jobs {
        jobs.push(overlay_job.clone());
    }
    jobs.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.name.cmp(&b.name)));
    Hook {
        name: base.name.clone(),
        parallel: overlay.parallel || base.parallel,
        fail_fast: overlay.fail_fast || base.fail_fast,
        parallel_limit: overlay.parallel_limit.or(base.parallel_limit),
        stash_untracked: overlay.stash_untracked,
        jobs,
    }
}

#[cfg(test)]
mod hook_merge_tests {
    use super::*;
    use crate::config::{IsolateSpec, Job, Package};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn mk_job(name: &str, priority: u32) -> Job {
        Job {
            name: name.to_owned(),
            run: "true".to_owned(),
            fix: None,
            glob: Vec::new(),
            exclude: Vec::new(),
            tags: Vec::new(),
            skip: None,
            only: None,
            env: BTreeMap::new(),
            root: None,
            stage_fixed: false,
            isolate: None::<IsolateSpec>,
            timeout: None,
            interactive: false,
            fail_text: None,
            priority,
            reads: Vec::new(),
            writes: Vec::new(),
            network: false,
            concurrent_safe: false,
        }
    }

    fn mk_hook(name: &str, jobs: Vec<Job>) -> Hook {
        Hook {
            name: name.to_owned(),
            parallel: false,
            fail_fast: false,
            parallel_limit: None,
            stash_untracked: false,
            jobs,
        }
    }

    fn mk_config(root_hooks: Vec<Hook>, pkgs: Vec<(String, &str, Vec<Hook>)>) -> Config {
        let mut hooks = BTreeMap::new();
        for h in root_hooks {
            hooks.insert(h.name.clone(), h);
        }
        let mut packages = BTreeMap::new();
        for (name, path, hooks_vec) in pkgs {
            let mut pkg_hooks = BTreeMap::new();
            for h in hooks_vec {
                pkg_hooks.insert(h.name.clone(), h);
            }
            packages.insert(
                name.clone(),
                Package {
                    name,
                    path: PathBuf::from(path),
                    hooks: pkg_hooks,
                },
            );
        }
        Config {
            meta: crate::config::Meta {
                version: 1,
                min_betterhook: None,
            },
            hooks,
            packages,
        }
    }

    #[test]
    fn package_job_replaces_root_job_of_same_name() {
        let root = mk_hook(
            "pre-commit",
            vec![mk_job("lint", 0), mk_job("test", 1)],
        );
        let pkg_hook = mk_hook("pre-commit", vec![mk_job("lint", 0)]); // pretend it's a different lint
        let config = mk_config(
            vec![root],
            vec![("frontend".to_owned(), "apps/web", vec![pkg_hook])],
        );
        let match_ = PackageMatch::Package(
            config.packages.get("frontend").unwrap(),
            vec![PathBuf::from("apps/web/src/a.ts")],
        );
        let merged = hook_for_match(&config, &match_, "pre-commit").unwrap();
        // Only one "lint", plus the "test" from root.
        assert_eq!(merged.jobs.len(), 2);
        let names: Vec<&str> = merged.jobs.iter().map(|j| j.name.as_str()).collect();
        assert!(names.contains(&"lint"));
        assert!(names.contains(&"test"));
    }

    #[test]
    fn package_with_no_hook_falls_back_to_root() {
        let root = mk_hook("pre-commit", vec![mk_job("lint", 0)]);
        let config = mk_config(
            vec![root],
            vec![("frontend".to_owned(), "apps/web", Vec::new())],
        );
        let match_ = PackageMatch::Package(
            config.packages.get("frontend").unwrap(),
            vec![PathBuf::from("apps/web/a.ts")],
        );
        let merged = hook_for_match(&config, &match_, "pre-commit").unwrap();
        assert_eq!(merged.jobs.len(), 1);
        assert_eq!(merged.jobs[0].name, "lint");
    }

    #[test]
    fn package_only_hook_works_without_root() {
        let pkg_hook = mk_hook("pre-commit", vec![mk_job("scoped", 0)]);
        let config = mk_config(
            Vec::new(),
            vec![("frontend".to_owned(), "apps/web", vec![pkg_hook])],
        );
        let match_ = PackageMatch::Package(
            config.packages.get("frontend").unwrap(),
            vec![PathBuf::from("apps/web/a.ts")],
        );
        let merged = hook_for_match(&config, &match_, "pre-commit").unwrap();
        assert_eq!(merged.jobs.len(), 1);
        assert_eq!(merged.jobs[0].name, "scoped");
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
