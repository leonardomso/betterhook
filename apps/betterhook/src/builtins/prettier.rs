//! `prettier` builtin — wraps `prettier --check` and parses the stdout
//! line-per-file format into one diagnostic per unformatted file.
//!
//! Prettier's default `--check` output is:
//!
//! ```text
//! Checking formatting...
//! [warn] src/main.ts
//! [warn] src/Button.tsx
//! [warn] Code style issues found in 2 files. Run Prettier with --write to fix.
//! ```
//!
//! We pick out `[warn] <path>` lines whose path looks like a file (not
//! the trailing summary sentence).

use crate::runner::output::DiagnosticSeverity;

use super::{BuiltinId, BuiltinMeta, Diagnostic};

#[must_use]
pub fn meta() -> BuiltinMeta {
    BuiltinMeta {
        id: BuiltinId("prettier"),
        description: "JS/TS/CSS/MD formatter via `prettier --check`.",
        run: "prettier --check {staged_files}",
        fix: Some("prettier --write {files}"),
        glob: &[
            "*.js", "*.jsx", "*.ts", "*.tsx", "*.json", "*.css", "*.scss", "*.md", "*.yml",
            "*.yaml", "*.html",
        ],
        reads: &["**/*"],
        writes: &[],
        network: false,
        concurrent_safe: true,
        tool_binary: "prettier",
    }
}

#[must_use]
pub fn parse_output(stdout: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for raw in stdout.lines() {
        let line = raw.trim();
        let Some(rest) = line.strip_prefix("[warn] ") else {
            continue;
        };
        if rest.starts_with("Code style issues") || rest.starts_with("All matched files") {
            continue;
        }
        out.push(Diagnostic {
            file: rest.to_owned(),
            line: None,
            column: None,
            severity: DiagnosticSeverity::Warning,
            message: "formatting drift — run `prettier --write` to fix".to_owned(),
            rule: Some("prettier".to_owned()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_list() {
        let out = "Checking formatting...\n[warn] src/main.ts\n[warn] src/Button.tsx\n[warn] Code style issues found in 2 files.\n";
        let diags = parse_output(out);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].file, "src/main.ts");
        assert_eq!(diags[1].file, "src/Button.tsx");
    }

    #[test]
    fn clean_output_has_no_diagnostics() {
        let out = "Checking formatting...\nAll matched files use Prettier code style!\n";
        assert!(parse_output(out).is_empty());
    }
}
