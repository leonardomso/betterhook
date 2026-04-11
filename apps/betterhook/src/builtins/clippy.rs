//! `clippy` builtin — runs `cargo clippy --message-format=json` and
//! parses each JSON line into a [`Diagnostic`].
//!
//! clippy (and the cargo compiler-message wire format in general)
//! emits one JSON object per line; the records we care about have
//! `"reason": "compiler-message"` and a nested `message` object that
//! follows the [rustc JSON diagnostic schema][rustc-json]. We read only
//! the fields we need — file, line, column, level, message, code — so
//! we don't take a hard dependency on a rustc-specific type crate.
//!
//! [rustc-json]: https://doc.rust-lang.org/rustc/json.html

use serde_json::Value;

use crate::runner::output::DiagnosticSeverity;

use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("clippy"),
        description: "Rust lints via `cargo clippy --message-format=json`.",
        run: "cargo clippy --workspace --all-targets --message-format=json -- -D warnings",
        fix: Some("cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged"),
        glob: &["*.rs"],
        reads: &["**/*.rs", "**/Cargo.toml", "**/Cargo.lock"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "cargo",
    }
}

/// Parse a single JSON line from `cargo --message-format=json`. Returns
/// `None` for records that aren't compiler messages or that don't carry
/// a primary span we can attribute to a file.
#[must_use]
pub fn parse_line(line: &str) -> Option<Diagnostic> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("reason")?.as_str()? != "compiler-message" {
        return None;
    }
    let msg = v.get("message")?;
    let level = msg.get("level")?.as_str()?;
    let severity = match level {
        "error" | "error: internal compiler error" => DiagnosticSeverity::Error,
        "warning" => DiagnosticSeverity::Warning,
        "note" | "help" => DiagnosticSeverity::Info,
        _ => DiagnosticSeverity::Hint,
    };
    let message = msg.get("message")?.as_str()?.to_owned();
    let rule = msg
        .get("code")
        .and_then(|c| c.get("code"))
        .and_then(|c| c.as_str())
        .map(str::to_owned);

    let spans = msg.get("spans")?.as_array()?;
    let primary = spans
        .iter()
        .find(|s| s.get("is_primary").and_then(Value::as_bool).unwrap_or(false))
        .or_else(|| spans.first())?;
    let file = primary.get("file_name")?.as_str()?.to_owned();
    let line_num = primary
        .get("line_start")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());
    let column = primary
        .get("column_start")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());

    Some(Diagnostic {
        file,
        line: line_num,
        column,
        severity,
        message,
        rule,
    })
}

/// Parse every line in `stdout`. Non-JSON and non-compiler-message
/// lines are silently skipped — cargo also emits `build-script-executed`
/// and `compiler-artifact` records we don't care about here.
#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    stdout.lines().filter_map(parse_line).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_warning_with_span() {
        let line = r#"{"reason":"compiler-message","package_id":"p","target":{},"message":{"message":"unused variable: `x`","code":{"code":"unused_variables","explanation":null},"level":"warning","spans":[{"file_name":"src/main.rs","byte_start":10,"byte_end":11,"line_start":3,"line_end":3,"column_start":9,"column_end":10,"is_primary":true,"text":[]}],"children":[],"rendered":"warning: unused variable: `x`"}}"#;
        let d = parse_line(line).expect("diagnostic parses");
        assert_eq!(d.file, "src/main.rs");
        assert_eq!(d.line, Some(3));
        assert_eq!(d.column, Some(9));
        assert_eq!(d.severity, DiagnosticSeverity::Warning);
        assert_eq!(d.rule.as_deref(), Some("unused_variables"));
        assert!(d.message.contains("unused variable"));
    }

    #[test]
    fn skips_non_compiler_messages() {
        assert!(
            parse_line(r#"{"reason":"compiler-artifact","package_id":"p"}"#).is_none()
        );
        assert!(parse_line("not-json").is_none());
    }

    #[test]
    fn parse_output_collects_multiple_messages() {
        let a = r#"{"reason":"compiler-message","message":{"message":"m1","level":"error","spans":[{"file_name":"a.rs","line_start":1,"column_start":1,"is_primary":true}]}}"#;
        let b = r#"{"reason":"compiler-message","message":{"message":"m2","level":"warning","spans":[{"file_name":"b.rs","line_start":2,"column_start":2,"is_primary":true}]}}"#;
        let out = format!("{a}\n{b}\n");
        let diags = parse_output(&out);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diags[1].severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn meta_is_concurrent_safe() {
        let m = meta();
        assert!(m.concurrent_safe);
        assert!(!m.network);
    }
}
