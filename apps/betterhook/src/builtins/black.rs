//! `black` builtin — wraps `black --check` and extracts the file list
//! black prints for files needing reformatting.
//!
//! `black --check` emits one `would reformat <path>` line per drifted
//! file on stderr, followed by a summary line. We look for the
//! `would reformat ` prefix and turn each match into one diagnostic.

use crate::runner::output::DiagnosticSeverity;

use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("black"),
        description: "Python formatter via `black --check`.",
        run: "black --check {staged_files}",
        fix: Some("black {files}"),
        glob: &["*.py", "*.pyi"],
        reads: &["**/*.py", "**/*.pyi"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "black",
    }
}

#[must_use]
pub fn parse_output(stdout_or_stderr: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for raw in stdout_or_stderr.lines() {
        let line = raw.trim();
        let Some(path) = line.strip_prefix("would reformat ") else {
            continue;
        };
        out.push(Diagnostic {
            file: path.to_owned(),
            line: None,
            column: None,
            severity: DiagnosticSeverity::Warning,
            message: "formatting drift — run `black` to fix".to_owned(),
            rule: Some("black".to_owned()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_would_reformat_lines() {
        let out = "would reformat src/main.py\nwould reformat src/cli.py\nOh no! 2 files would be reformatted.\n";
        let diags = parse_output(out);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].file, "src/main.py");
        assert_eq!(diags[1].rule.as_deref(), Some("black"));
    }

    #[test]
    fn clean_output_has_no_diagnostics() {
        assert!(parse_output("All done!\n1 file would be left unchanged.\n").is_empty());
    }
}
