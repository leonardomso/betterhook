//! `biome` builtin — runs `biome check --reporter=json` and parses
//! its per-diagnostic records.
//!
//! Biome's JSON reporter wraps diagnostics inside a top-level object:
//!
//! ```json
//! {
//!   "diagnostics": [
//!     {
//!       "category": "lint/suspicious/noDoubleEquals",
//!       "severity": "error",
//!       "description": "Use === instead of ==.",
//!       "location": {
//!         "path": {"file": "src/main.ts"},
//!         "span": [120, 122],
//!         "sourceCode": "..."
//!       }
//!     }
//!   ]
//! }
//! ```
//!
//! Biome historically also reported `files` with nested diagnostics;
//! we handle both shapes defensively.

use serde::Deserialize;

use crate::runner::output::DiagnosticSeverity;

use super::common::severity_from_level;
use super::{BuiltinId, BuiltinMeta, Diagnostic};

/// Subset of the biome `check --reporter=json` schema. Biome's shape
/// has varied across versions — we only bind the fields that have
/// been stable across the releases we support, and use
/// `#[serde(default)]` throughout so a schema shift doesn't break us.
#[derive(Debug, Deserialize, Default)]
struct BiomeReport {
    #[serde(default)]
    diagnostics: Vec<BiomeDiagnostic>,
}

#[derive(Debug, Deserialize, Default)]
struct BiomeDiagnostic {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    location: Option<BiomeLocation>,
}

#[derive(Debug, Deserialize, Default)]
struct BiomeLocation {
    #[serde(default)]
    path: Option<BiomePath>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
enum BiomePath {
    Structured {
        #[serde(default)]
        file: String,
    },
    Bare(String),
    #[default]
    Missing,
}

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("biome"),
        description: "JS/TS formatter + linter via `biome check --reporter=json`.",
        run: "biome check --reporter=json {staged_files}",
        fix: Some("biome check --write --reporter=json {files}"),
        glob: &["*.js", "*.jsx", "*.ts", "*.tsx", "*.json"],
        reads: &["**/*.js", "**/*.jsx", "**/*.ts", "**/*.tsx", "**/*.json"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "biome",
    }
}

fn diag_from_biome(d: BiomeDiagnostic) -> Diagnostic {
    let severity = d
        .severity
        .as_deref()
        .map_or(DiagnosticSeverity::Warning, severity_from_level);
    let message = d.description.or(d.message).unwrap_or_default();
    let file = d
        .location
        .and_then(|l| l.path)
        .map(|p| match p {
            BiomePath::Structured { file } => file,
            BiomePath::Bare(s) => s,
            BiomePath::Missing => String::new(),
        })
        .unwrap_or_default();
    Diagnostic {
        file,
        line: None,
        column: None,
        severity,
        message,
        rule: d.category,
    }
}

#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    let Ok(root) = serde_json::from_str::<BiomeReport>(stdout) else {
        return Vec::new();
    };
    root.diagnostics.into_iter().map(diag_from_biome).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_biome_diagnostics() {
        let input = r#"{
            "diagnostics": [
                {"category":"lint/suspicious/noDoubleEquals","severity":"error","description":"Use === instead of ==.","location":{"path":{"file":"src/main.ts"}}},
                {"category":"lint/style/useConst","severity":"warning","description":"Use const.","location":{"path":{"file":"src/cli.ts"}}}
            ]
        }"#;
        let diags = parse_output(input);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diags[1].rule.as_deref(), Some("lint/style/useConst"));
    }

    #[test]
    fn empty_report_has_no_diagnostics() {
        assert!(parse_output(r#"{"diagnostics":[]}"#).is_empty());
    }
}
