//! `gofmt` builtin — wraps `gofmt -l` (list only) and turns each file
//! in the list into a formatting-drift diagnostic.
//!
//! `gofmt -l <files>` prints one path per line for any file that would
//! be changed by `gofmt -w`, so the parser is a trimmed-line filter.

use crate::runner::output::DiagnosticSeverity;

use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("gofmt"),
        description: "Go formatter via `gofmt -l` (list drifted files).",
        run: "gofmt -l {staged_files}",
        fix: Some("gofmt -w {files}"),
        glob: &["*.go"],
        reads: &["**/*.go"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "gofmt",
    }
}

#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for raw in stdout.lines() {
        let path = raw.trim();
        if path.is_empty() {
            continue;
        }
        out.push(Diagnostic {
            file: path.to_owned(),
            line: None,
            column: None,
            severity: DiagnosticSeverity::Warning,
            message: "formatting drift — run `gofmt -w` to fix".to_owned(),
            rule: Some("gofmt".to_owned()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_list() {
        let diags = parse_output("cmd/main.go\ninternal/foo/bar.go\n");
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].file, "cmd/main.go");
    }

    #[test]
    fn empty_output_has_no_diagnostics() {
        assert!(parse_output("").is_empty());
    }
}
