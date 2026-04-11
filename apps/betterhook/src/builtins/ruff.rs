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

use serde::Deserialize;

use crate::runner::output::DiagnosticSeverity;

use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[derive(Debug, Deserialize, Default)]
struct RuffItem {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    location: Option<RuffLocation>,
}

#[derive(Debug, Deserialize, Default)]
struct RuffLocation {
    #[serde(default)]
    row: Option<u64>,
    #[serde(default)]
    column: Option<u64>,
}

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
    let Ok(items) = serde_json::from_str::<Vec<RuffItem>>(stdout) else {
        return Vec::new();
    };
    items
        .into_iter()
        .map(|item| {
            let (line, column) = item.location.map_or((None, None), |l| {
                (
                    l.row.and_then(|n| u32::try_from(n).ok()),
                    l.column.and_then(|n| u32::try_from(n).ok()),
                )
            });
            Diagnostic {
                file: item.filename,
                line,
                column,
                severity: DiagnosticSeverity::Warning,
                message: item.message,
                rule: item.code,
            }
        })
        .collect()
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
