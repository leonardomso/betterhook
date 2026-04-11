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

use serde_json::Value;

use crate::runner::output::DiagnosticSeverity;

use super::{BuiltinId, BuiltinMeta, Diagnostic};

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
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let file = item
            .get("File")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let line = item
            .get("StartLine")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        let column = item
            .get("StartColumn")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        let rule = item
            .get("RuleID")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let description = item
            .get("Description")
            .and_then(Value::as_str)
            .unwrap_or("secret detected");
        let message = format!("{description} — remove before committing");
        out.push(Diagnostic {
            file,
            line,
            column,
            severity: DiagnosticSeverity::Error,
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
