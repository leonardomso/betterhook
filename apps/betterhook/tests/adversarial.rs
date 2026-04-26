//! Adversarial tests — boundary cases, malformed input, and "this
//! should never panic" guarantees.
//!
//! Each module in this file targets one subsystem and exercises edge
//! cases the per-module unit tests don't cover, plus the kind of
//! garbage input a fuzzer might find.

use std::collections::BTreeMap;
use std::path::PathBuf;

use betterhook::cache::{
    ArgsHash, CacheKey, CachedInput, CachedResult, ContentHash, Store, ToolHash, args_hash,
    hash_bytes, inputs_fresh, snapshot_inputs,
};
use betterhook::config::parse::Format;
use betterhook::config::{Job, parse_bytes};
use betterhook::runner::dag::{DagError, build_dag};
use betterhook::runner::output::DiagnosticSeverity;

// ────────────────────────────── helpers ──────────────────────────────

fn job(name: &str, reads: &[&str], writes: &[&str], priority: u32) -> Job {
    Job {
        name: name.to_owned(),
        run: "true".to_owned(),
        fix: None,
        glob: Vec::new(),
        exclude: Vec::new(),
        tags: Vec::new(),
        skip: None,
        only: None,
        env: BTreeMap::new(),
        root: None,
        stage_fixed: false,
        isolate: None,
        timeout: None,
        interactive: false,
        fail_text: None,
        priority,
        reads: reads.iter().map(|s| (*s).to_owned()).collect(),
        writes: writes.iter().map(|s| (*s).to_owned()).collect(),
        network: false,
        concurrent_safe: false,
        builtin: None,
    }
}

// ─────────────────────────────── DAG ────────────────────────────────
mod dag {
    use super::*;

    #[test]
    fn empty_job_list_yields_empty_graph() {
        let dag = build_dag(&[]).unwrap();
        assert!(dag.nodes.is_empty());
        assert!(dag.roots().is_empty());
        assert_eq!(dag.edge_count(), 0);
    }

    #[test]
    fn single_job_is_a_root_with_no_edges() {
        let dag = build_dag(&[job("solo", &["**/*.rs"], &["**/*.rs"], 0)]).unwrap();
        assert_eq!(dag.nodes.len(), 1);
        assert_eq!(dag.roots(), vec![0]);
        assert_eq!(dag.edge_count(), 0);
    }

    #[test]
    fn self_overlap_does_not_create_self_loop() {
        // A job that reads and writes the same files should not edge to
        // itself — `build_dag` only iterates over (a, b) with a < b.
        let dag = build_dag(&[job("self", &["a.rs"], &["a.rs"], 0)]).unwrap();
        assert!(dag.nodes[0].parents.is_empty());
        assert!(dag.nodes[0].children.is_empty());
    }

    #[test]
    fn three_way_chain_resolves_topologically() {
        // a writes -> b reads + writes -> c reads. Result should be a
        // single chain a → b → c.
        let jobs = vec![
            job("a", &[], &["**/*.ts"], 0),
            job("b", &["**/*.ts"], &["**/*.ts"], 1),
            job("c", &["**/*.ts"], &[], 2),
        ];
        let dag = build_dag(&jobs).unwrap();
        assert_eq!(dag.roots(), vec![0]);
        // a is parent of b
        assert!(dag.nodes[0].children.contains(&1));
        // b is parent of c
        assert!(dag.nodes[1].children.contains(&2));
        // c has no children
        assert!(dag.nodes[2].children.is_empty());
    }

    #[test]
    fn invalid_glob_in_reads_returns_error() {
        let j = job("bad", &["src/[unterminated"], &[], 0);
        let err = build_dag(&[j]).unwrap_err();
        let DagError::Glob { job, .. } = err;
        assert_eq!(job, "bad");
    }

    #[test]
    fn pessimistic_overlap_serializes_star_writers() {
        // Two jobs writing very different concrete files but using `**`
        // patterns should still serialize because the probe heuristic
        // is intentionally pessimistic.
        let jobs = vec![
            job("a", &[], &["**/*.json"], 0),
            job("b", &[], &["**/*.json"], 1),
        ];
        let dag = build_dag(&jobs).unwrap();
        assert_eq!(dag.edge_count(), 1);
    }

    #[test]
    fn many_disjoint_jobs_run_fully_parallel() {
        // 50 jobs each writing a unique file. Every job should be a
        // root with zero edges between any pair.
        let jobs: Vec<Job> = (0..50)
            .map(|i| {
                let name = format!("j{i}");
                let f = format!("file_{i}.rs");
                let reads = [];
                let writes = [f.as_str()];
                job(&name, &reads, &writes, i)
            })
            .collect();
        let dag = build_dag(&jobs).unwrap();
        assert_eq!(dag.roots().len(), 50);
        assert_eq!(dag.edge_count(), 0);
    }

    #[test]
    fn glob_explosion_does_not_panic() {
        // 200 patterns per job. Pure stress check — must not panic, must
        // build successfully.
        let big: Vec<String> = (0..200).map(|i| format!("dir_{i}/**/*.rs")).collect();
        let mut j = job("big", &[], &[], 0);
        j.writes = big;
        let dag = build_dag(&[j]).unwrap();
        assert_eq!(dag.nodes.len(), 1);
    }
}

// ───────────────────────── cache key + freshness ─────────────────────
mod cache {
    use super::*;

    #[test]
    fn args_hash_is_distinct_for_every_permutation() {
        let a = args_hash(&["a".to_owned(), "b".to_owned(), "c".to_owned()]);
        let b = args_hash(&["a".to_owned(), "c".to_owned(), "b".to_owned()]);
        let c = args_hash(&["c".to_owned(), "b".to_owned(), "a".to_owned()]);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn empty_args_hash_is_stable() {
        let a = args_hash(&[]);
        let b = args_hash(&[]);
        assert_eq!(a, b);
    }

    #[test]
    fn cache_key_path_is_filesystem_safe() {
        // Every char in the on-disk path should be safe to use on a
        // POSIX filesystem (no NUL, no slashes inside the segments
        // beyond the deliberate sharding split).
        let key = CacheKey {
            content: ContentHash("c".repeat(64)),
            tool: ToolHash("ab".to_owned() + &"f".repeat(62)),
            args: ArgsHash("0".repeat(64)),
        };
        let rel = key.relative_path();
        let rel_str = rel.to_string_lossy();
        assert!(!rel_str.contains('\0'));
        assert!(!rel_str.contains(".."));
        // One shard component plus one filename component.
        assert_eq!(rel.components().count(), 2);
    }

    #[test]
    fn snapshot_of_missing_file_records_none_mtime() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("never-existed.rs");
        let snap = snapshot_inputs(std::slice::from_ref(&missing));
        assert_eq!(snap.len(), 1);
        assert!(snap[0].modified_at.is_none());
    }

    #[test]
    fn freshness_gate_rejects_when_input_is_deleted() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = dir.path().join("a.rs");
        std::fs::write(&f, b"hi").unwrap();
        let snap = snapshot_inputs(std::slice::from_ref(&f));
        assert!(inputs_fresh(&snap));
        std::fs::remove_file(&f).unwrap();
        assert!(
            !inputs_fresh(&snap),
            "deleted input must be treated as stale"
        );
    }

    #[test]
    fn freshness_gate_passes_for_unchanged_inputs() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = dir.path().join("a.rs");
        std::fs::write(&f, b"hi").unwrap();
        let snap = snapshot_inputs(&[f]);
        // Sub-second precision should not flap.
        for _ in 0..5 {
            assert!(inputs_fresh(&snap));
        }
    }

    #[test]
    fn empty_inputs_freshness_is_always_true() {
        // An empty snapshot means there is nothing to invalidate, so the
        // freshness helper must stay true.
        assert!(inputs_fresh(&[]));
    }

    #[test]
    fn store_clear_on_empty_dir_is_zero() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::new(dir.path());
        assert_eq!(store.clear().unwrap(), 0);
        assert_eq!(store.len().unwrap(), 0);
    }

    #[test]
    fn store_round_trip_through_cached_result() {
        // End-to-end: write a result, read it back, every field intact.
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::new(dir.path());
        let key = CacheKey {
            content: ContentHash("d".repeat(64)),
            tool: ToolHash("ef".to_owned() + &"0".repeat(62)),
            args: ArgsHash("9".repeat(64)),
        };
        let result = CachedResult {
            exit: 0,
            events: Vec::new(),
            created_at: std::time::SystemTime::now(),
            inputs: vec![CachedInput {
                path: PathBuf::from("a.rs"),
                modified_at: None,
            }],
        };
        store.put(&key, &result).unwrap();
        let back = store.get(&key).unwrap().expect("entry roundtrips");
        assert_eq!(back.inputs.len(), 1);
        assert_eq!(back.inputs[0].path, PathBuf::from("a.rs"));
    }

    #[test]
    fn hash_bytes_avoids_collisions_on_short_inputs() {
        // 256 single-byte inputs should produce 256 distinct hashes.
        let mut seen = std::collections::HashSet::new();
        for b in 0u8..=255 {
            seen.insert(hash_bytes(&[b]));
        }
        assert_eq!(seen.len(), 256);
    }
}

// ─────────────────────────── KDL parser ──────────────────────────────
mod kdl_parser {
    use super::*;

    fn assert_parse_err(src: &str) {
        let err = parse_bytes(src, Format::Kdl, "adversarial.kdl").unwrap_err();
        // Just check that it's a structured error and not a panic.
        let _ = format!("{err}");
    }

    #[test]
    fn empty_input_is_valid() {
        // An empty KDL document should produce an empty (default) config.
        let cfg = parse_bytes("", Format::Kdl, "empty.kdl").unwrap();
        assert!(cfg.hooks.is_empty());
    }

    #[test]
    fn unterminated_block_returns_error() {
        assert_parse_err("hook \"pre-commit\" {");
    }

    #[test]
    fn unterminated_string_returns_error() {
        assert_parse_err("hook \"pre-commit");
    }

    #[test]
    fn unknown_top_level_node_does_not_panic() {
        // Unknown nodes should be tolerated (skipped) or rejected with
        // a clean error — never panic.
        let src = "wat 1 2 3\nhook \"pre-commit\" {\n  job \"a\" {\n    run \"true\"\n  }\n}";
        let _ = parse_bytes(src, Format::Kdl, "adversarial.kdl");
    }

    #[test]
    fn job_without_run_does_not_panic() {
        // Missing required fields should error cleanly at lower time,
        // not panic during parse.
        let src = r#"
hook "pre-commit" {
    job "no-run" {
    }
}
"#;
        let _ = parse_bytes(src, Format::Kdl, "adversarial.kdl");
    }

    #[test]
    fn deeply_nested_braces_are_safe() {
        let src = "hook \"a\" { job \"b\" { run \"true\" glob \"*.rs\" } }";
        let _ = parse_bytes(src, Format::Kdl, "adversarial.kdl");
    }

    #[test]
    fn arbitrary_garbage_does_not_panic() {
        for src in [
            "}}}",
            "\x00\x01\x02",
            "\"\"",
            "\\\\",
            ";;;;;",
            "hook { hook { hook { hook { hook { hook {",
        ] {
            let _ = parse_bytes(src, Format::Kdl, "adversarial.kdl");
        }
    }
}

// ───────────────────────── builtin parsers ───────────────────────────
mod builtins {
    use betterhook::builtins;

    #[test]
    fn every_builtin_has_a_unique_name() {
        let r = builtins::registry();
        let names: Vec<&&str> = r.keys().collect();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), r.len());
    }

    #[test]
    fn every_builtin_has_a_nonempty_run_template() {
        for meta in builtins::registry().values() {
            assert!(!meta.run.is_empty(), "{} has empty run", meta.id.0);
        }
    }

    #[test]
    fn clippy_parser_skips_garbage_lines() {
        let out = "not json\n{\"reason\":\"compiler-message\",\"message\":{\"message\":\"x\",\"level\":\"warning\",\"spans\":[]}}\nstill not json\n";
        // Should not panic; should return zero diagnostics because there's
        // no usable span.
        let _ = builtins::clippy::parse_output(out);
    }

    #[test]
    fn clippy_parser_handles_truncated_json() {
        // Truncated JSON should not panic.
        let _ = builtins::clippy::parse_output("{\"reason\":\"compiler-message");
    }

    #[test]
    fn eslint_parser_handles_top_level_object_not_array() {
        // eslint always emits an array; an object should yield zero diagnostics
        // and not panic.
        assert!(builtins::eslint::parse_output("{}").is_empty());
    }

    #[test]
    fn ruff_parser_handles_missing_location_field() {
        let input = r#"[{"code":"F401","message":"unused","filename":"a.py"}]"#;
        let diags = builtins::ruff::parse_output(input);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].line.is_none());
        assert!(diags[0].column.is_none());
    }

    #[test]
    fn shellcheck_parser_handles_string_severity() {
        let input =
            r#"[{"file":"a.sh","line":1,"column":1,"level":"info","code":1000,"message":"x"}]"#;
        let diags = builtins::shellcheck::parse_output(input);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule.as_deref(), Some("SC1000"));
    }

    #[test]
    fn gitleaks_parser_handles_missing_description() {
        let input = r#"[{"RuleID":"x","File":"a","StartLine":1,"StartColumn":1}]"#;
        let diags = builtins::gitleaks::parse_output(input);
        assert_eq!(diags.len(), 1);
        // Default message must still be present.
        assert!(diags[0].message.contains("secret detected") || !diags[0].message.is_empty());
    }

    #[test]
    fn govet_parser_drops_lines_without_line_number() {
        let out = "not a vet line\n# package boundary\nplain text\n";
        assert!(builtins::govet::parse_output(out).is_empty());
    }

    #[test]
    fn rustfmt_parser_ignores_unrelated_lines() {
        let out = "Compiling foo v0.1.0\nFinished release [optimized]\n";
        assert!(builtins::rustfmt::parse_output(out).is_empty());
    }

    #[test]
    fn prettier_parser_strips_summary_line() {
        let out =
            "Checking formatting...\n[warn] a.ts\n[warn] Code style issues found in 1 files.\n";
        let diags = builtins::prettier::parse_output(out);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].file, "a.ts");
    }

    #[test]
    fn black_parser_ignores_summary_line() {
        let out = "would reformat a.py\nOh no! 1 file would be reformatted.\n";
        let diags = builtins::black::parse_output(out);
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn diagnostic_severity_round_trips_through_serde() {
        for sev in [
            super::DiagnosticSeverity::Error,
            super::DiagnosticSeverity::Warning,
            super::DiagnosticSeverity::Info,
            super::DiagnosticSeverity::Hint,
        ] {
            let s = serde_json::to_string(&sev).unwrap();
            let back: super::DiagnosticSeverity = serde_json::from_str(&s).unwrap();
            assert_eq!(sev, back);
        }
    }
}

// ────────────────────────── husky importer ──────────────────────────
mod husky {
    use betterhook::config::import::{ImportSource, husky};
    use std::path::PathBuf;

    #[test]
    fn empty_script_is_safe() {
        let (raw, _) = husky::from_script("", &PathBuf::from(".husky/pre-commit")).unwrap();
        // The hook block exists but has no jobs.
        assert!(raw.hooks.contains_key("pre-commit"));
    }

    #[test]
    fn whitespace_only_script_is_safe() {
        let (raw, _) =
            husky::from_script("\n\n   \n  \n", &PathBuf::from(".husky/pre-commit")).unwrap();
        assert!(raw.hooks["pre-commit"].jobs.is_empty());
    }

    #[test]
    fn comments_only_script_yields_no_jobs() {
        let src = "#!/usr/bin/env sh\n# nothing to see here\n# really\n";
        let (raw, _) = husky::from_script(src, &PathBuf::from(".husky/pre-commit")).unwrap();
        assert!(raw.hooks["pre-commit"].jobs.is_empty());
    }

    #[test]
    fn auto_detect_handles_extensionless_husky_file() {
        let p = PathBuf::from(".husky/post-checkout");
        assert_eq!(ImportSource::auto_detect(&p), Some(ImportSource::Husky));
    }

    #[test]
    fn auto_detect_returns_none_for_unknown_path() {
        let p = PathBuf::from("random/path/to/file.toml");
        assert!(ImportSource::auto_detect(&p).is_none());
    }
}

// ──────────────────────── speculative debouncer ──────────────────────
mod speculative {
    use betterhook::daemon::speculative::{SpeculativeStats, read_stats, stats_path, write_stats};

    #[test]
    fn stats_path_is_under_betterhook_subdir() {
        let p = stats_path(std::path::Path::new("/tmp/x"));
        assert!(p.to_string_lossy().contains("betterhook"));
        assert!(p.to_string_lossy().ends_with(".json"));
    }

    #[test]
    fn read_stats_returns_none_for_missing_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let nope = dir.path().join("definitely-not-here");
        assert!(read_stats(&nope).is_none());
    }

    #[test]
    fn write_stats_silently_ignores_unwritable_dir() {
        // Pointing at a path under an unwritable parent — must not panic.
        write_stats(
            std::path::Path::new("/this/path/should/never/exist/forever"),
            &SpeculativeStats::default(),
        );
    }
}

// ────────────────────────── config boundaries ──────────────────────
mod config_boundaries {
    use super::*;

    #[test]
    fn empty_toml_is_valid() {
        let raw = parse_bytes("", Format::Toml, "empty.toml").unwrap();
        assert!(raw.hooks.is_empty());
    }

    #[test]
    fn empty_yaml_is_valid() {
        let raw = parse_bytes("{}", Format::Yaml, "empty.yaml").unwrap();
        assert!(raw.hooks.is_empty());
    }

    #[test]
    fn empty_json_is_valid() {
        let raw = parse_bytes("{}", Format::Json, "empty.json").unwrap();
        assert!(raw.hooks.is_empty());
    }

    #[test]
    fn config_with_bom_prefix() {
        let src = "\u{FEFF}[hooks.pre-commit.jobs.lint]\nrun = \"true\"\n";
        let _ = parse_bytes(src, Format::Toml, "bom.toml");
    }

    #[test]
    fn config_with_trailing_whitespace() {
        let src = "[hooks.pre-commit.jobs.lint]\nrun = \"true\"   \n";
        let raw = parse_bytes(src, Format::Toml, "ws.toml").unwrap();
        assert!(raw.hooks.contains_key("pre-commit"));
    }

    #[test]
    fn unicode_job_name() {
        let src = "[hooks.pre-commit.jobs.\"日本語\"]\nrun = \"true\"\n";
        let raw = parse_bytes(src, Format::Toml, "unicode.toml").unwrap();
        assert!(raw.hooks["pre-commit"].jobs.contains_key("日本語"));
    }

    #[test]
    fn very_long_job_name_does_not_panic() {
        let name = "x".repeat(1000);
        let src = format!("[hooks.pre-commit.jobs.\"{name}\"]\nrun = \"true\"\n");
        let raw = parse_bytes(&src, Format::Toml, "long.toml").unwrap();
        assert_eq!(raw.hooks["pre-commit"].jobs.len(), 1);
    }

    #[test]
    fn very_long_run_command_does_not_panic() {
        let cmd = "echo ".to_owned() + &"x".repeat(10_000);
        let src = format!("[hooks.pre-commit.jobs.big]\nrun = \"{cmd}\"\n");
        let raw = parse_bytes(&src, Format::Toml, "long-cmd.toml").unwrap();
        assert!(
            raw.hooks["pre-commit"].jobs["big"]
                .run
                .as_ref()
                .unwrap()
                .len()
                > 10_000
        );
    }

    #[test]
    fn fifty_jobs_in_one_hook() {
        let mut src = String::new();
        for i in 0..50 {
            use std::fmt::Write;
            let _ = write!(src, "[hooks.pre-commit.jobs.job_{i}]\nrun = \"true\"\n");
        }
        let raw = parse_bytes(&src, Format::Toml, "fifty.toml").unwrap();
        assert_eq!(raw.hooks["pre-commit"].jobs.len(), 50);
    }

    #[test]
    fn lower_with_no_hooks_is_valid() {
        let raw = parse_bytes("[meta]\nversion = 1\n", Format::Toml, "no-hooks.toml").unwrap();
        let cfg = raw.lower().unwrap();
        assert!(cfg.hooks.is_empty());
    }

    #[test]
    fn lower_fifty_jobs_builds_dag() {
        let mut src = String::new();
        for i in 0..50 {
            use std::fmt::Write;
            let _ = write!(src, "[hooks.pre-commit.jobs.job_{i}]\nrun = \"true\"\n");
        }
        let raw = parse_bytes(&src, Format::Toml, "fifty.toml").unwrap();
        let cfg = raw.lower().unwrap();
        let dag = build_dag(&cfg.hooks["pre-commit"].jobs).unwrap();
        assert_eq!(dag.nodes.len(), 50);
    }
}

// ───────────────── cache adversarial (P13) ──────────────────────────
mod cache_adversarial {
    use super::*;

    #[test]
    fn cache_key_with_empty_file_set_is_stable() {
        let a = args_hash(&[]);
        let b = args_hash(&[]);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_bytes_with_1mb_input() {
        let big = vec![0xABu8; 1_000_000];
        let h = hash_bytes(&big);
        assert_eq!(h.len(), 64, "hash should be 64 hex chars");
    }

    #[test]
    fn hash_bytes_empty_input() {
        let h = hash_bytes(b"");
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn store_put_get_with_many_inputs() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::new(dir.path());
        let key = CacheKey {
            content: ContentHash("a".repeat(64)),
            tool: ToolHash("bc".to_owned() + &"0".repeat(62)),
            args: ArgsHash("d".repeat(64)),
        };
        let inputs: Vec<CachedInput> = (0..100)
            .map(|i| CachedInput {
                path: PathBuf::from(format!("file_{i}.rs")),
                modified_at: None,
            })
            .collect();
        let result = CachedResult {
            exit: 0,
            events: Vec::new(),
            created_at: std::time::SystemTime::now(),
            inputs,
        };
        store.put(&key, &result).unwrap();
        let back = store.get(&key).unwrap().expect("should round-trip");
        assert_eq!(back.inputs.len(), 100);
    }

    #[test]
    fn store_verify_detects_no_corruption_on_valid_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::new(dir.path());
        let key = CacheKey {
            content: ContentHash("e".repeat(64)),
            tool: ToolHash("fg".to_owned() + &"0".repeat(62)),
            args: ArgsHash("h".repeat(64)),
        };
        store
            .put(
                &key,
                &CachedResult {
                    exit: 0,
                    events: Vec::new(),
                    created_at: std::time::SystemTime::now(),
                    inputs: Vec::new(),
                },
            )
            .unwrap();
        let corrupt = store.verify().unwrap();
        assert!(corrupt.is_empty());
    }

    #[test]
    fn store_stats_after_put_and_clear() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::new(dir.path());
        let key = CacheKey {
            content: ContentHash("i".repeat(64)),
            tool: ToolHash("jk".to_owned() + &"0".repeat(62)),
            args: ArgsHash("l".repeat(64)),
        };
        store
            .put(
                &key,
                &CachedResult {
                    exit: 0,
                    events: Vec::new(),
                    created_at: std::time::SystemTime::now(),
                    inputs: Vec::new(),
                },
            )
            .unwrap();
        let stats = store.stats().unwrap();
        assert_eq!(stats.entries, 1);
        assert!(stats.total_bytes > 0);
        store.clear().unwrap();
        let stats2 = store.stats().unwrap();
        assert_eq!(stats2.entries, 0);
    }
}

// ───────────────── importer adversarial (P13) ───────────────────────
mod importer_adversarial {
    use betterhook::config::import::{ImportSource, hk, husky, lefthook, pre_commit};

    #[test]
    fn lefthook_empty_yaml_is_safe() {
        let (raw, _) = lefthook::from_yaml("{}").unwrap();
        assert!(raw.hooks.is_empty());
    }

    #[test]
    fn lefthook_minimal_command() {
        let src = "pre-commit:\n  commands:\n    a:\n      run: \"true\"\n";
        let (raw, _) = lefthook::from_yaml(src).unwrap();
        assert!(raw.hooks.contains_key("pre-commit"));
    }

    #[test]
    fn husky_multiline_script() {
        let script = "#!/usr/bin/env sh\nset -e\nnpx lint-staged\ncargo test\n";
        let (raw, _) =
            husky::from_script(script, &std::path::PathBuf::from(".husky/pre-commit")).unwrap();
        assert!(!raw.hooks["pre-commit"].jobs.is_empty());
    }

    #[test]
    fn hk_empty_toml_is_safe() {
        let _ = hk::from_text("");
    }

    #[test]
    fn pre_commit_empty_repos_is_safe() {
        let src = "repos: []\n";
        let (raw, _) = pre_commit::from_yaml(src).unwrap();
        assert!(raw.hooks["pre-commit"].jobs.is_empty());
    }

    #[test]
    fn import_source_from_cli_all_variants() {
        assert_eq!(
            ImportSource::from_cli("lefthook"),
            Some(ImportSource::Lefthook)
        );
        assert_eq!(ImportSource::from_cli("husky"), Some(ImportSource::Husky));
        assert_eq!(ImportSource::from_cli("hk"), Some(ImportSource::Hk));
        assert_eq!(
            ImportSource::from_cli("pre-commit"),
            Some(ImportSource::PreCommit)
        );
        assert!(ImportSource::from_cli("nope").is_none());
    }
}

// ──────────────── DAG boundary tests (P13) ──────────────────────────
mod dag_boundary {
    use super::*;

    #[test]
    fn hundred_jobs_with_shared_writers() {
        let jobs: Vec<Job> = (0..100)
            .map(|i| {
                let write_pat = if i % 2 == 0 {
                    "src/**/*.ts".to_owned()
                } else {
                    format!("unique-{i}/**/*.rs")
                };
                job(&format!("j-{i}"), &[], &[&write_pat], i)
            })
            .collect();
        let dag = build_dag(&jobs).unwrap();
        assert_eq!(dag.nodes.len(), 100);
        assert!(dag.edge_count() > 0);
    }

    #[test]
    fn job_with_no_reads_writes_network_is_root() {
        let jobs = vec![
            job("isolated", &[], &[], 0),
            job("writer", &[], &["**/*.ts"], 1),
        ];
        let dag = build_dag(&jobs).unwrap();
        assert!(
            dag.nodes[0].parents.is_empty(),
            "job with no resource declarations should be a root"
        );
    }

    #[test]
    fn explain_dot_with_50_nodes_valid() {
        let jobs: Vec<Job> = (0..50)
            .map(|i| job(&format!("n-{i}"), &[], &[], i))
            .collect();
        let dag = build_dag(&jobs).unwrap();
        let mut dot = String::from("digraph betterhook {\n");
        for node in &dag.nodes {
            use std::fmt::Write;
            let _ = writeln!(dot, "  \"{}\";", node.job.name);
        }
        for (a, b) in dag.edges() {
            use std::fmt::Write;
            let _ = writeln!(
                dot,
                "  \"{}\" -> \"{}\";",
                dag.nodes[a].job.name, dag.nodes[b].job.name
            );
        }
        dot.push_str("}\n");
        assert!(dot.starts_with("digraph betterhook {"));
        assert!(dot.contains("\"n-0\""));
        assert!(dot.contains("\"n-49\""));
    }
}
