//! `gitleaks` builtin — runs `gitleaks protect --staged --report-format=json`
//! and surfaces each finding as an Error-severity diagnostic.
//!
//! gitleaks reports a top-level array of finding objects:
//!
//! ```json
//! [
//!   {
//!     "RuleID":"aws-access-key",
//!     "Description":"AWS Access Key",
//!     "File":"deploy/secrets.env",
//!     "StartLine":4,
//!     "StartColumn":9,
//!     "Match":"AKIA..."
//!   }
//! ]
//! ```

use serde::Deserialize;

use crate::runner::output::DiagnosticSeverity;

use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[derive(Debug, Deserialize, Default)]
struct GitleaksFinding {
    #[serde(default, rename = "RuleID")]
    rule_id: Option<String>,
    #[serde(default, rename = "Description")]
    description: Option<String>,
    #[serde(default, rename = "File")]
    file: String,
    #[serde(default, rename = "StartLine")]
    start_line: Option<u64>,
    #[serde(default, rename = "StartColumn")]
    start_column: Option<u64>,
}

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("gitleaks"),
        description: "Secret scanner via `gitleaks protect --staged --report-format=json`.",
        run: "gitleaks protect --staged --report-format=json --redact -v",
        fix: None,
        glob: &[],
        reads: &["**/*"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "gitleaks",
    }
}

#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    let Ok(items) = serde_json::from_str::<Vec<GitleaksFinding>>(stdout) else {
        return Vec::new();
    };
    items
        .into_iter()
        .map(|item| {
            let description = item
                .description
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("secret detected");
            Diagnostic {
                file: item.file,
                line: item.start_line.and_then(|n| u32::try_from(n).ok()),
                column: item.start_column.and_then(|n| u32::try_from(n).ok()),
                severity: DiagnosticSeverity::Error,
                message: format!("{description} — remove before committing"),
                rule: item.rule_id,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_leak_finding() {
        let input = r#"[{"RuleID":"aws-access-key","Description":"AWS Access Key","File":"deploy/secrets.env","StartLine":4,"StartColumn":9}]"#;
        let diags = parse_output(input);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diags[0].rule.as_deref(), Some("aws-access-key"));
        assert!(diags[0].message.contains("AWS Access Key"));
    }

    #[test]
    fn empty_report_has_no_diagnostics() {
        assert!(parse_output("[]").is_empty());
    }
}
