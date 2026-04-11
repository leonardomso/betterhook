//! Schema for `<common-dir>/betterhook/installed.json`.
//!
//! This file is written by `betterhook install` and consulted by
//! `betterhook uninstall` (SHA-verified) and `betterhook status`
//! (introspection).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Filename under `<common-dir>/betterhook/`.
pub const MANIFEST_FILENAME: &str = "installed.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledManifest {
    /// Wrapper schema version. Bumped alongside `wrapper::WRAPPER_VERSION`.
    pub wrapper_version: u32,
    /// The `betterhook` crate version that did the install.
    pub betterhook_version: String,
    /// Absolute path of the `betterhook` binary baked into the wrapper.
    pub betterhook_bin: String,
    /// Map from hook name (e.g. `"pre-commit"`) to `sha256:<hex>` of the
    /// wrapper bytes. `uninstall` refuses to remove any hook whose bytes
    /// don't match this SHA.
    pub hooks: BTreeMap<String, String>,
    /// If `install --takeover` unset a pre-existing `core.hooksPath`,
    /// this holds the previous value so `uninstall` can restore it.
    pub previous_core_hooks_path: Option<PathBuf>,
}
