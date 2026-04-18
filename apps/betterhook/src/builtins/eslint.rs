//! `eslint` builtin — runs `eslint --format=json` and turns each
//! message into a [`Diagnostic`].
//!
//! eslint's JSON schema is a top-level array of per-file results:
//!
//! ```json
//! [
//!   {
//!     "filePath": "/abs/path/src/main.ts",
//!     "messages": [
//!       {
//!         "ruleId": "no-unused-vars",
//!         "severity": 2,
//!         "message": "'x' is assigned a value but never used.",
//!         "line": 3,
//!         "column": 7
//!       }
//!     ],
//!     "errorCount": 1,
//!     "warningCount": 0
//!   }
//! ]
//! ```
//!
//! Severity `2` → error, `1` → warning. We parse defensively because
//! some eslint plugins emit messages with `line`/`column` missing.

use super::common::parse_eslint_json;
use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("eslint"),
        description: "JS/TS linter via `eslint --format=json`.",
        run: "eslint --format=json {staged_files}",
        fix: Some("eslint --fix {files}"),
        glob: &["*.js", "*.jsx", "*.ts", "*.tsx", "*.mjs", "*.cjs"],
        reads: &["**/*.js", "**/*.jsx", "**/*.ts", "**/*.tsx"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "eslint",
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
    fn parses_eslint_json_report() {
        let input = r#"[
            {
                "filePath": "/abs/src/main.ts",
                "messages": [
                    {"ruleId": "no-unused-vars", "severity": 2, "message": "'x' is assigned a value but never used.", "line": 3, "column": 7},
                    {"ruleId": "semi", "severity": 1, "message": "Missing semicolon.", "line": 10, "column": 42}
                ],
                "errorCount": 1,
                "warningCount": 1
            }
        ]"#;
        let diags = parse_output(input);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diags[0].rule.as_deref(), Some("no-unused-vars"));
        assert_eq!(diags[0].line, Some(3));
        assert_eq!(diags[1].severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn clean_report_has_no_diagnostics() {
        let input = r#"[{"filePath":"a.ts","messages":[]}]"#;
        assert!(parse_output(input).is_empty());
    }

    #[test]
    fn invalid_json_is_silently_empty() {
        assert!(parse_output("not json").is_empty());
    }
}
