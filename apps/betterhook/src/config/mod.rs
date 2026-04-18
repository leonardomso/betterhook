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

pub use extends::resolve;
pub use parse::{Format, parse_bytes, parse_file};
pub use schema::{Config, Hook, IsolateSpec, Job, Meta, Package, RawConfig, ToolPathScope};

use crate::error::ConfigResult;

/// Load a config file, resolve `extends`, apply `betterhook.local.*`, and
/// lower the result to the canonical typed [`Config`].
pub fn load(path: &Path) -> ConfigResult<Config> {
    extends::resolve(path)?.lower()
}
