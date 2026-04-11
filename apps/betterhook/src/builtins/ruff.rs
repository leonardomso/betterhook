//! `ruff` builtin — runs `ruff check --output-format=json` and parses
//! each diagnostic into a [`Diagnostic`].
//!
//! Ruff's JSON schema is a top-level array of diagnostic objects:
//!
//! ```json
//! [
//!   {
//!     "code": "F401",
//!     "message": "`os` imported but unused",
//!     "filename": "/abs/src/main.py",
//!     "location": {"row": 3, "column": 1},
//!     "fix": {"message": "Remove unused import"}
//!   }
//! ]
//! ```

use serde_json::Value;

use crate::runner::output::DiagnosticSeverity;

use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("ruff"),
        description: "Python linter via `ruff check --output-format=json`.",
        run: "ruff check --output-format=json {staged_files}",
        fix: Some("ruff check --fix {files}"),
        glob: &["*.py", "*.pyi"],
        reads: &["**/*.py", "**/*.pyi", "**/pyproject.toml", "**/ruff.toml"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "ruff",
    }
}

#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let file = item
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let message = item
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let rule = item.get("code").and_then(Value::as_str).map(str::to_owned);
        let loc = item.get("location");
        let line = loc
            .and_then(|l| l.get("row"))
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        let column = loc
            .and_then(|l| l.get("column"))
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        out.push(Diagnostic {
            file,
            line,
            column,
            severity: DiagnosticSeverity::Warning,
            message,
            rule,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ruff_report() {
        let input = r#"[
            {"code":"F401","message":"`os` imported but unused","filename":"/abs/main.py","location":{"row":3,"column":1}},
            {"code":"E501","message":"Line too long","filename":"/abs/main.py","location":{"row":12,"column":88}}
        ]"#;
        let diags = parse_output(input);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].rule.as_deref(), Some("F401"));
        assert_eq!(diags[0].line, Some(3));
        assert_eq!(diags[1].column, Some(88));
    }

    #[test]
    fn empty_report_yields_no_diagnostics() {
        assert!(parse_output("[]").is_empty());
    }
}
