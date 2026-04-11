//! Best-effort importers from other hook managers.
//!
//! Each submodule converts a foreign config (lefthook, husky, hk, or
//! pre-commit) into a betterhook [`RawConfig`] plus a
//! [`MigrationReport`] of any details that didn't survive the round
//! trip. The CLI exposes these via `betterhook import --from <source>`.

use std::path::Path;

use crate::config::schema::RawConfig;
use crate::error::{ConfigError, ConfigResult};

pub mod hk;
pub mod husky;
pub mod lefthook;
pub mod pre_commit;

/// Source format the importer should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSource {
    Lefthook,
    Husky,
    Hk,
    PreCommit,
}

impl ImportSource {
    /// Parse a `--from <source>` value.
    #[must_use]
    pub fn from_cli(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "lefthook" => Some(Self::Lefthook),
            "husky" => Some(Self::Husky),
            "hk" => Some(Self::Hk),
            "pre-commit" | "precommit" => Some(Self::PreCommit),
            _ => None,
        }
    }

    /// Auto-detect a source from a file path. Used when the user runs
    /// `betterhook import` without an explicit `--from`.
    #[must_use]
    pub fn auto_detect(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        if name == "lefthook.yml" || name == "lefthook.yaml" {
            return Some(Self::Lefthook);
        }
        if name == ".pre-commit-config.yaml" || name == ".pre-commit-config.yml" {
            return Some(Self::PreCommit);
        }
        if name == "hk.toml" || name == "hk.yaml" || name == "hk.yml" {
            return Some(Self::Hk);
        }
        // husky scripts live in .husky/<hook-name>; if the path is
        // inside that directory, treat it as husky.
        if path
            .components()
            .any(|c| c.as_os_str().to_string_lossy() == ".husky")
        {
            return Some(Self::Husky);
        }
        None
    }
}

/// Per-source bundle of facts the importer wants the user to see.
#[derive(Debug, Default, Clone)]
pub struct MigrationReport {
    pub notes: Vec<String>,
}

impl MigrationReport {
    pub fn note<S: Into<String>>(&mut self, msg: S) {
        self.notes.push(msg.into());
    }
}

/// Drive an importer end-to-end given a path on disk.
pub fn import_file(
    source: ImportSource,
    path: &Path,
) -> ConfigResult<(RawConfig, MigrationReport)> {
    let bytes = std::fs::read(path).map_err(|src| ConfigError::Io {
        path: path.to_path_buf(),
        source: src,
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|_| ConfigError::Invalid {
        message: format!("{} is not valid UTF-8", path.display()),
    })?;
    match source {
        ImportSource::Lefthook => lefthook::from_yaml(text),
        ImportSource::Husky => husky::from_script(text, path),
        ImportSource::Hk => hk::from_text(text),
        ImportSource::PreCommit => pre_commit::from_yaml(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn from_cli_accepts_known_sources() {
        assert_eq!(
            ImportSource::from_cli("lefthook"),
            Some(ImportSource::Lefthook)
        );
        assert_eq!(ImportSource::from_cli("husky"), Some(ImportSource::Husky));
        assert_eq!(ImportSource::from_cli("hk"), Some(ImportSource::Hk));
        assert_eq!(
            ImportSource::from_cli("pre-commit"),
            Some(ImportSource::PreCommit)
        );
        assert_eq!(ImportSource::from_cli("nope"), None);
    }

    #[test]
    fn auto_detect_picks_obvious_filenames() {
        assert_eq!(
            ImportSource::auto_detect(&PathBuf::from("repo/lefthook.yml")),
            Some(ImportSource::Lefthook)
        );
        assert_eq!(
            ImportSource::auto_detect(&PathBuf::from(".pre-commit-config.yaml")),
            Some(ImportSource::PreCommit)
        );
        assert_eq!(
            ImportSource::auto_detect(&PathBuf::from("hk.toml")),
            Some(ImportSource::Hk)
        );
        assert_eq!(
            ImportSource::auto_detect(&PathBuf::from(".husky/pre-commit")),
            Some(ImportSource::Husky)
        );
        assert_eq!(
            ImportSource::auto_detect(&PathBuf::from("Cargo.toml")),
            None
        );
    }
}
