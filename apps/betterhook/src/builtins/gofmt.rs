//! `gofmt` builtin — wraps `gofmt -l` (list only) and turns each file
//! in the list into a formatting-drift diagnostic.
//!
//! `gofmt -l <files>` prints one path per line for any file that would
//! be changed by `gofmt -w`, so the parser is a trimmed-line filter.

use super::common::parse_file_list;
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
    parse_file_list(
        stdout,
        "",
        &[],
        "formatting drift — run `gofmt -w` to fix",
        "gofmt",
    )
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
