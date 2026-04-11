//! `betterhook status` — machine-readable introspection for agents.
//!
//! Returns a JSON document describing the current worktree's betterhook
//! state: installed hooks + their SHAs, resolved config path and format,
//! hook list with job names, git identity, and daemon socket when one
//! exists. Everything an agent needs to decide whether a commit will
//! trigger betterhook and what jobs it would run.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dispatch::find_config;
use crate::error::ConfigResult;
use crate::git::{git_common_dir, git_dir, show_toplevel};
use crate::install::{InstalledManifest, MANIFEST_FILENAME};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub betterhook_version: &'static str,
    pub worktree: WorktreeInfo,
    pub installed: Option<InstalledInfo>,
    pub config: Option<ConfigInfo>,
    /// Phase 40: speculative runner snapshot. Read from the sidecar
    /// the daemon writes at `<common>/betterhook/speculative-stats.json`.
    /// `None` means the daemon has never run in this repo.
    #[serde(default)]
    pub speculative: Option<crate::daemon::speculative::SpeculativeStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledInfo {
    pub manifest_path: PathBuf,
    pub wrapper_version: u32,
    pub betterhook_bin: String,
    pub installed_for_versions: String,
    pub hooks: Vec<InstalledHook>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledHook {
    pub name: String,
    pub expected_sha: String,
    pub present: bool,
    pub matches: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigInfo {
    pub path: PathBuf,
    pub hooks: Vec<HookInfo>,
    /// Monorepo packages declared in the config. Empty for single-
    /// package repos.
    #[serde(default)]
    pub packages: Vec<PackageInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub path: PathBuf,
    pub hooks: Vec<HookInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookInfo {
    pub name: String,
    pub parallel: bool,
    pub fail_fast: bool,
    pub stash_untracked: bool,
    pub jobs: Vec<String>,
    /// Phase 28: resolved DAG summary so agents can see at a glance
    /// which jobs are roots, how many edges exist, and which pairs
    /// will serialize.
    pub dag: Option<DagSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagSummary {
    pub node_count: usize,
    pub edge_count: usize,
    /// Job names that have no parents in the DAG — the scheduler
    /// dispatches these first.
    pub roots: Vec<String>,
    /// `(parent, child)` pairs for every edge. Order is stable.
    pub edges: Vec<(String, String)>,
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum StatusError {
    #[error("git error")]
    #[diagnostic(transparent)]
    Git(#[from] crate::git::GitError),

    #[error("config error")]
    #[diagnostic(transparent)]
    Config(#[from] Box<crate::error::ConfigError>),

    #[error("io error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse manifest at {path}")]
    ManifestParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

pub type StatusResult<T> = Result<T, StatusError>;

/// Collect the full status for `worktree` (defaults to `.`).
pub async fn collect(worktree: Option<&Path>) -> StatusResult<Status> {
    let worktree_arg = worktree.unwrap_or(Path::new("."));
    let toplevel = show_toplevel(worktree_arg).await?;
    let git_dir = git_dir(&toplevel).await?;
    let common_dir = git_common_dir(&toplevel).await?;

    let installed = read_installed(&common_dir).ok().flatten();
    let config = read_config(&toplevel).ok().flatten();
    let speculative = crate::daemon::speculative::read_stats(&common_dir);

    Ok(Status {
        betterhook_version: crate::VERSION,
        worktree: WorktreeInfo {
            path: toplevel,
            git_dir,
            common_dir,
        },
        installed,
        config,
        speculative,
    })
}

fn read_installed(common_dir: &Path) -> StatusResult<Option<InstalledInfo>> {
    let manifest_path = common_dir.join("betterhook").join(MANIFEST_FILENAME);
    let bytes = match std::fs::read(&manifest_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(StatusError::Io {
                path: manifest_path,
                source,
            });
        }
    };
    let manifest: InstalledManifest =
        serde_json::from_slice(&bytes).map_err(|source| StatusError::ManifestParse {
            path: manifest_path.clone(),
            source,
        })?;

    let hooks_dir = common_dir.join("hooks");
    let mut hooks: Vec<InstalledHook> = manifest
        .hooks
        .iter()
        .map(|(name, expected_sha)| {
            let target = hooks_dir.join(name);
            let (present, matches) = match std::fs::read(&target) {
                Ok(bytes) => {
                    let actual = crate::install::sha256_hex(&bytes);
                    (true, &actual == expected_sha)
                }
                Err(_) => (false, false),
            };
            InstalledHook {
                name: name.clone(),
                expected_sha: expected_sha.clone(),
                present,
                matches,
            }
        })
        .collect();
    hooks.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Some(InstalledInfo {
        manifest_path,
        wrapper_version: manifest.wrapper_version,
        betterhook_bin: manifest.betterhook_bin,
        installed_for_versions: manifest.betterhook_version,
        hooks,
    }))
}

fn read_config(worktree: &Path) -> ConfigResult<Option<ConfigInfo>> {
    let Some(path) = find_config(worktree) else {
        return Ok(None);
    };
    let config = crate::config::load(&path)?;
    let hooks: Vec<HookInfo> = config.hooks.values().map(hook_info).collect();
    let packages: Vec<PackageInfo> = config
        .packages
        .values()
        .map(|pkg| PackageInfo {
            name: pkg.name.clone(),
            path: pkg.path.clone(),
            hooks: pkg.hooks.values().map(hook_info).collect(),
        })
        .collect();
    Ok(Some(ConfigInfo {
        path,
        hooks,
        packages,
    }))
}

fn hook_info(h: &crate::config::Hook) -> HookInfo {
    HookInfo {
        name: h.name.clone(),
        parallel: h.parallel,
        fail_fast: h.fail_fast,
        stash_untracked: h.stash_untracked,
        jobs: h.jobs.iter().map(|j| j.name.clone()).collect(),
        dag: summarize_dag(h),
    }
}

fn summarize_dag(hook: &crate::config::Hook) -> Option<DagSummary> {
    let graph = crate::runner::build_dag(&hook.jobs).ok()?;
    let roots: Vec<String> = graph
        .roots()
        .into_iter()
        .map(|i| graph.nodes[i].job.name.clone())
        .collect();
    let edges: Vec<(String, String)> = graph
        .edges()
        .into_iter()
        .map(|(a, b)| (graph.nodes[a].job.name.clone(), graph.nodes[b].job.name.clone()))
        .collect();
    Some(DagSummary {
        node_count: graph.nodes.len(),
        edge_count: graph.edge_count(),
        roots,
        edges,
    })
}

