//! Config parsing, validation, and canonical AST.
//!
//! The public entry points are [`parse::parse_file`] and [`parse::parse_bytes`].
//! Both produce a [`schema::RawConfig`] which can be lowered to the typed
//! [`schema::Config`] via [`schema::RawConfig::lower`].

pub mod extends;
pub mod import;
pub mod kdl;
pub mod migrate;
pub mod parse;
pub mod schema;

use std::path::Path;
use std::path::PathBuf;

pub use extends::resolve;
pub use parse::{Format, parse_bytes, parse_file};
pub use schema::{
    Config, Hook, HookName, IsolateSpec, Job, JobName, Meta, Package, PackageName, RawConfig,
    ToolPathScope,
};

use crate::error::ConfigResult;

/// Candidate config filenames, in lookup order. First match wins.
pub const CONFIG_CANDIDATES: &[&str] = &[
    "betterhook.toml",
    "betterhook.yml",
    "betterhook.yaml",
    "betterhook.json",
    "betterhook.kdl",
];

/// Find the first `betterhook.*` config file in `worktree`.
#[must_use]
pub fn find_config_path(worktree: &Path) -> Option<PathBuf> {
    for name in CONFIG_CANDIDATES {
        let candidate = worktree.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Load a config file, resolve `extends`, apply `betterhook.local.*`, and
/// lower the result to the canonical typed [`Config`].
pub fn load(path: &Path) -> ConfigResult<Config> {
    extends::resolve(path)?.lower()
}
