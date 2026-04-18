//! Multi-format config parsing, extends, overlay, and validation tests.

use std::collections::BTreeMap;
use std::path::PathBuf;

use betterhook::config::parse::Format;
use betterhook::config::schema::RawConfig;
use betterhook::config::{load, parse_bytes};

// ---------------------------------------------------------------------------
// Format detection
// ---------------------------------------------------------------------------

#[test]
fn format_from_path_toml() {
    assert!(matches!(
        Format::from_path(std::path::Path::new("betterhook.toml")).unwrap(),
        Format::Toml
    ));
}

#[test]
fn format_from_path_yaml() {
    assert!(matches!(
        Format::from_path(std::path::Path::new("betterhook.yaml")).unwrap(),
        Format::Yaml
    ));
}

#[test]
fn format_from_path_yml() {
    assert!(matches!(
        Format::from_path(std::path::Path::new("betterhook.yml")).unwrap(),
        Format::Yaml
    ));
}

#[test]
fn format_from_path_json() {
    assert!(matches!(
        Format::from_path(std::path::Path::new("betterhook.json")).unwrap(),
        Format::Json
    ));
}

#[test]
fn format_from_path_kdl() {
    assert!(matches!(
        Format::from_path(std::path::Path::new("betterhook.kdl")).unwrap(),
        Format::Kdl
    ));
}

#[test]
fn format_from_unknown_extension_errors() {
    assert!(Format::from_path(std::path::Path::new("betterhook.xml")).is_err());
}

// ---------------------------------------------------------------------------
// TOML parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_toml_basic_hook() {
    let src = r#"
[meta]
version = 1

[hooks.pre-commit.jobs.lint]
run = "eslint {files}"
glob = ["*.ts"]
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    assert!(raw.hooks.contains_key("pre-commit"));
    assert!(raw.hooks["pre-commit"].jobs.contains_key("lint"));
    assert_eq!(raw.hooks["pre-commit"].jobs["lint"].run, Some("eslint {files}".to_owned()));
}

#[test]
fn parse_toml_empty_is_valid() {
    let raw = parse_bytes("", Format::Toml, "empty.toml").unwrap();
    assert!(raw.hooks.is_empty());
}

#[test]
fn parse_toml_meta_version() {
    let src = "[meta]\nversion = 1\n";
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    assert_eq!(raw.meta.as_ref().unwrap().version, Some(1));
}

#[test]
fn parse_toml_deny_unknown_top_level_field() {
    let src = "badfield = true\n";
    assert!(parse_bytes(src, Format::Toml, "test.toml").is_err());
}

#[test]
fn parse_toml_deny_unknown_meta_field() {
    let src = "[meta]\nversion = 1\nwat = true\n";
    assert!(parse_bytes(src, Format::Toml, "test.toml").is_err());
}

#[test]
fn parse_toml_multiple_hooks() {
    let src = r#"
[hooks.pre-commit.jobs.lint]
run = "eslint"

[hooks.pre-push.jobs.test]
run = "cargo test"
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    assert_eq!(raw.hooks.len(), 2);
}

#[test]
fn parse_toml_job_with_all_fields() {
    let src = r#"
[hooks.pre-commit.jobs.lint]
run = "eslint {files}"
fix = "eslint --fix {files}"
glob = ["*.ts", "*.tsx"]
exclude = ["*.d.ts"]
tags = ["frontend"]
skip = "CI"
only = "lint"
stage_fixed = true
timeout = "30s"
interactive = false
fail_text = "Fix your lint errors!"
reads = ["**/*.ts"]
writes = []
network = false
concurrent_safe = true
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    let job = &raw.hooks["pre-commit"].jobs["lint"];
    assert_eq!(job.fix.as_deref(), Some("eslint --fix {files}"));
    assert_eq!(job.glob.len(), 2);
    assert_eq!(job.exclude.len(), 1);
    assert_eq!(job.tags.len(), 1);
    assert!(job.stage_fixed.unwrap());
    assert_eq!(job.timeout.as_deref(), Some("30s"));
}

// ---------------------------------------------------------------------------
// YAML parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_yaml_basic_hook() {
    let src = r#"
meta:
  version: 1
hooks:
  pre-commit:
    jobs:
      lint:
        run: "eslint"
"#;
    let raw = parse_bytes(src, Format::Yaml, "test.yaml").unwrap();
    assert!(raw.hooks.contains_key("pre-commit"));
}

#[test]
fn parse_yaml_empty_is_valid() {
    let raw = parse_bytes("{}", Format::Yaml, "empty.yaml").unwrap();
    assert!(raw.hooks.is_empty());
}

// ---------------------------------------------------------------------------
// JSON parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_json_basic_hook() {
    let src = r#"{
  "meta": { "version": 1 },
  "hooks": {
    "pre-commit": {
      "jobs": {
        "lint": { "run": "eslint" }
      }
    }
  }
}"#;
    let raw = parse_bytes(src, Format::Json, "test.json").unwrap();
    assert!(raw.hooks.contains_key("pre-commit"));
}

#[test]
fn parse_json_empty_object_is_valid() {
    let raw = parse_bytes("{}", Format::Json, "empty.json").unwrap();
    assert!(raw.hooks.is_empty());
}

// ---------------------------------------------------------------------------
// KDL parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_kdl_basic_hook() {
    let src = r#"
hook "pre-commit" {
    job "lint" {
        run "eslint"
    }
}
"#;
    let raw = parse_bytes(src, Format::Kdl, "test.kdl").unwrap();
    assert!(raw.hooks.contains_key("pre-commit"));
}

#[test]
fn parse_kdl_empty_is_valid() {
    let raw = parse_bytes("", Format::Kdl, "empty.kdl").unwrap();
    assert!(raw.hooks.is_empty());
}

// ---------------------------------------------------------------------------
// Cross-format equivalence
// ---------------------------------------------------------------------------

#[test]
fn toml_yaml_json_produce_equivalent_configs() {
    let toml_src = r#"
[hooks.pre-commit.jobs.lint]
run = "eslint"
"#;
    let yaml_src = r#"
hooks:
  pre-commit:
    jobs:
      lint:
        run: "eslint"
"#;
    let json_src = r#"{"hooks":{"pre-commit":{"jobs":{"lint":{"run":"eslint"}}}}}"#;

    let toml_cfg = parse_bytes(toml_src, Format::Toml, "t.toml")
        .unwrap()
        .lower()
        .unwrap();
    let yaml_cfg = parse_bytes(yaml_src, Format::Yaml, "t.yaml")
        .unwrap()
        .lower()
        .unwrap();
    let json_cfg = parse_bytes(json_src, Format::Json, "t.json")
        .unwrap()
        .lower()
        .unwrap();

    assert_eq!(toml_cfg, yaml_cfg);
    assert_eq!(yaml_cfg, json_cfg);
}

// ---------------------------------------------------------------------------
// RawConfig::lower()
// ---------------------------------------------------------------------------

#[test]
fn lower_produces_config_with_hook_and_job() {
    let src = r#"
[meta]
version = 1

[hooks.pre-commit.jobs.lint]
run = "eslint"
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    let cfg = raw.lower().unwrap();
    assert_eq!(cfg.hooks.len(), 1);
    assert_eq!(cfg.hooks["pre-commit"].jobs.len(), 1);
    assert_eq!(cfg.hooks["pre-commit"].jobs[0].run, "eslint");
}

#[test]
fn lower_job_without_run_or_builtin_errors() {
    let src = r#"
[hooks.pre-commit.jobs.bad]
glob = ["*.ts"]
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    assert!(raw.lower().is_err(), "job with no run or builtin should fail lower");
}

#[test]
fn lower_preserves_parallel_and_fail_fast() {
    let src = r#"
[hooks.pre-commit]
parallel = true
fail_fast = true

[hooks.pre-commit.jobs.lint]
run = "eslint"
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    let cfg = raw.lower().unwrap();
    assert!(cfg.hooks["pre-commit"].parallel);
    assert!(cfg.hooks["pre-commit"].fail_fast);
}

#[test]
fn lower_timeout_string_parsed() {
    let src = r#"
[hooks.pre-commit.jobs.lint]
run = "eslint"
timeout = "30s"
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    let cfg = raw.lower().unwrap();
    assert!(cfg.hooks["pre-commit"].jobs[0].timeout.is_some());
    assert_eq!(
        cfg.hooks["pre-commit"].jobs[0].timeout.unwrap(),
        std::time::Duration::from_secs(30)
    );
}

#[test]
fn lower_invalid_timeout_string_errors() {
    let src = r#"
[hooks.pre-commit.jobs.lint]
run = "eslint"
timeout = "not-a-duration"
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    assert!(raw.lower().is_err());
}

// ---------------------------------------------------------------------------
// RawConfig::v1_from_hooks
// ---------------------------------------------------------------------------

#[test]
fn v1_from_hooks_sets_meta_version() {
    let hooks = BTreeMap::new();
    let raw = RawConfig::v1_from_hooks(hooks);
    assert_eq!(raw.meta.as_ref().unwrap().version, Some(1));
}

// ---------------------------------------------------------------------------
// Overlay merging
// ---------------------------------------------------------------------------

#[test]
fn overlay_merge_adds_new_hook() {
    let base_src = r#"
[hooks.pre-commit.jobs.lint]
run = "eslint"
"#;
    let overlay_src = r#"
[hooks.pre-push.jobs.test]
run = "cargo test"
"#;
    let mut base = parse_bytes(base_src, Format::Toml, "base.toml").unwrap();
    let overlay = parse_bytes(overlay_src, Format::Toml, "overlay.toml").unwrap();
    base.merge_overlay(overlay);
    assert_eq!(base.hooks.len(), 2);
    assert!(base.hooks.contains_key("pre-commit"));
    assert!(base.hooks.contains_key("pre-push"));
}

#[test]
fn overlay_merge_adds_job_to_existing_hook() {
    let base_src = r#"
[hooks.pre-commit.jobs.lint]
run = "eslint"
"#;
    let overlay_src = r#"
[hooks.pre-commit.jobs.fmt]
run = "prettier --write"
"#;
    let mut base = parse_bytes(base_src, Format::Toml, "base.toml").unwrap();
    let overlay = parse_bytes(overlay_src, Format::Toml, "overlay.toml").unwrap();
    base.merge_overlay(overlay);
    let hook = &base.hooks["pre-commit"];
    assert_eq!(hook.jobs.len(), 2);
}

#[test]
fn overlay_merge_replaces_existing_job() {
    let base_src = r#"
[hooks.pre-commit.jobs.lint]
run = "eslint"
"#;
    let overlay_src = r#"
[hooks.pre-commit.jobs.lint]
run = "oxlint"
"#;
    let mut base = parse_bytes(base_src, Format::Toml, "base.toml").unwrap();
    let overlay = parse_bytes(overlay_src, Format::Toml, "overlay.toml").unwrap();
    base.merge_overlay(overlay);
    assert_eq!(
        base.hooks["pre-commit"].jobs["lint"].run.as_deref(),
        Some("oxlint")
    );
}

// ---------------------------------------------------------------------------
// Packages
// ---------------------------------------------------------------------------

#[test]
fn parse_toml_with_packages() {
    let src = r#"
[meta]
version = 1

[packages.frontend]
path = "apps/web"

[packages.frontend.hooks.pre-commit.jobs.lint]
run = "eslint"
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    assert_eq!(raw.packages.len(), 1);
    assert_eq!(raw.packages["frontend"].path, PathBuf::from("apps/web"));
}

// ---------------------------------------------------------------------------
// Extends
// ---------------------------------------------------------------------------

#[test]
fn extends_resolves_from_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path().join("base.toml");
    std::fs::write(
        &base,
        r#"
[hooks.pre-commit.jobs.lint]
run = "eslint"
"#,
    )
    .unwrap();

    let main = dir.path().join("betterhook.toml");
    std::fs::write(
        &main,
        format!(
            "extends = [\"{}\"]\n\n[hooks.pre-push.jobs.test]\nrun = \"cargo test\"\n",
            base.display()
        ),
    )
    .unwrap();

    let raw = betterhook::config::resolve(&main).unwrap();
    assert!(raw.hooks.contains_key("pre-commit"), "should inherit from base");
    assert!(raw.hooks.contains_key("pre-push"), "should keep own hooks");
}

#[test]
fn extends_circular_detection() {
    let dir = tempfile::TempDir::new().unwrap();
    let a = dir.path().join("a.toml");
    let b = dir.path().join("b.toml");
    std::fs::write(&a, format!("extends = [\"{}\"]", b.display())).unwrap();
    std::fs::write(&b, format!("extends = [\"{}\"]", a.display())).unwrap();
    assert!(
        betterhook::config::resolve(&a).is_err(),
        "circular extends should error"
    );
}

// ---------------------------------------------------------------------------
// load() integration
// ---------------------------------------------------------------------------

#[test]
fn load_valid_config_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("betterhook.toml");
    std::fs::write(
        &path,
        r#"
[meta]
version = 1

[hooks.pre-commit.jobs.lint]
run = "eslint"
"#,
    )
    .unwrap();
    let cfg = load(&path).unwrap();
    assert_eq!(cfg.hooks.len(), 1);
}

#[test]
fn load_nonexistent_file_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("no-such-file.toml");
    assert!(load(&path).is_err());
}

// ---------------------------------------------------------------------------
// IsolateSpec parsing
// ---------------------------------------------------------------------------

#[test]
fn isolate_tool_variant_parses() {
    let src = r#"
[hooks.pre-commit.jobs.lint]
run = "eslint"
isolate = "eslint"
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    let cfg = raw.lower().unwrap();
    let job = &cfg.hooks["pre-commit"].jobs[0];
    assert!(job.isolate.is_some());
}

// ---------------------------------------------------------------------------
// Stash untracked
// ---------------------------------------------------------------------------

#[test]
fn stash_untracked_field_parses() {
    let src = r#"
[hooks.pre-commit]
stash_untracked = true

[hooks.pre-commit.jobs.lint]
run = "eslint"
"#;
    let raw = parse_bytes(src, Format::Toml, "test.toml").unwrap();
    let cfg = raw.lower().unwrap();
    assert!(cfg.hooks["pre-commit"].stash_untracked);
}
