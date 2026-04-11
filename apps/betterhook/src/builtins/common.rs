//! Shared helpers used by multiple builtin parsers.
//!
//! The builtin modules were originally copy-pasted from one another
//! and drifted slightly on severity mapping, file-list parsing, and
//! eslint-compatible JSON walking. This module is the single source
//! of truth for the three shapes we see repeatedly:
//!
//! 1. **File-list output** — one path per line with an optional
//!    prefix (rustfmt's `Diff in`, prettier's `[warn]`, black's
//!    `would reformat`, gofmt's bare path). [`parse_file_list`]
//!    accepts the prefix and a message template and emits one
//!    [`Diagnostic`] per matched line.
//!
//! 2. **eslint-compatible JSON** — a top-level array of per-file
//!    objects with a `messages` array of `{ruleId, severity:int,
//!    message, line, column}`. [`parse_eslint_json`] handles both
//!    eslint and oxlint since oxlint deliberately mirrors the shape.
//!
//! 3. **Level-to-severity mapping** — tools spell severities as
//!    strings or ints, and each parser did its own mapping. The two
//!    functions [`severity_from_level`] and [`severity_from_code`]
//!    are the shared source of truth.

use serde_json::Value;

use crate::runner::output::DiagnosticSeverity;

use super::Diagnostic;

/// Map a string severity ("error", "warning", "info", "note", "help",
/// "fatal") to the shared [`DiagnosticSeverity`] taxonomy.
#[must_use]
pub fn severity_from_level(s: &str) -> DiagnosticSeverity {
    match s.to_ascii_lowercase().as_str() {
        "error" | "fatal" => DiagnosticSeverity::Error,
        "warning" | "warn" => DiagnosticSeverity::Warning,
        "info" | "information" | "note" | "help" => DiagnosticSeverity::Info,
        _ => DiagnosticSeverity::Hint,
    }
}

/// Map an integer severity code as used by eslint/oxlint (2 → error,
/// 1 → warning, anything else → info) to [`DiagnosticSeverity`].
#[must_use]
pub fn severity_from_code(code: i64) -> DiagnosticSeverity {
    match code {
        2 => DiagnosticSeverity::Error,
        1 => DiagnosticSeverity::Warning,
        _ => DiagnosticSeverity::Info,
    }
}

/// Parse a "one file per line" stdout chunk into one diagnostic per
/// matched line. Lines without `prefix` are ignored.
///
/// * `prefix` — the literal prefix (e.g. `"would reformat "`) or `""`
///   for bare-path output like gofmt. Leading whitespace is trimmed
///   before the prefix check.
/// * `skip_if_contains` — if a line (after stripping the prefix) starts
///   with any of these substrings, it's treated as a summary line and
///   skipped. Use this to filter out prettier/black's trailing
///   "X files would be reformatted" sentence.
/// * `message` — the static text to attach to each diagnostic.
/// * `rule` — the rule identifier (typically the tool name).
#[must_use]
pub fn parse_file_list(
    stdout: &str,
    prefix: &str,
    skip_if_starts_with: &[&str],
    message: &str,
    rule: &str,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for raw in stdout.lines() {
        let line = raw.trim();
        let remainder = if prefix.is_empty() {
            if line.is_empty() {
                continue;
            }
            line
        } else {
            let Some(rest) = line.strip_prefix(prefix) else {
                continue;
            };
            rest
        };
        if skip_if_starts_with.iter().any(|s| remainder.starts_with(s)) {
            continue;
        }
        out.push(Diagnostic {
            file: remainder.to_owned(),
            line: None,
            column: None,
            severity: DiagnosticSeverity::Warning,
            message: message.to_owned(),
            rule: Some(rule.to_owned()),
        });
    }
    out
}

/// Parse an eslint-compatible JSON report (top-level array of per-file
/// objects, each with a `messages` array). Used by both `eslint` and
/// `oxlint`, which emit identical schemas.
#[must_use]
pub fn parse_eslint_json(stdout: &str) -> Vec<Diagnostic> {
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
            let rule = msg.get("ruleId").and_then(Value::as_str).map(str::to_owned);
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
    fn level_mapping_canonical() {
        assert_eq!(severity_from_level("error"), DiagnosticSeverity::Error);
        assert_eq!(severity_from_level("FATAL"), DiagnosticSeverity::Error);
        assert_eq!(severity_from_level("warning"), DiagnosticSeverity::Warning);
        assert_eq!(severity_from_level("info"), DiagnosticSeverity::Info);
        assert_eq!(severity_from_level("note"), DiagnosticSeverity::Info);
        assert_eq!(severity_from_level("???"), DiagnosticSeverity::Hint);
    }

    #[test]
    fn code_mapping_canonical() {
        assert_eq!(severity_from_code(2), DiagnosticSeverity::Error);
        assert_eq!(severity_from_code(1), DiagnosticSeverity::Warning);
        assert_eq!(severity_from_code(0), DiagnosticSeverity::Info);
        assert_eq!(severity_from_code(5), DiagnosticSeverity::Info);
    }

    #[test]
    fn parse_file_list_handles_prefix_and_summary() {
        let out = "[warn] a.ts\n[warn] b.ts\n[warn] Code style issues found in 2 files.\n";
        let diags = parse_file_list(out, "[warn] ", &["Code style issues"], "msg", "prettier");
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].file, "a.ts");
        assert_eq!(diags[1].file, "b.ts");
    }

    #[test]
    fn parse_file_list_handles_bare_paths() {
        let out = "cmd/main.go\ninternal/foo.go\n";
        let diags = parse_file_list(out, "", &[], "msg", "gofmt");
        assert_eq!(diags.len(), 2);
    }

    #[test]
    fn parse_eslint_json_reads_severity_and_spans() {
        let input = r#"[{"filePath":"/a.ts","messages":[{"ruleId":"x","severity":2,"message":"m","line":1,"column":2}]}]"#;
        let diags = parse_eslint_json(input);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diags[0].line, Some(1));
        assert_eq!(diags[0].column, Some(2));
    }
}
