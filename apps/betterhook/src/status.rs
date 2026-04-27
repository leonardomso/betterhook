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
use crate::git::{git_common_dir, git_dir, show_toplevel};
use crate::install::{InstalledManifest, MANIFEST_FILENAME};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub betterhook_version: &'static str,
    pub worktree: WorktreeInfo,
    pub installed: Option<InstalledInfo>,
    pub config: Option<ConfigInfo>,
    #[serde(default)]
    pub diagnostics: Vec<StatusDiagnostic>,
    /// Snapshot of speculative-runner state read from
    /// `<common>/betterhook/speculative-stats.json`.
    /// `None` means the daemon has not written stats for this repo yet.
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
    /// Summary of the resolved execution DAG so agents can see which
    /// jobs start first and which pairs must serialize.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusDiagnostic {
    pub component: StatusComponent,
    pub path: Option<PathBuf>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusComponent {
    Installed,
    Config,
    Speculative,
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum StatusError {
    #[error("git error")]
    #[diagnostic(transparent)]
    Git(#[from] crate::git::GitError),

    #[error("config error at {path}")]
    Config {
        path: PathBuf,
        #[source]
        source: Box<crate::error::ConfigError>,
    },

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

    #[error("failed to parse speculative stats at {path}")]
    SpeculativeParse {
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

    let mut diagnostics = Vec::new();
    let installed = match read_installed_async(&common_dir).await {
        Ok(installed) => installed,
        Err(err) => {
            diagnostics.push(diagnostic_for_error(StatusComponent::Installed, &err));
            None
        }
    };
    let config = match read_config_async(&toplevel).await {
        Ok(config) => config,
        Err(err) => {
            diagnostics.push(diagnostic_for_error(StatusComponent::Config, &err));
            None
        }
    };
    let speculative = match read_speculative_async(&common_dir).await {
        Ok(speculative) => speculative,
        Err(err) => {
            diagnostics.push(diagnostic_for_error(StatusComponent::Speculative, &err));
            None
        }
    };

    Ok(Status {
        betterhook_version: crate::VERSION,
        worktree: WorktreeInfo {
            path: toplevel,
            git_dir,
            common_dir,
        },
        installed,
        config,
        diagnostics,
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

fn read_config(worktree: &Path) -> StatusResult<Option<ConfigInfo>> {
    let Some(path) = find_config(worktree) else {
        return Ok(None);
    };
    let config = crate::config::load(&path).map_err(|source| StatusError::Config {
        path: path.clone(),
        source: Box::new(source),
    })?;
    let hooks: Vec<HookInfo> = config.hooks.values().map(hook_info).collect();
    let packages: Vec<PackageInfo> = config
        .packages
        .values()
        .map(|pkg| PackageInfo {
            name: pkg.name.to_string(),
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

fn read_speculative(
    common_dir: &Path,
) -> StatusResult<Option<crate::daemon::speculative::SpeculativeStats>> {
    let path = crate::daemon::speculative::stats_path(common_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(StatusError::Io { path, source }),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| StatusError::SpeculativeParse { path, source })
}

async fn read_installed_async(common_dir: &Path) -> StatusResult<Option<InstalledInfo>> {
    let common_dir = common_dir.to_path_buf();
    let task_common_dir = common_dir.clone();
    tokio::task::spawn_blocking(move || read_installed(&task_common_dir))
        .await
        .map_err(|source| StatusError::Io {
            path: common_dir.clone(),
            source: std::io::Error::other(format!("status installed task failed: {source}")),
        })?
}

async fn read_config_async(worktree: &Path) -> StatusResult<Option<ConfigInfo>> {
    let worktree = worktree.to_path_buf();
    let task_worktree = worktree.clone();
    tokio::task::spawn_blocking(move || read_config(&task_worktree))
        .await
        .map_err(|source| StatusError::Io {
            path: worktree.clone(),
            source: std::io::Error::other(format!("status config task failed: {source}")),
        })?
}

async fn read_speculative_async(
    common_dir: &Path,
) -> StatusResult<Option<crate::daemon::speculative::SpeculativeStats>> {
    let common_dir = common_dir.to_path_buf();
    let task_common_dir = common_dir.clone();
    tokio::task::spawn_blocking(move || read_speculative(&task_common_dir))
        .await
        .map_err(|source| StatusError::Io {
            path: common_dir.clone(),
            source: std::io::Error::other(format!("status speculative task failed: {source}")),
        })?
}

fn diagnostic_for_error(component: StatusComponent, err: &StatusError) -> StatusDiagnostic {
    let path = match err {
        StatusError::Io { path, .. }
        | StatusError::ManifestParse { path, .. }
        | StatusError::SpeculativeParse { path, .. }
        | StatusError::Config { path, .. } => Some(path.clone()),
        StatusError::Git(_) => None,
    };
    StatusDiagnostic {
        component,
        path,
        message: err.to_string(),
    }
}

fn hook_info(h: &crate::config::Hook) -> HookInfo {
    HookInfo {
        name: h.name.to_string(),
        parallel: h.parallel,
        fail_fast: h.fail_fast,
        stash_untracked: h.stash_untracked,
        jobs: h.jobs.iter().map(|j| j.name.to_string()).collect(),
        dag: summarize_dag(h),
    }
}

fn summarize_dag(hook: &crate::config::Hook) -> Option<DagSummary> {
    let graph = crate::runner::build_dag(&hook.jobs).ok()?;
    let roots: Vec<String> = graph
        .roots()
        .into_iter()
        .map(|i| graph.nodes[i].job.name.to_string())
        .collect();
    let edges: Vec<(String, String)> = graph
        .edges()
        .into_iter()
        .map(|(a, b)| {
            (
                graph.nodes[a].job.name.to_string(),
                graph.nodes[b].job.name.to_string(),
            )
        })
        .collect();
    Some(DagSummary {
        node_count: graph.nodes.len(),
        edge_count: graph.edge_count(),
        roots,
        edges,
    })
}
