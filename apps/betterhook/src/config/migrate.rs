//! Compatibility shim — `betterhook migrate` was renamed to
//! `betterhook import --from lefthook` in phase 51. This module
//! re-exports the lefthook importer so older callers (and the hidden
//! `migrate` CLI alias) keep working.

use std::path::Path;

use crate::config::import::{self, ImportSource, MigrationReport};
use crate::config::schema::RawConfig;
use crate::error::ConfigResult;

/// Read a `lefthook.yml` and return the converted `RawConfig`.
pub fn from_lefthook_file(path: &Path) -> ConfigResult<(RawConfig, MigrationReport)> {
    import::import_file(ImportSource::Lefthook, path)
}

/// Convert raw lefthook YAML text into a `RawConfig`.
pub fn from_lefthook_yaml(source: &str) -> ConfigResult<(RawConfig, MigrationReport)> {
    import::lefthook::from_yaml(source)
}

pub use crate::config::import::MigrationReport as MigrationReportAlias;
