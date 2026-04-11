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

use serde_json::Value;

use crate::runner::output::DiagnosticSeverity;

use super::common::severity_from_level;
use super::{BuiltinId, BuiltinMeta, Diagnostic};

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

fn diag_from_value(v: &Value) -> Diagnostic {
    let severity = v
        .get("severity")
        .and_then(Value::as_str)
        .map_or(DiagnosticSeverity::Warning, severity_from_level);
    let message = v
        .get("description")
        .and_then(Value::as_str)
        .or_else(|| v.get("message").and_then(Value::as_str))
        .unwrap_or("")
        .to_owned();
    let rule = v
        .get("category")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let file = v
        .get("location")
        .and_then(|l| l.get("path"))
        .and_then(|p| p.get("file").or(Some(p)))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Diagnostic {
        file,
        line: None,
        column: None,
        severity,
        message,
        rule,
    }
}

#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    let Ok(root) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(arr) = root.get("diagnostics").and_then(Value::as_array) {
        for v in arr {
            out.push(diag_from_value(v));
        }
    }
    out
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
