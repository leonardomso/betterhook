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

use super::common::parse_file_list;
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
    parse_file_list(
        stdout,
        "[warn] ",
        &["Code style issues", "All matched files"],
        "formatting drift — run `prettier --write` to fix",
        "prettier",
    )
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
