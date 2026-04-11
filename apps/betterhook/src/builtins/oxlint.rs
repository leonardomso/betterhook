//! `oxlint` builtin — runs `oxlint --format=json` and converts its
//! eslint-compatible JSON output into diagnostics.
//!
//! oxlint deliberately mirrors eslint's JSON schema (top-level array of
//! per-file objects with a `messages` array), so the parser reuses the
//! same logic as the eslint builtin but keyed on `"oxlint"` severity
//! integers.

use serde_json::Value;

use crate::runner::output::DiagnosticSeverity;

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
