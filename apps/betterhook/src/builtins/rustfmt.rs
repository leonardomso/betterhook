//! `rustfmt` builtin — checks formatting via `cargo fmt --check`.
//!
//! We can't use a `cargo fmt --check <file>` form reliably because the
//! underlying `rustfmt` binary interprets the trailing args as rustfmt
//! flags, so we run `cargo fmt --all -- --check` and parse the
//! `Diff in <path>` blocks rustfmt emits. Each diff block becomes one
//! `Diagnostic` keyed on the file path.

use crate::runner::output::DiagnosticSeverity;

use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("rustfmt"),
        description: "Check Rust formatting with `cargo fmt --check`.",
        run: "cargo fmt --all -- --check",
        fix: Some("cargo fmt --all"),
        glob: &["*.rs"],
        reads: &["**/*.rs"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "cargo",
    }
}

/// Parse `cargo fmt -- --check` stdout into one diagnostic per file
/// with formatting drift.
#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("Diff in ") {
            // Format: `Diff in /abs/path/to/file.rs at line 12:`
            let file = rest
                .split(" at line ")
                .next()
                .unwrap_or(rest)
                .trim_end_matches(':')
                .to_owned();
            let line_num = rest
                .split(" at line ")
                .nth(1)
                .and_then(|s| s.trim_end_matches(':').parse::<u32>().ok());
            out.push(Diagnostic {
                file,
                line: line_num,
                column: None,
                severity: DiagnosticSeverity::Warning,
                message: "formatting drift — run `cargo fmt` to fix".to_owned(),
                rule: Some("rustfmt".to_owned()),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_diff_block() {
        let out = "Diff in /repo/src/main.rs at line 12:\n-old\n+new\n";
        let diags = parse_output(out);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].file, "/repo/src/main.rs");
        assert_eq!(diags[0].line, Some(12));
        assert_eq!(diags[0].rule.as_deref(), Some("rustfmt"));
    }

    #[test]
    fn returns_empty_for_clean_output() {
        assert!(parse_output("").is_empty());
        assert!(parse_output("no drift\nhello\n").is_empty());
    }

    #[test]
    fn meta_is_concurrent_safe() {
        let m = meta();
        assert!(m.concurrent_safe);
        assert!(!m.network);
    }
}
