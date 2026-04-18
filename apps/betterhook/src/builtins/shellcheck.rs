//! `shellcheck` builtin — runs `shellcheck --format=json` and parses
//! each comment record into a [`Diagnostic`].
//!
//! shellcheck's JSON schema is a top-level array where each item looks
//! like:
//!
//! ```json
//! {
//!   "file":"install.sh",
//!   "line":14,"column":5,"endLine":14,"endColumn":20,
//!   "level":"warning",
//!   "code":2086,
//!   "message":"Double quote to prevent globbing and word splitting."
//! }
//! ```

use serde::Deserialize;

use crate::runner::output::DiagnosticSeverity;

use super::common::severity_from_level;
use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[derive(Debug, Deserialize, Default)]
struct ShellcheckItem {
    #[serde(default)]
    file: String,
    #[serde(default)]
    line: Option<u64>,
    #[serde(default)]
    column: Option<u64>,
    #[serde(default)]
    level: String,
    #[serde(default)]
    code: Option<u64>,
    #[serde(default)]
    message: String,
}

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("shellcheck"),
        description: "Shell lint via `shellcheck --format=json`.",
        run: "shellcheck --format=json {staged_files}",
        fix: None,
        glob: &["*.sh", "*.bash"],
        reads: &["**/*.sh", "**/*.bash"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "shellcheck",
    }
}

#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    let Ok(items) = serde_json::from_str::<Vec<ShellcheckItem>>(stdout) else {
        return Vec::new();
    };
    items
        .into_iter()
        .map(|item| Diagnostic {
            file: item.file,
            line: item.line.and_then(|n| u32::try_from(n).ok()),
            column: item.column.and_then(|n| u32::try_from(n).ok()),
            severity: if item.level.is_empty() {
                DiagnosticSeverity::Warning
            } else {
                severity_from_level(&item.level)
            },
            message: item.message,
            rule: item.code.map(|c| format!("SC{c}")),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shellcheck_report() {
        let input = r#"[{"file":"install.sh","line":14,"column":5,"level":"warning","code":2086,"message":"Double quote to prevent globbing."}]"#;
        let diags = parse_output(input);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule.as_deref(), Some("SC2086"));
        assert_eq!(diags[0].severity, DiagnosticSeverity::Warning);
    }
}
