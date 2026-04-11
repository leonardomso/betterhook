//! `.pre-commit-config.yaml` → betterhook importer.
//!
//! pre-commit's config has a top-level `repos:` array of repos, each
//! with a `hooks:` array of named hooks. We collapse the whole thing
//! into a single betterhook `pre-commit` block where each pre-commit
//! hook becomes one betterhook job that shells out to `pre-commit run
//! <id>`. The user gets a working hook on day one and can later replace
//! the wrapped invocations with native commands.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::schema::{RawConfig, RawHook, RawJob};
use crate::error::{ConfigError, ConfigResult};

use super::MigrationReport;

#[derive(Debug, Deserialize, Default)]
struct PreCommitRoot {
    #[serde(default)]
    repos: Vec<PreCommitRepo>,
}

#[derive(Debug, Deserialize, Default)]
struct PreCommitRepo {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    hooks: Vec<PreCommitHook>,
}

#[derive(Debug, Deserialize, Default)]
struct PreCommitHook {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    files: Option<String>,
    #[serde(default)]
    exclude: Option<String>,
    #[serde(default)]
    args: Vec<String>,
}

pub fn from_yaml(source: &str) -> ConfigResult<(RawConfig, MigrationReport)> {
    let mut report = MigrationReport::default();
    let root: PreCommitRoot =
        serde_yaml_ng::from_str(source).map_err(|e| ConfigError::Invalid {
            message: format!("failed to parse .pre-commit-config.yaml: {e}"),
        })?;

    let mut jobs: BTreeMap<String, RawJob> = BTreeMap::new();
    for repo in root.repos {
        for hook in repo.hooks {
            let id = hook.id.clone();
            let pretty_name = hook.name.as_ref().unwrap_or(&id).clone();
            let extra_args = if hook.args.is_empty() {
                String::new()
            } else {
                format!(" {}", hook.args.join(" "))
            };
            // Use `pre-commit run --files {files}` for an exact replay
            // of the upstream hook on the staged files. Requires the
            // user to keep `pre-commit` installed; we flag that.
            let run = format!("pre-commit run {id}{extra_args} --files {{files}}");
            let glob = hook.files.into_iter().collect::<Vec<_>>();
            let exclude = hook.exclude.into_iter().collect::<Vec<_>>();
            jobs.insert(
                pretty_name.clone(),
                RawJob {
                    run: Some(run),
                    glob,
                    exclude,
                    tags: vec!["pre-commit".to_owned()],
                    ..RawJob::default()
                },
            );
            report.note(format!(
                "imported `{id}` from {} — wrapped with `pre-commit run`; replace with native command for full speed",
                repo.repo
            ));
        }
    }

    let mut hooks = BTreeMap::new();
    hooks.insert(
        "pre-commit".to_owned(),
        RawHook {
            jobs,
            ..RawHook::default()
        },
    );

    Ok((RawConfig::v1_from_hooks(hooks), report))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r"
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v4.6.0
    hooks:
      - id: trailing-whitespace
      - id: end-of-file-fixer
        files: '\.py$'
";

    #[test]
    fn parses_repo_array() {
        let (raw, report) = from_yaml(SAMPLE).unwrap();
        let cfg = raw.lower().unwrap();
        let hook = &cfg.hooks["pre-commit"];
        assert_eq!(hook.jobs.len(), 2);
        assert!(hook.jobs[0].run.contains("pre-commit run"));
        assert!(report.notes.iter().any(|n| n.contains("trailing-whitespace")));
    }
}
