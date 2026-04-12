//! Comprehensive tests for all 12 builtin linter/formatter wrappers.
//!
//! Each builtin gets: a real-world output sample → correct parse, empty
//! output → zero diagnostics, malformed output → no panic, and `meta()`
//! returns valid defaults. Plus registry-level and severity-mapping tests.

use betterhook::builtins::{self, common};
use betterhook::runner::output::DiagnosticSeverity;

// ────────────────────────── registry tests ───────────────────────────

#[test]
fn registry_has_exactly_12_builtins() {
    assert_eq!(builtins::registry().len(), 12);
}

#[test]
fn every_builtin_has_unique_name() {
    let names = builtins::names();
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(names.len(), unique.len());
}

#[test]
fn every_builtin_has_nonempty_run() {
    for meta in builtins::registry().values() {
        assert!(!meta.run.is_empty(), "{} has empty run", meta.id.0);
    }
}

#[test]
fn every_builtin_has_nonempty_tool_binary() {
    for meta in builtins::registry().values() {
        assert!(
            !meta.tool_binary.is_empty(),
            "{} has empty tool_binary",
            meta.id.0
        );
    }
}

#[test]
fn get_returns_none_for_unknown() {
    assert!(builtins::get("nonexistent-tool-xyz").is_none());
}

#[test]
fn get_returns_some_for_all_known() {
    for name in builtins::names() {
        assert!(builtins::get(name).is_some(), "get({name}) returned None");
    }
}

// ────────────────── severity mapping (shared helpers) ─────────────────

#[test]
fn severity_from_level_covers_all_known_strings() {
    assert_eq!(common::severity_from_level("error"), DiagnosticSeverity::Error);
    assert_eq!(common::severity_from_level("fatal"), DiagnosticSeverity::Error);
    assert_eq!(common::severity_from_level("ERROR"), DiagnosticSeverity::Error);
    assert_eq!(common::severity_from_level("warning"), DiagnosticSeverity::Warning);
    assert_eq!(common::severity_from_level("warn"), DiagnosticSeverity::Warning);
    assert_eq!(common::severity_from_level("info"), DiagnosticSeverity::Info);
    assert_eq!(common::severity_from_level("information"), DiagnosticSeverity::Info);
    assert_eq!(common::severity_from_level("note"), DiagnosticSeverity::Info);
    assert_eq!(common::severity_from_level("help"), DiagnosticSeverity::Info);
    assert_eq!(common::severity_from_level("unknown"), DiagnosticSeverity::Hint);
    assert_eq!(common::severity_from_level(""), DiagnosticSeverity::Hint);
}

#[test]
fn severity_from_code_covers_eslint_values() {
    assert_eq!(common::severity_from_code(2), DiagnosticSeverity::Error);
    assert_eq!(common::severity_from_code(1), DiagnosticSeverity::Warning);
    assert_eq!(common::severity_from_code(0), DiagnosticSeverity::Info);
    assert_eq!(common::severity_from_code(-1), DiagnosticSeverity::Info);
    assert_eq!(common::severity_from_code(99), DiagnosticSeverity::Info);
}

// ──────────────────────── rustfmt ────────────────────────────────────

#[test]
fn rustfmt_parses_diff_with_line_number() {
    let out = "Diff in /repo/src/main.rs at line 12:\n-old\n+new\n";
    let diags = builtins::rustfmt::parse_output(out);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].file, "/repo/src/main.rs");
    assert_eq!(diags[0].line, Some(12));
    assert_eq!(diags[0].severity, DiagnosticSeverity::Warning);
}

#[test]
fn rustfmt_parses_multiple_diffs() {
    let out = "Diff in a.rs at line 1:\n-x\n+y\nDiff in b.rs at line 5:\n-a\n+b\n";
    assert_eq!(builtins::rustfmt::parse_output(out).len(), 2);
}

#[test]
fn rustfmt_empty_output_no_diagnostics() {
    assert!(builtins::rustfmt::parse_output("").is_empty());
}

#[test]
fn rustfmt_unrelated_lines_ignored() {
    assert!(builtins::rustfmt::parse_output("Compiling foo\nFinished\n").is_empty());
}

// ──────────────────────── clippy ─────────────────────────────────────

#[test]
fn clippy_parses_warning_with_span() {
    let line = r#"{"reason":"compiler-message","message":{"message":"unused variable: `x`","level":"warning","code":{"code":"unused_variables"},"spans":[{"file_name":"src/main.rs","line_start":3,"column_start":9,"is_primary":true}]}}"#;
    let diags = builtins::clippy::parse_output(line);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, DiagnosticSeverity::Warning);
    assert_eq!(diags[0].file, "src/main.rs");
    assert_eq!(diags[0].line, Some(3));
    assert_eq!(diags[0].rule.as_deref(), Some("unused_variables"));
}

#[test]
fn clippy_skips_compiler_artifact() {
    let line = r#"{"reason":"compiler-artifact","package_id":"foo"}"#;
    assert!(builtins::clippy::parse_output(line).is_empty());
}

#[test]
fn clippy_handles_error_level() {
    let line = r#"{"reason":"compiler-message","message":{"message":"type mismatch","level":"error","spans":[{"file_name":"a.rs","line_start":1,"column_start":1,"is_primary":true}]}}"#;
    let diags = builtins::clippy::parse_output(line);
    assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
}

#[test]
fn clippy_empty_output() {
    assert!(builtins::clippy::parse_output("").is_empty());
}

#[test]
fn clippy_malformed_json_no_panic() {
    assert!(builtins::clippy::parse_output("{truncated").is_empty());
}

// ──────────────────────── prettier ───────────────────────────────────

#[test]
fn prettier_parses_warn_lines() {
    let out = "Checking formatting...\n[warn] src/a.ts\n[warn] src/b.ts\n[warn] Code style issues found in 2 files.\n";
    let diags = builtins::prettier::parse_output(out);
    assert_eq!(diags.len(), 2);
    assert_eq!(diags[0].file, "src/a.ts");
    assert_eq!(diags[1].file, "src/b.ts");
}

#[test]
fn prettier_clean_output_no_diagnostics() {
    let out = "Checking formatting...\nAll matched files use Prettier code style!\n";
    assert!(builtins::prettier::parse_output(out).is_empty());
}

#[test]
fn prettier_empty_output() {
    assert!(builtins::prettier::parse_output("").is_empty());
}

// ──────────────────────── eslint ─────────────────────────────────────

#[test]
fn eslint_parses_json_report() {
    let input = r#"[{"filePath":"/a.ts","messages":[{"ruleId":"no-unused-vars","severity":2,"message":"x unused","line":3,"column":7}]}]"#;
    let diags = builtins::eslint::parse_output(input);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
    assert_eq!(diags[0].rule.as_deref(), Some("no-unused-vars"));
}

#[test]
fn eslint_empty_messages_no_diagnostics() {
    assert!(builtins::eslint::parse_output(r#"[{"filePath":"a.ts","messages":[]}]"#).is_empty());
}

#[test]
fn eslint_invalid_json_no_panic() {
    assert!(builtins::eslint::parse_output("not json").is_empty());
}

#[test]
fn eslint_top_level_object_no_panic() {
    assert!(builtins::eslint::parse_output("{}").is_empty());
}

// ──────────────────────── ruff ───────────────────────────────────────

#[test]
fn ruff_parses_diagnostic() {
    let input = r#"[{"code":"F401","message":"unused import","filename":"/a.py","location":{"row":3,"column":1}}]"#;
    let diags = builtins::ruff::parse_output(input);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].rule.as_deref(), Some("F401"));
    assert_eq!(diags[0].line, Some(3));
}

#[test]
fn ruff_missing_location_field() {
    let input = r#"[{"code":"F401","message":"x","filename":"a.py"}]"#;
    let diags = builtins::ruff::parse_output(input);
    assert_eq!(diags.len(), 1);
    assert!(diags[0].line.is_none());
}

#[test]
fn ruff_empty_report() {
    assert!(builtins::ruff::parse_output("[]").is_empty());
}

// ──────────────────────── black ──────────────────────────────────────

#[test]
fn black_parses_would_reformat() {
    let out = "would reformat src/main.py\nwould reformat src/cli.py\nOh no! 2 files.\n";
    let diags = builtins::black::parse_output(out);
    assert_eq!(diags.len(), 2);
    assert_eq!(diags[0].file, "src/main.py");
}

#[test]
fn black_clean_output() {
    assert!(builtins::black::parse_output("All done!\n1 file left unchanged.\n").is_empty());
}

// ──────────────────────── gofmt ──────────────────────────────────────

#[test]
fn gofmt_parses_file_list() {
    let diags = builtins::gofmt::parse_output("cmd/main.go\ninternal/foo.go\n");
    assert_eq!(diags.len(), 2);
    assert_eq!(diags[0].file, "cmd/main.go");
}

#[test]
fn gofmt_empty_output() {
    assert!(builtins::gofmt::parse_output("").is_empty());
}

// ──────────────────────── govet ──────────────────────────────────────

#[test]
fn govet_parses_with_column() {
    let out = "./cmd/main.go:12:4: printf call has possible formatting\n";
    let diags = builtins::govet::parse_output(out);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].line, Some(12));
    assert_eq!(diags[0].column, Some(4));
}

#[test]
fn govet_parses_without_column() {
    let out = "cmd/main.go:7: unreachable code\n";
    let diags = builtins::govet::parse_output(out);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].line, Some(7));
    assert!(diags[0].column.is_none());
}

#[test]
fn govet_ignores_package_lines() {
    assert!(builtins::govet::parse_output("# example.com/foo\nvet: ok\n").is_empty());
}

// ──────────────────────── biome ──────────────────────────────────────

#[test]
fn biome_parses_diagnostics() {
    let input = r#"{"diagnostics":[{"category":"lint/suspicious/noDoubleEquals","severity":"error","description":"Use ===","location":{"path":{"file":"src/main.ts"}}}]}"#;
    let diags = builtins::biome::parse_output(input);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
}

#[test]
fn biome_empty_diagnostics() {
    assert!(builtins::biome::parse_output(r#"{"diagnostics":[]}"#).is_empty());
}

#[test]
fn biome_invalid_json_no_panic() {
    assert!(builtins::biome::parse_output("{bad").is_empty());
}

// ──────────────────────── oxlint ─────────────────────────────────────

#[test]
fn oxlint_parses_eslint_compatible() {
    let input = r#"[{"filePath":"/a.ts","messages":[{"ruleId":"no-unused-vars","severity":2,"message":"unused","line":1,"column":1}]}]"#;
    let diags = builtins::oxlint::parse_output(input);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
}

#[test]
fn oxlint_empty_report() {
    assert!(builtins::oxlint::parse_output("[]").is_empty());
}

// ──────────────────────── shellcheck ─────────────────────────────────

#[test]
fn shellcheck_parses_with_rule_code() {
    let input = r#"[{"file":"a.sh","line":14,"column":5,"level":"warning","code":2086,"message":"Double quote to prevent globbing."}]"#;
    let diags = builtins::shellcheck::parse_output(input);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].rule.as_deref(), Some("SC2086"));
    assert_eq!(diags[0].severity, DiagnosticSeverity::Warning);
}

#[test]
fn shellcheck_info_level() {
    let input = r#"[{"file":"a.sh","line":1,"column":1,"level":"info","code":1000,"message":"x"}]"#;
    let diags = builtins::shellcheck::parse_output(input);
    assert_eq!(diags[0].severity, DiagnosticSeverity::Info);
}

#[test]
fn shellcheck_empty_report() {
    assert!(builtins::shellcheck::parse_output("[]").is_empty());
}

// ──────────────────────── gitleaks ───────────────────────────────────

#[test]
fn gitleaks_parses_finding() {
    let input = r#"[{"RuleID":"aws-access-key","Description":"AWS Access Key","File":"deploy/secrets.env","StartLine":4,"StartColumn":9}]"#;
    let diags = builtins::gitleaks::parse_output(input);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
    assert_eq!(diags[0].rule.as_deref(), Some("aws-access-key"));
    assert!(diags[0].message.contains("AWS Access Key"));
}

#[test]
fn gitleaks_missing_description() {
    let input = r#"[{"RuleID":"x","File":"a","StartLine":1,"StartColumn":1}]"#;
    let diags = builtins::gitleaks::parse_output(input);
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("secret detected"));
}

#[test]
fn gitleaks_empty_report() {
    assert!(builtins::gitleaks::parse_output("[]").is_empty());
}

// ──────────────── builtin config merge tests ─────────────────────────

#[test]
fn builtin_meta_rustfmt_has_correct_defaults() {
    let m = builtins::get("rustfmt").unwrap();
    assert!(m.concurrent_safe);
    assert!(!m.network);
    assert!(m.run.contains("cargo fmt"));
    assert!(m.fix.is_some());
    assert!(!m.glob.is_empty());
}

#[test]
fn builtin_meta_clippy_has_correct_defaults() {
    let m = builtins::get("clippy").unwrap();
    assert!(m.concurrent_safe);
    assert!(m.run.contains("clippy"));
    assert!(m.fix.is_some());
}

#[test]
fn builtin_meta_eslint_has_correct_defaults() {
    let m = builtins::get("eslint").unwrap();
    assert!(m.concurrent_safe);
    assert!(m.run.contains("eslint"));
    assert_eq!(m.tool_binary, "eslint");
}

#[test]
fn builtin_meta_gitleaks_has_no_fix() {
    let m = builtins::get("gitleaks").unwrap();
    assert!(m.fix.is_none());
}

#[test]
fn builtin_meta_govet_has_no_fix() {
    let m = builtins::get("govet").unwrap();
    assert!(m.fix.is_none());
}

#[test]
fn builtin_meta_shellcheck_has_no_fix() {
    let m = builtins::get("shellcheck").unwrap();
    assert!(m.fix.is_none());
}

// ───────────────── diagnostic serde round-trip ───────────────────────

#[test]
fn diagnostic_severity_round_trips_through_json() {
    for sev in [
        DiagnosticSeverity::Error,
        DiagnosticSeverity::Warning,
        DiagnosticSeverity::Info,
        DiagnosticSeverity::Hint,
    ] {
        let json = serde_json::to_string(&sev).unwrap();
        let back: DiagnosticSeverity = serde_json::from_str(&json).unwrap();
        assert_eq!(sev, back);
    }
}
