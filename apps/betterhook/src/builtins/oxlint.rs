//! `oxlint` builtin — runs `oxlint --format=json` and converts its
//! eslint-compatible JSON output into diagnostics.
//!
//! oxlint deliberately mirrors eslint's JSON schema (top-level array of
//! per-file objects with a `messages` array), so the parser reuses the
//! same logic as the eslint builtin but keyed on `"oxlint"` severity
//! integers.

use super::common::parse_eslint_json;
use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("oxlint"),
        description: "Very fast JS/TS linter (Rust) via `oxlint --format=json`.",
        run: "oxlint --format=json {staged_files}",
        fix: Some("oxlint --fix {files}"),
        glob: &["*.js", "*.jsx", "*.ts", "*.tsx", "*.mjs", "*.cjs"],
        reads: &["**/*.js", "**/*.jsx", "**/*.ts", "**/*.tsx"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "oxlint",
    }
}

#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    parse_eslint_json(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::output::DiagnosticSeverity;

    #[test]
    fn parses_oxlint_report() {
        let input = r#"[{
            "filePath":"/abs/main.ts",
            "messages":[
                {"ruleId":"no-unused-vars","severity":2,"message":"unused","line":1,"column":1}
            ]
        }]"#;
        let diags = parse_output(input);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diags[0].line, Some(1));
    }

    #[test]
    fn empty_output_has_no_diagnostics() {
        assert!(parse_output("[]").is_empty());
    }
}
