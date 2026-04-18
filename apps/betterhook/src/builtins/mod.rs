//! Builtin linter/formatter wrappers.
//!
//! A builtin is a named bundle that gives a user a one-line `builtin =
//! "<name>"` shortcut for a common tool (rustfmt, clippy, eslint, ruff,
//! etc.). Each builtin exposes:
//!
//!   1. A default `Job` template with capability fields already set
//!      (`reads`, `writes`, `network`, `concurrent_safe`) so the DAG
//!      resolver can reason about it out of the box.
//!   2. A parser that turns the tool's native output into
//!      [`OutputEvent::Diagnostic`] records, so agents reading the
//!      NDJSON sink get structured results instead of free-form text.
//!
//! Phases 41–49 add one file per tool. This module is the registry that
//! `config::schema::Job::builtin` will look up, plus the `parse_lines`
//! helper every parser uses.

use std::collections::BTreeMap;

use crate::runner::output::DiagnosticSeverity;

pub mod biome;
pub mod black;
pub mod clippy;
pub mod common;
pub mod eslint;
pub mod gitleaks;
pub mod gofmt;
pub mod govet;
pub mod oxlint;
pub mod prettier;
pub mod ruff;
pub mod rustfmt;
pub mod shellcheck;

/// Opaque reference used by `config::Job::builtin = "<name>"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinId(pub &'static str);

/// Metadata surfaced by `betterhook builtins list` (phase 49) and the
/// `doctor` tool-availability probe (phase 50).
#[derive(Debug, Clone)]
pub struct BuiltinMeta {
    pub id: BuiltinId,
    /// Short human-readable description, one line.
    pub description: &'static str,
    /// Default `run` template. Supports the same `{staged_files}` /
    /// `{files}` placeholder vocabulary the runner already understands.
    pub run: &'static str,
    /// Optional `fix` template.
    pub fix: Option<&'static str>,
    /// Default glob patterns scoping which files the builtin runs on.
    pub glob: &'static [&'static str],
    /// Read/write/network/concurrent-safe capability defaults.
    pub reads: &'static [&'static str],
    pub writes: &'static [&'static str],
    pub network: bool,
    pub concurrent_safe: bool,
    /// The tool binary name `doctor` looks up on `PATH`.
    pub tool_binary: &'static str,
}

/// Produced by every builtin parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub rule: Option<String>,
}

/// Static registry of every builtin. Callers look up by the string
/// they see in `Job::builtin`. Populated by each per-tool module's
/// `meta()` constructor.
#[must_use]
pub fn registry() -> BTreeMap<&'static str, BuiltinMeta> {
    let mut map = BTreeMap::new();
    let items = [
        rustfmt::meta(),
        clippy::meta(),
        prettier::meta(),
        eslint::meta(),
        ruff::meta(),
        black::meta(),
        gofmt::meta(),
        govet::meta(),
        biome::meta(),
        oxlint::meta(),
        shellcheck::meta(),
        gitleaks::meta(),
    ];
    for m in items {
        map.insert(m.id.0, m);
    }
    map
}

/// Look up a builtin by name.
#[must_use]
pub fn get(name: &str) -> Option<BuiltinMeta> {
    registry().get(name).cloned()
}

/// Names of every registered builtin, sorted.
#[must_use]
pub fn names() -> Vec<&'static str> {
    registry().keys().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_includes_rustfmt_and_clippy() {
        let r = registry();
        assert!(r.contains_key("rustfmt"));
        assert!(r.contains_key("clippy"));
    }

    #[test]
    fn get_returns_none_for_unknown() {
        assert!(get("nope").is_none());
    }
}
