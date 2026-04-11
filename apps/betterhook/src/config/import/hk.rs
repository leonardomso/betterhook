//! hk → betterhook importer.
//!
//! `hk` (jdx/hk) ships a `hk.toml` schema with `[hooks.<name>.<job>]`
//! tables that closely mirror our own. We rename `hk`-specific fields
//! (`steps`, `command`) to their betterhook equivalents and emit notes
//! for anything that doesn't have a 1:1 mapping.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::schema::{RawConfig, RawHook, RawJob};
use crate::error::{ConfigError, ConfigResult};

use super::MigrationReport;

#[derive(Debug, Deserialize, Default)]
struct HkRoot {
    #[serde(default)]
    hooks: BTreeMap<String, HkHook>,
}

#[derive(Debug, Deserialize, Default)]
struct HkHook {
    #[serde(default)]
    fix: Option<bool>,
    #[serde(default)]
    steps: BTreeMap<String, HkStep>,
}

#[derive(Debug, Deserialize, Default)]
struct HkStep {
    #[serde(default, alias = "command")]
    run: Option<String>,
    #[serde(default)]
    glob: Option<StringOrVec>,
    #[serde(default)]
    exclude: Option<StringOrVec>,
    #[serde(default, alias = "fix_command")]
    fix: Option<String>,
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

pub fn from_text(source: &str) -> ConfigResult<(RawConfig, MigrationReport)> {
    let mut report = MigrationReport::default();
    let root: HkRoot = toml::from_str(source).map_err(|e| ConfigError::Invalid {
        message: format!("failed to parse hk.toml: {e}"),
    })?;

    let mut hooks = BTreeMap::new();
    for (hook_name, hk_hook) in root.hooks {
        if hk_hook.fix.is_some() {
            report.note(format!(
                "{hook_name}: hk's hook-level `fix` field has no exact equivalent — set per-job `fix = ...` if needed"
            ));
        }
        let mut jobs = BTreeMap::new();
        for (job_name, step) in hk_hook.steps {
            let run = step.run.unwrap_or_else(|| {
                report.note(format!(
                    "{hook_name}/{job_name}: missing `run` — inserted `true`"
                ));
                "true".to_owned()
            });
            jobs.insert(
                job_name,
                RawJob {
                    run: Some(run),
                    fix: step.fix,
                    glob: step.glob.map(StringOrVec::into_vec).unwrap_or_default(),
                    exclude: step.exclude.map(StringOrVec::into_vec).unwrap_or_default(),
                    ..RawJob::default()
                },
            );
        }
        hooks.insert(
            hook_name,
            RawHook {
                jobs,
                ..RawHook::default()
            },
        );
    }

    report.note(
        "imported from hk.toml — review per-job capability fields (`reads`, `writes`, `concurrent_safe`) to enable the DAG scheduler"
            .to_owned(),
    );

    Ok((RawConfig::v1_from_hooks(hooks), report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_hk_toml() {
        let src = r#"
[hooks.pre-commit.steps.lint]
run = "eslint --cache {staged_files}"
glob = "*.ts"

[hooks.pre-commit.steps.test]
command = "cargo test"
"#;
        let (raw, report) = from_text(src).unwrap();
        let cfg = raw.lower().unwrap();
        let hook = &cfg.hooks["pre-commit"];
        assert_eq!(hook.jobs.len(), 2);
        let lint = hook
            .jobs
            .iter()
            .find(|j| j.name == "lint")
            .expect("lint job exists");
        assert!(lint.run.contains("eslint"));
        assert!(report.notes.iter().any(|n| n.contains("hk.toml")));
    }
}
