//! husky → betterhook importer.
//!
//! husky doesn't have a structured config file. Each hook lives at
//! `.husky/<hook-name>` as a shell script. We pattern-match the most
//! common shapes — `npx lint-staged`, `npx prettier --write`, plain
//! `eslint .` calls — and turn each into a betterhook job. Anything we
//! don't recognize lands as a single passthrough job named after the
//! script's basename, and the [`MigrationReport`] flags it for review.

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::schema::{RawConfig, RawHook, RawJob, RawMeta};
use crate::error::ConfigResult;

use super::MigrationReport;

/// Build a `RawConfig` from a single husky script's contents.
pub fn from_script(source: &str, path: &Path) -> ConfigResult<(RawConfig, MigrationReport)> {
    let mut report = MigrationReport::default();
    let hook_name = path.file_name().map_or_else(
        || "pre-commit".to_owned(),
        |f| f.to_string_lossy().into_owned(),
    );

    let mut jobs: BTreeMap<String, RawJob> = BTreeMap::new();
    let mut idx = 0usize;
    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with("set ")
            || line.starts_with(". ")
            || line == "."
            || line.starts_with("source ")
            || line.starts_with("export ")
        {
            continue;
        }
        // Strip leading `npx ` / `pnpm exec ` / `yarn ` so the
        // dispatched command starts with the actual tool.
        let stripped = strip_runner(line);
        if stripped.is_empty() {
            continue;
        }
        idx += 1;
        let job_name = job_name_from(stripped, idx);
        jobs.insert(
            job_name,
            RawJob {
                run: Some(stripped.to_owned()),
                fix: None,
                glob: Vec::new(),
                exclude: Vec::new(),
                tags: Vec::new(),
                skip: None,
                only: None,
                env: BTreeMap::new(),
                root: None,
                stage_fixed: None,
                isolate: None,
                timeout: None,
                interactive: None,
                fail_text: None,
                reads: Vec::new(),
                writes: Vec::new(),
                network: None,
                concurrent_safe: None,
            },
        );
    }
    if jobs.is_empty() {
        report.note(format!(
            "{}: no recognizable commands; emitted an empty hook block",
            path.display()
        ));
    }

    let mut hooks = BTreeMap::new();
    hooks.insert(
        hook_name.clone(),
        RawHook {
            parallel: None,
            fail_fast: None,
            priority: Vec::new(),
            stash_untracked: None,
            parallel_limit: None,
            jobs,
        },
    );
    report.note(format!(
        "imported husky script `{}` into `{hook_name}` — review and add capability fields (`reads`, `writes`, `concurrent_safe`) to enable the DAG scheduler",
        path.display()
    ));

    Ok((
        RawConfig {
            meta: Some(RawMeta {
                version: Some(1),
                min_betterhook: None,
            }),
            extends: Vec::new(),
            hooks,
            packages: BTreeMap::new(),
        },
        report,
    ))
}

fn strip_runner(line: &str) -> &str {
    for prefix in [
        "npx ",
        "pnpm exec ",
        "pnpm dlx ",
        "yarn dlx ",
        "yarn run ",
        "bun x ",
        "bunx ",
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest;
        }
    }
    line
}

fn job_name_from(cmd: &str, idx: usize) -> String {
    let token = cmd.split_whitespace().next().unwrap_or("job");
    let base = token.rsplit('/').next().unwrap_or(token);
    if base.is_empty() {
        format!("step-{idx}")
    } else {
        format!("{base}-{idx}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_lint_staged_script() {
        let src = "#!/usr/bin/env sh\n. \"$(dirname \"$0\")/_/husky.sh\"\n\nnpx lint-staged\nnpx prettier --check .\n";
        let path = PathBuf::from(".husky/pre-commit");
        let (raw, report) = from_script(src, &path).unwrap();
        let cfg = raw.lower().unwrap();
        let hook = &cfg.hooks["pre-commit"];
        assert_eq!(hook.jobs.len(), 2);
        assert!(hook.jobs[0].run.contains("lint-staged"));
        assert!(hook.jobs[1].run.contains("prettier"));
        assert!(report.notes.iter().any(|n| n.contains("husky")));
    }

    #[test]
    fn empty_script_emits_empty_hook() {
        let src = "#!/usr/bin/env sh\nset -e\n";
        let path = PathBuf::from(".husky/pre-push");
        let (raw, report) = from_script(src, &path).unwrap();
        let cfg = raw.lower().unwrap();
        assert!(cfg.hooks["pre-push"].jobs.is_empty());
        assert!(report.notes.iter().any(|n| n.contains("no recognizable")));
    }
}
