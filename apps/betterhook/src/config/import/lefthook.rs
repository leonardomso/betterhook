//! Lefthook YAML → betterhook `RawConfig` importer.
//!
//! This is a best-effort migrator, not a bug-compatible emulator —
//! lefthook features whose behavior we're improving (stale priority
//! handling, partial env semantics, etc.) are deliberately dropped and
//! listed in the [`MigrationReport`] so users know what to double-check.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::schema::{RawConfig, RawHook, RawIsolate, RawIsolateTable, RawJob};
use crate::error::{ConfigError, ConfigResult};

use super::MigrationReport;

#[derive(Debug, Default, Deserialize)]
struct LefthookHook {
    #[serde(default)]
    parallel: Option<bool>,
    #[serde(default)]
    follow: Option<bool>,
    #[serde(default)]
    commands: BTreeMap<String, LefthookCommand>,
}

#[derive(Debug, Default, Deserialize)]
struct LefthookCommand {
    #[serde(default)]
    run: Option<String>,
    #[serde(default)]
    glob: Option<StringOrVec>,
    #[serde(default)]
    exclude: Option<StringOrVec>,
    #[serde(default)]
    tags: Option<StringOrVec>,
    #[serde(default)]
    skip: Option<serde_yaml_ng::Value>,
    #[serde(default)]
    only: Option<serde_yaml_ng::Value>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    stage_fixed: Option<bool>,
    #[serde(default)]
    interactive: Option<bool>,
    #[serde(default)]
    fail_text: Option<String>,
    #[serde(default, alias = "pool")]
    isolate: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringOrVec {
    Single(String),
    Many(Vec<String>),
}

impl StringOrVec {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::Single(s) => vec![s],
            Self::Many(v) => v,
        }
    }
}

/// Convert lefthook YAML source into a betterhook `RawConfig` plus a
/// human-readable migration report.
pub fn from_yaml(source: &str) -> ConfigResult<(RawConfig, MigrationReport)> {
    let top: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(source).map_err(|e| ConfigError::Invalid {
            message: format!("failed to parse lefthook yaml: {e}"),
        })?;
    let mapping = match top {
        serde_yaml_ng::Value::Mapping(m) => m,
        serde_yaml_ng::Value::Null => serde_yaml_ng::Mapping::new(),
        _ => {
            return Err(ConfigError::Invalid {
                message: "lefthook yaml root must be a mapping".to_owned(),
            });
        }
    };

    let mut report = MigrationReport::default();
    let mut config = RawConfig::v1_from_hooks(BTreeMap::new());

    for (key, value) in mapping {
        let Some(hook_name) = key.as_str().map(str::to_owned) else {
            continue;
        };
        if !is_known_git_hook(&hook_name) {
            report.note(format!(
                "skipped top-level key '{hook_name}' — not a recognized git hook"
            ));
            continue;
        }
        let lh_hook: LefthookHook = match serde_yaml_ng::from_value(value) {
            Ok(h) => h,
            Err(e) => {
                report.note(format!(
                    "hook '{hook_name}': could not parse body ({e}); skipped"
                ));
                continue;
            }
        };
        let bh_hook = convert_hook(&hook_name, lh_hook, &mut report);
        config.hooks.insert(hook_name, bh_hook);
    }
    report.note(
        "extends / remotes / templates blocks are not supported by the migrator and were dropped"
            .to_owned(),
    );
    Ok((config, report))
}

fn convert_hook(name: &str, lh: LefthookHook, report: &mut MigrationReport) -> RawHook {
    let mut jobs = BTreeMap::new();
    for (job_name, lh_cmd) in lh.commands {
        let job = convert_command(name, &job_name, lh_cmd, report);
        jobs.insert(job_name, job);
    }
    RawHook {
        parallel: lh.parallel,
        fail_fast: lh.follow.map(|f| !f),
        jobs,
        ..RawHook::default()
    }
}

fn convert_command(
    hook_name: &str,
    job_name: &str,
    lh: LefthookCommand,
    report: &mut MigrationReport,
) -> RawJob {
    let mut run = lh.run.unwrap_or_default();
    if run.is_empty() {
        report.note(format!(
            "{hook_name}/{job_name}: no `run` value — betterhook requires one, inserted `true`"
        ));
        "true".clone_into(&mut run);
    }

    let skip = lh.skip.and_then(|v| match v {
        serde_yaml_ng::Value::String(s) => Some(s),
        other => {
            report.note(format!(
                "{hook_name}/{job_name}: non-string `skip` ({}) — dropped",
                type_name_of(&other)
            ));
            None
        }
    });
    let only = lh.only.and_then(|v| match v {
        serde_yaml_ng::Value::String(s) => Some(s),
        other => {
            report.note(format!(
                "{hook_name}/{job_name}: non-string `only` ({}) — dropped",
                type_name_of(&other)
            ));
            None
        }
    });

    let isolate = lh.isolate.map(|name| {
        RawIsolate::Table(RawIsolateTable {
            name: Some(name),
            ..RawIsolateTable::default()
        })
    });

    RawJob {
        run: Some(run),
        glob: lh.glob.map(StringOrVec::into_vec).unwrap_or_default(),
        exclude: lh.exclude.map(StringOrVec::into_vec).unwrap_or_default(),
        tags: lh.tags.map(StringOrVec::into_vec).unwrap_or_default(),
        skip,
        only,
        env: lh.env,
        root: lh.root.map(std::path::PathBuf::from),
        stage_fixed: lh.stage_fixed,
        isolate,
        interactive: lh.interactive,
        fail_text: lh.fail_text,
        ..RawJob::default()
    }
}

fn is_known_git_hook(name: &str) -> bool {
    matches!(
        name,
        "applypatch-msg"
            | "pre-applypatch"
            | "post-applypatch"
            | "pre-commit"
            | "pre-merge-commit"
            | "prepare-commit-msg"
            | "commit-msg"
            | "post-commit"
            | "pre-rebase"
            | "post-checkout"
            | "post-merge"
            | "pre-push"
            | "pre-receive"
            | "update"
            | "post-receive"
            | "post-update"
            | "push-to-checkout"
            | "pre-auto-gc"
            | "post-rewrite"
            | "sendemail-validate"
            | "fsmonitor-watchman"
            | "p4-changelist"
            | "p4-prepare-changelist"
            | "p4-post-changelist"
            | "p4-pre-submit"
            | "post-index-change"
            | "reference-transaction"
            | "proc-receive"
    )
}

fn type_name_of(v: &serde_yaml_ng::Value) -> &'static str {
    match v {
        serde_yaml_ng::Value::Null => "null",
        serde_yaml_ng::Value::Bool(_) => "bool",
        serde_yaml_ng::Value::Number(_) => "number",
        serde_yaml_ng::Value::String(_) => "string",
        serde_yaml_ng::Value::Sequence(_) => "sequence",
        serde_yaml_ng::Value::Mapping(_) => "mapping",
        serde_yaml_ng::Value::Tagged(_) => "tagged",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
pre-commit:
  parallel: true
  commands:
    lint:
      run: eslint --cache --fix {staged_files}
      glob: "*.ts"
      exclude: "**/*.gen.ts"
      stage_fixed: true
      tags: [javascript]
    test:
      run: cargo test --quiet
      fail_text: "tests must pass before commit"

pre-push:
  commands:
    audit:
      run: cargo audit
"#;

    #[test]
    fn sample_round_trips_through_lower() {
        let (raw, report) = from_yaml(SAMPLE).unwrap();
        let cfg = raw.lower().unwrap();

        let pre_commit = &cfg.hooks["pre-commit"];
        assert!(pre_commit.parallel);
        let names: Vec<&str> = pre_commit.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["lint", "test"]);
        assert_eq!(pre_commit.jobs[0].glob, vec!["*.ts".to_string()]);
        assert!(pre_commit.jobs[0].stage_fixed);
        assert_eq!(
            pre_commit.jobs[1].fail_text.as_deref(),
            Some("tests must pass before commit")
        );

        let pre_push = &cfg.hooks["pre-push"];
        assert_eq!(pre_push.jobs.len(), 1);
        assert_eq!(pre_push.jobs[0].run, "cargo audit");

        assert!(!report.notes.is_empty());
    }

    #[test]
    fn unknown_top_level_key_is_reported_and_skipped() {
        let doc = r#"
extends:
  - base.yml
pre-commit:
  commands:
    a:
      run: "true"
"#;
        let (raw, report) = from_yaml(doc).unwrap();
        let cfg = raw.lower().unwrap();
        assert!(cfg.hooks.contains_key("pre-commit"));
        assert!(!cfg.hooks.contains_key("extends"));
        assert!(report.notes.iter().any(|n| n.contains("extends")));
    }
}
