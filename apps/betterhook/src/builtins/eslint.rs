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

use serde_json::Value;

use crate::runner::output::DiagnosticSeverity;

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

fn severity_from_code(code: i64) -> DiagnosticSeverity {
    match code {
        2 => DiagnosticSeverity::Error,
        1 => DiagnosticSeverity::Warning,
        _ => DiagnosticSeverity::Info,
    }
}

#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    let Ok(Value::Array(files)) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for file in files {
        let file_path = file
            .get("filePath")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let Some(messages) = file.get("messages").and_then(Value::as_array) else {
            continue;
        };
        for msg in messages {
            let severity = msg
                .get("severity")
                .and_then(Value::as_i64)
                .map_or(DiagnosticSeverity::Warning, severity_from_code);
            let message = msg
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let rule = msg
                .get("ruleId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let line = msg
                .get("line")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok());
            let column = msg
                .get("column")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok());
            out.push(Diagnostic {
                file: file_path.clone(),
                line,
                column,
                severity,
                message,
                rule,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
