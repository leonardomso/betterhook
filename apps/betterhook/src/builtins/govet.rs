//! `go vet` builtin — parses `file:line:col: message` stderr lines
//! into per-location diagnostics.
//!
//! `go vet` doesn't ship a JSON reporter in the default toolchain, so
//! this parser is a plain line-by-line split on the conventional
//! `path:line:col: message` shape. Lines that don't match that shape
//! (build errors, pkg summaries) are dropped.

use crate::runner::output::DiagnosticSeverity;

use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("govet"),
        description: "Go suspicious construct check via `go vet ./...`.",
        run: "go vet ./...",
        fix: None,
        glob: &["*.go"],
        reads: &["**/*.go", "**/go.mod", "**/go.sum"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "go",
    }
}

#[must_use]
pub fn parse_output(stderr: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for raw in stderr.lines() {
        let Some(diag) = parse_line(raw) else {
            continue;
        };
        out.push(diag);
    }
    out
}

fn parse_line(line: &str) -> Option<Diagnostic> {
    let trimmed = line.trim_start();
    // Format: `./path/to/foo.go:12:4: message text`
    // or:     `path/foo.go:12: message text`
    let mut parts = trimmed.splitn(4, ':');
    let file = parts.next()?.to_owned();
    let line_num = parts.next()?.trim().parse::<u32>().ok()?;
    let maybe_col = parts.next()?;
    // The third segment is either a column number or the start of
    // the message (when no column is present).
    let (column, message_start) = match maybe_col.trim().parse::<u32>() {
        Ok(c) => (Some(c), parts.next()?),
        Err(_) => (None, maybe_col),
    };
    let message = message_start.trim().to_owned();
    if file.is_empty() || message.is_empty() {
        return None;
    }
    Some(Diagnostic {
        file,
        line: Some(line_num),
        column,
        severity: DiagnosticSeverity::Warning,
        message,
        rule: Some("govet".to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vet_with_column() {
        let out = "./cmd/main.go:12:4: Println call has possible formatting directive %v\n";
        let diags = parse_output(out);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].file, "./cmd/main.go");
        assert_eq!(diags[0].line, Some(12));
        assert_eq!(diags[0].column, Some(4));
    }

    #[test]
    fn parses_vet_without_column() {
        let out = "cmd/main.go:7: unreachable code\n";
        let diags = parse_output(out);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].line, Some(7));
        assert_eq!(diags[0].column, None);
    }

    #[test]
    fn ignores_unrelated_lines() {
        let out = "# example.com/foo\nrandom garbage\n";
        assert!(parse_output(out).is_empty());
    }
}
