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

use serde_json::Value;

use crate::runner::output::DiagnosticSeverity;

use super::common::severity_from_level;
use super::{BuiltinId, BuiltinMeta, Diagnostic};

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
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(stdout) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let file = item
            .get("file")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let line = item
            .get("line")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        let column = item
            .get("column")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        let severity = item
            .get("level")
            .and_then(Value::as_str)
            .map_or(DiagnosticSeverity::Warning, severity_from_level);
        let message = item
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let rule = item
            .get("code")
            .and_then(Value::as_u64)
            .map(|c| format!("SC{c}"));
        out.push(Diagnostic {
            file,
            line,
            column,
            severity,
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
    fn parses_shellcheck_report() {
        let input = r#"[{"file":"install.sh","line":14,"column":5,"level":"warning","code":2086,"message":"Double quote to prevent globbing."}]"#;
        let diags = parse_output(input);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule.as_deref(), Some("SC2086"));
        assert_eq!(diags[0].severity, DiagnosticSeverity::Warning);
    }
}
