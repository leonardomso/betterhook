//! Install, uninstall, and the wrapper-script model.
//!
//! The wrapper script we write into `<common-dir>/hooks/<name>` is
//! worktree-aware — at runtime it calls `git rev-parse --show-toplevel`
//! to identify which worktree is committing, then dispatches to that
//! worktree's own `betterhook.{toml,yml,json}`. All worktrees share one
//! byte-identical wrapper, so installs from different worktrees don't
//! race each other.
//!
//! See the `manifest` module for the schema of `installed.json`, which
//! is how `uninstall` knows what's safe to remove.

pub mod manifest;
pub mod wrapper;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use miette::Diagnostic;
use thiserror::Error;

use crate::git::GitError;
pub use manifest::{InstalledManifest, MANIFEST_FILENAME};
pub use wrapper::{WRAPPER_VERSION, render_wrapper, sha256_hex};

#[derive(Debug, Error, Diagnostic)]
pub enum InstallError {
    #[error("failed to load betterhook config at {path}")]
    #[diagnostic(code(betterhook::install::config_missing))]
    ConfigMissing {
        path: PathBuf,
        #[source]
        source: Box<crate::error::ConfigError>,
    },

    #[error("git error")]
    #[diagnostic(transparent)]
    Git(#[from] GitError),

    #[error("filesystem error at {path}")]
    #[diagnostic(code(betterhook::install::io))]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "core.hooksPath is already set to {existing:?} — pass --takeover to replace it or unset it manually"
    )]
    #[diagnostic(
        code(betterhook::install::foreign_core_hooks_path),
        help("another hooks tool (husky, pre-commit) may own core.hooksPath already")
    )]
    ForeignCoreHooksPath { existing: PathBuf },

    #[error("betterhook is not installed in {common_dir}: no {manifest}")]
    #[diagnostic(code(betterhook::install::not_installed))]
    NotInstalled {
        common_dir: PathBuf,
        manifest: String,
    },

    #[error("failed to parse installed manifest at {path}")]
    #[diagnostic(code(betterhook::install::manifest))]
    Manifest {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

pub type InstallResult<T> = Result<T, InstallError>;

/// Outcome of a successful install, suitable for human or json reporting.
#[derive(Debug, Clone)]
pub struct InstallReport {
    pub common_dir: PathBuf,
    pub hooks_dir: PathBuf,
    pub installed: Vec<String>,
    pub manifest_path: PathBuf,
    /// Set when a launchd/systemd unit was written. The CLI surfaces
    /// the included `load_command` so the user can finalize it.
    pub unit: Option<crate::daemon::lifecycle::InstalledUnit>,
}

/// Outcome of a successful uninstall.
#[derive(Debug, Clone)]
pub struct UninstallReport {
    pub removed: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

/// Options accepted by [`install`].
#[derive(Debug, Default, Clone)]
pub struct InstallOptions {
    /// Worktree root to read `betterhook.*` from. Defaults to `.`.
    pub worktree: Option<PathBuf>,
    /// Explicit config file path, overriding auto-discovery.
    pub config_path: Option<PathBuf>,
    /// If `Some`, install wrappers only for these hook types, ignoring
    /// whatever is in the config's `hooks` map.
    pub only_hooks: Option<Vec<String>>,
    /// Unset a foreign `core.hooksPath` instead of refusing.
    pub takeover: bool,
    /// Skip writing the launchd/systemd unit file (default: false).
    /// Useful for transient repos, CI, or tests that don't want to
    /// touch `~/Library/LaunchAgents/`.
    pub skip_unit: bool,
    /// Override the directory where the unit file is written. Tests
    /// use this to write into a tempdir instead of the real platform
    /// location.
    pub unit_dir_override: Option<PathBuf>,
}

/// Install worktree-aware wrappers into `<common-dir>/hooks/` for every
/// hook type declared in the resolved config.
pub async fn install(opts: InstallOptions) -> InstallResult<InstallReport> {
    let worktree = opts.worktree.clone().unwrap_or_else(|| PathBuf::from("."));
    let config_path = opts
        .config_path
        .clone()
        .or_else(|| crate::config::find_config_path(&worktree))
        .unwrap_or_else(|| worktree.join("betterhook.toml"));

    let config = load_config_async(&config_path).await?;

    let common_dir = crate::git::git_common_dir(&worktree).await?;

    // Honor a pre-existing core.hooksPath — either take over or refuse.
    if let Some(existing) = get_core_hooks_path(&worktree).await? {
        if opts.takeover {
            unset_core_hooks_path(&worktree).await?;
        } else {
            return Err(InstallError::ForeignCoreHooksPath { existing });
        }
    }

    let hooks_dir = common_dir.join("hooks");
    let bin = current_exe_path_async().await?;
    let bin_str = bin.display().to_string();

    let hook_types: Vec<String> = opts
        .only_hooks
        .clone()
        .unwrap_or_else(|| config.hooks.keys().cloned().collect());

    let hooks_dir_for_blocking = hooks_dir.clone();
    let common_dir_for_blocking = common_dir.clone();
    let bin_for_blocking = bin.clone();
    let bin_str_for_blocking = bin_str.clone();
    let opts_for_blocking = opts.clone();
    let hook_types_for_blocking = hook_types.clone();
    let (manifest_path, unit, installed_order) = tokio::task::spawn_blocking(move || {
        ensure_dir(&hooks_dir_for_blocking)?;
        let (installed_shas, installed_order) = write_wrappers(
            &hooks_dir_for_blocking,
            &bin_str_for_blocking,
            &hook_types_for_blocking,
        )?;
        let (manifest_path, unit) = write_installation_metadata(
            &common_dir_for_blocking,
            &bin_for_blocking,
            &bin_str_for_blocking,
            &opts_for_blocking,
            installed_shas,
        )?;
        Ok::<_, InstallError>((manifest_path, unit, installed_order))
    })
    .await
    .map_err(|source| InstallError::Io {
        path: hooks_dir.clone(),
        source: std::io::Error::other(format!("install task failed: {source}")),
    })??;

    Ok(InstallReport {
        common_dir,
        hooks_dir,
        installed: installed_order,
        manifest_path,
        unit,
    })
}

/// Stamp a wrapper into every hook type declared in the config. The
/// wrapper is byte-identical across hook types — what differs is the
/// filename git uses to resolve it.
fn write_wrappers(
    hooks_dir: &Path,
    bin_str: &str,
    hook_types: &[String],
) -> InstallResult<(BTreeMap<String, String>, Vec<String>)> {
    let wrapper = render_wrapper(bin_str);
    let wrapper_sha = sha256_hex(wrapper.as_bytes());
    let mut installed_shas: BTreeMap<String, String> = BTreeMap::new();
    let mut installed_order: Vec<String> = Vec::new();
    for hook_name in hook_types {
        let target = hooks_dir.join(hook_name);
        write_executable(&target, &wrapper)?;
        installed_shas.insert(hook_name.clone(), wrapper_sha.clone());
        installed_order.push(hook_name.clone());
    }
    Ok((installed_shas, installed_order))
}

/// Write the installed manifest and (on supported platforms) the
/// launchd/systemd unit file. Best-effort for the unit file: an
/// unsupported platform or `skip_unit = true` leaves the manifest's
/// `unit_path` as `None` and the on-demand spawn path keeps working.
fn write_installation_metadata(
    common_dir: &Path,
    bin: &Path,
    bin_str: &str,
    opts: &InstallOptions,
    installed_shas: BTreeMap<String, String>,
) -> InstallResult<(PathBuf, Option<crate::daemon::lifecycle::InstalledUnit>)> {
    let manifest_dir = common_dir.join("betterhook");
    ensure_dir(&manifest_dir)?;
    let socket_path = manifest_dir.join("sock");
    let unit = if opts.skip_unit {
        None
    } else {
        crate::daemon::lifecycle::install_unit(
            common_dir,
            bin,
            &socket_path,
            opts.unit_dir_override.as_deref(),
        )
        .map_err(|source| InstallError::Io {
            path: socket_path.clone(),
            source,
        })?
    };
    let manifest_path = manifest_dir.join(MANIFEST_FILENAME);
    let manifest = InstalledManifest {
        wrapper_version: WRAPPER_VERSION,
        betterhook_version: crate::VERSION.to_string(),
        betterhook_bin: bin_str.to_owned(),
        hooks: installed_shas,
        previous_core_hooks_path: None,
        unit_path: unit.as_ref().map(|u| u.path.clone()),
    };
    write_manifest(&manifest_path, &manifest)?;
    Ok((manifest_path, unit))
}

/// Remove only wrappers whose SHA-256 matches what we wrote. User-edited
/// hooks and third-party wrappers are never touched.
pub async fn uninstall(worktree: Option<PathBuf>) -> InstallResult<UninstallReport> {
    let worktree = worktree.unwrap_or_else(|| PathBuf::from("."));
    let common_dir = crate::git::git_common_dir(&worktree).await?;
    let common_dir_for_blocking = common_dir.clone();
    tokio::task::spawn_blocking(move || uninstall_blocking(&common_dir_for_blocking))
        .await
        .map_err(|source| InstallError::Io {
            path: common_dir.clone(),
            source: std::io::Error::other(format!("uninstall task failed: {source}")),
        })?
}

// ============================================================================
// Helpers
// ============================================================================

async fn get_core_hooks_path(cwd: &Path) -> InstallResult<Option<PathBuf>> {
    match crate::git::run_git(cwd, ["config", "--get", "core.hooksPath"]).await {
        Ok(bytes) => {
            let s = String::from_utf8_lossy(&bytes).trim().to_string();
            if s.is_empty() {
                Ok(None)
            } else {
                Ok(Some(PathBuf::from(s)))
            }
        }
        Err(GitError::NonZero { status: 1, .. }) => Ok(None), // exit 1 = key not set
        Err(e) => Err(InstallError::Git(e)),
    }
}

async fn unset_core_hooks_path(cwd: &Path) -> InstallResult<()> {
    crate::git::run_git(cwd, ["config", "--unset", "core.hooksPath"]).await?;
    Ok(())
}

async fn load_config_async(path: &Path) -> InstallResult<crate::config::Config> {
    let path = path.to_path_buf();
    let task_path = path.clone();
    tokio::task::spawn_blocking(move || {
        let error_path = task_path.clone();
        crate::config::load(&task_path).map_err(|source| InstallError::ConfigMissing {
            path: error_path,
            source: Box::new(source),
        })
    })
    .await
    .map_err(|source| InstallError::Io {
        path: path.clone(),
        source: std::io::Error::other(format!("config load task failed: {source}")),
    })?
}

async fn current_exe_path_async() -> InstallResult<PathBuf> {
    tokio::task::spawn_blocking(current_exe_path)
        .await
        .map_err(|source| InstallError::Io {
            path: PathBuf::from("<current_exe>"),
            source: std::io::Error::other(format!("current_exe task failed: {source}")),
        })?
}

fn uninstall_blocking(common_dir: &Path) -> InstallResult<UninstallReport> {
    let manifest_path = common_dir.join("betterhook").join(MANIFEST_FILENAME);
    let manifest = read_manifest(&manifest_path).map_err(|err| match err {
        InstallError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            InstallError::NotInstalled {
                common_dir: common_dir.to_path_buf(),
                manifest: MANIFEST_FILENAME.to_string(),
            }
        }
        other => other,
    })?;

    let hooks_dir = common_dir.join("hooks");
    let mut removed = Vec::new();
    let mut skipped = Vec::new();

    for (hook_name, expected_sha) in &manifest.hooks {
        let target = hooks_dir.join(hook_name);
        match std::fs::read(&target) {
            Ok(bytes) => {
                let current_sha = sha256_hex(&bytes);
                if &current_sha == expected_sha {
                    std::fs::remove_file(&target).map_err(|source| InstallError::Io {
                        path: target.clone(),
                        source,
                    })?;
                    removed.push(hook_name.clone());
                } else {
                    skipped.push((
                        hook_name.clone(),
                        "hook was modified after install — leaving in place".to_string(),
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                skipped.push((hook_name.clone(), "already removed".to_string()));
            }
            Err(source) => {
                return Err(InstallError::Io {
                    path: target,
                    source,
                });
            }
        }
    }

    if let Some(unit_path) = &manifest.unit_path {
        crate::daemon::lifecycle::uninstall_unit(unit_path).map_err(|source| InstallError::Io {
            path: unit_path.clone(),
            source,
        })?;
    }

    std::fs::remove_file(&manifest_path).map_err(|source| InstallError::Io {
        path: manifest_path.clone(),
        source,
    })?;

    Ok(UninstallReport { removed, skipped })
}

fn ensure_dir(p: &Path) -> InstallResult<()> {
    std::fs::create_dir_all(p).map_err(|source| InstallError::Io {
        path: p.to_path_buf(),
        source,
    })
}

fn write_executable(path: &Path, content: &str) -> InstallResult<()> {
    std::fs::write(path, content).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut perms = std::fs::metadata(path)
        .map_err(|source| InstallError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn write_manifest(path: &Path, m: &InstalledManifest) -> InstallResult<()> {
    let bytes = serde_json::to_vec_pretty(m).map_err(|source| InstallError::Manifest {
        path: path.to_path_buf(),
        source,
    })?;
    std::fs::write(path, bytes).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn read_manifest(path: &Path) -> InstallResult<InstalledManifest> {
    let bytes = std::fs::read(path).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| InstallError::Manifest {
        path: path.to_path_buf(),
        source,
    })
}

fn current_exe_path() -> InstallResult<PathBuf> {
    std::env::current_exe().map_err(|source| InstallError::Io {
        path: PathBuf::from("<current_exe>"),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::new_git_repo_with_file;
    use std::process::Command as StdCommand;

    fn write_minimal_config(root: &Path) {
        std::fs::write(
            root.join("betterhook.toml"),
            r#"
[meta]
version = 1

[hooks.pre-commit.jobs.t]
run = "true"
"#,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn install_creates_wrapper_and_manifest() {
        let (_d, root) = new_git_repo_with_file("README.md", "hi");
        write_minimal_config(&root);

        let report = install(InstallOptions {
            worktree: Some(root.clone()),
            skip_unit: true,
            ..Default::default()
        })
        .await
        .unwrap();

        assert_eq!(report.installed, vec!["pre-commit".to_string()]);
        let wrapper_path = report.hooks_dir.join("pre-commit");
        assert!(wrapper_path.is_file(), "wrapper missing");
        let bytes = std::fs::read(&wrapper_path).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("rev-parse --show-toplevel"));
        assert!(text.contains("__dispatch"));
        let perms = std::fs::metadata(&wrapper_path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o111, 0o111, "wrapper not executable");

        assert!(report.manifest_path.is_file(), "manifest missing");
    }

    #[tokio::test]
    async fn reinstall_is_idempotent() {
        let (_d, root) = new_git_repo_with_file("README.md", "hi");
        write_minimal_config(&root);

        let first = install(InstallOptions {
            worktree: Some(root.clone()),
            skip_unit: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let first_bytes = std::fs::read(first.hooks_dir.join("pre-commit")).unwrap();

        let second = install(InstallOptions {
            worktree: Some(root.clone()),
            skip_unit: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let second_bytes = std::fs::read(second.hooks_dir.join("pre-commit")).unwrap();

        assert_eq!(first_bytes, second_bytes, "wrappers must be byte-identical");
    }

    #[tokio::test]
    async fn uninstall_removes_only_managed_wrappers() {
        let (_d, root) = new_git_repo_with_file("README.md", "hi");
        write_minimal_config(&root);

        install(InstallOptions {
            worktree: Some(root.clone()),
            skip_unit: true,
            ..Default::default()
        })
        .await
        .unwrap();

        let report = uninstall(Some(root.clone())).await.unwrap();
        assert_eq!(report.removed, vec!["pre-commit".to_string()]);
        assert!(report.skipped.is_empty());

        let common_dir = crate::git::git_common_dir(&root).await.unwrap();
        assert!(!common_dir.join("hooks").join("pre-commit").exists());
        assert!(
            !common_dir
                .join("betterhook")
                .join(MANIFEST_FILENAME)
                .exists()
        );
    }

    #[tokio::test]
    async fn uninstall_refuses_to_touch_user_modified_hook() {
        let (_d, root) = new_git_repo_with_file("README.md", "hi");
        write_minimal_config(&root);

        let report = install(InstallOptions {
            worktree: Some(root.clone()),
            skip_unit: true,
            ..Default::default()
        })
        .await
        .unwrap();

        let wrapper_path = report.hooks_dir.join("pre-commit");
        std::fs::write(&wrapper_path, "#!/bin/sh\necho user-modified\n").unwrap();

        let rep = uninstall(Some(root.clone())).await.unwrap();
        assert!(rep.removed.is_empty());
        assert_eq!(rep.skipped.len(), 1);
        assert!(
            wrapper_path.exists(),
            "user-modified file must be preserved"
        );
    }

    #[tokio::test]
    async fn install_refuses_foreign_core_hooks_path() {
        let (_d, root) = new_git_repo_with_file("README.md", "hi");
        write_minimal_config(&root);
        let status = StdCommand::new("git")
            .current_dir(&root)
            .args(["config", "core.hooksPath", ".githooks"])
            .status()
            .unwrap();
        assert!(status.success());

        let err = install(InstallOptions {
            worktree: Some(root.clone()),
            skip_unit: true,
            ..Default::default()
        })
        .await
        .unwrap_err();
        assert!(matches!(err, InstallError::ForeignCoreHooksPath { .. }));
    }

    #[tokio::test]
    async fn install_takeover_unsets_foreign_core_hooks_path() {
        let (_d, root) = new_git_repo_with_file("README.md", "hi");
        write_minimal_config(&root);
        StdCommand::new("git")
            .current_dir(&root)
            .args(["config", "core.hooksPath", ".githooks"])
            .status()
            .unwrap();

        install(InstallOptions {
            worktree: Some(root.clone()),
            takeover: true,
            skip_unit: true,
            ..Default::default()
        })
        .await
        .unwrap();

        let still = get_core_hooks_path(&root).await.unwrap();
        assert!(
            still.is_none(),
            "core.hooksPath should be unset after takeover"
        );
    }
}
