//! KDL config parser.
//!
//! KDL has no serde support of its own, so we walk the parsed
//! `KdlDocument` by hand and build a `RawConfig`. Field names accept
//! both `kebab-case` and `snake_case`; both normalize to `snake_case`.
//!
//! Example schema shape (v2 KDL):
//!
//! ```text
//! meta {
//!     version 1
//!     min-betterhook ">=1.0"
//! }
//!
//! extends "./base.toml" "./overrides.yml"
//!
//! hook "pre-commit" {
//!     parallel #true
//!     fail-fast #false
//!     priority "lint" "test"
//!
//!     job "lint" {
//!         run "eslint --fix {staged_files}"
//!         fix "eslint --fix {files}"
//!         glob "*.ts" "*.tsx"
//!         stage-fixed #true
//!         isolate "eslint"
//!         timeout "60s"
//!     }
//!
//!     job "test" {
//!         run "cargo test"
//!         isolate tool="cargo" target-dir="per-worktree"
//!     }
//! }
//! ```

use std::path::PathBuf;

// Absolute crate path to avoid a name collision with our own
// `config::kdl` submodule.
use ::kdl::{KdlDocument, KdlEntry, KdlNode};
use miette::NamedSource;

use super::schema::{RawConfig, RawHook, RawIsolate, RawIsolateTable, RawJob, RawMeta};
use crate::error::{ConfigError, ConfigResult};

/// Parse a KDL source string into a [`RawConfig`].
///
/// `source_name` is used in error messages.
pub fn parse_kdl(source: &str, source_name: &str) -> ConfigResult<RawConfig> {
    let doc: KdlDocument = source
        .parse()
        .map_err(|e: ::kdl::KdlError| ConfigError::Kdl {
            message: e.to_string(),
            src: NamedSource::new(source_name, source.to_owned()),
            span: None,
        })?;

    let mut config = RawConfig::default();

    for node in doc.nodes() {
        match node.name().value() {
            "meta" => {
                config.meta = Some(parse_meta(node)?);
            }
            "extends" => {
                for entry in node.entries() {
                    if entry.name().is_some() {
                        continue;
                    }
                    if let Some(s) = entry.value().as_string() {
                        config.extends.push(PathBuf::from(s));
                    }
                }
            }
            "hook" => {
                let (name, hook) = parse_hook(node)?;
                config.hooks.insert(name, hook);
            }
            other => {
                return Err(ConfigError::Invalid {
                    message: format!("unknown top-level KDL node: {other}"),
                });
            }
        }
    }

    Ok(config)
}

fn parse_meta(node: &KdlNode) -> ConfigResult<RawMeta> {
    let mut meta = RawMeta::default();
    let Some(children) = node.children() else {
        return Ok(meta);
    };
    for child in children.nodes() {
        match normalize_field(child.name().value()).as_str() {
            "version" => {
                meta.version = first_positional_integer(child)?;
            }
            "min_betterhook" => {
                meta.min_betterhook = first_positional_string(child);
            }
            other => {
                return Err(ConfigError::Invalid {
                    message: format!("unknown meta field: {other}"),
                });
            }
        }
    }
    Ok(meta)
}

fn parse_hook(node: &KdlNode) -> ConfigResult<(String, RawHook)> {
    let name = first_positional_string(node).ok_or_else(|| ConfigError::Invalid {
        message: "hook node requires a name (e.g. `hook \"pre-commit\"`)".to_owned(),
    })?;
    let mut hook = RawHook::default();
    let Some(children) = node.children() else {
        return Ok((name, hook));
    };
    for child in children.nodes() {
        match normalize_field(child.name().value()).as_str() {
            "parallel" => hook.parallel = first_positional_bool(child),
            "fail_fast" => hook.fail_fast = first_positional_bool(child),
            "stash_untracked" => hook.stash_untracked = first_positional_bool(child),
            "parallel_limit" => {
                hook.parallel_limit =
                    first_positional_integer(child)?.map(|v| v.try_into().unwrap_or(usize::MAX));
            }
            "priority" => {
                hook.priority = all_positional_strings(child);
            }
            "job" => {
                let (job_name, job) = parse_job(child)?;
                hook.jobs.insert(job_name, job);
            }
            other => {
                return Err(ConfigError::Invalid {
                    message: format!("unknown hook field: {other}"),
                });
            }
        }
    }
    Ok((name, hook))
}

fn parse_job(node: &KdlNode) -> ConfigResult<(String, RawJob)> {
    let name = first_positional_string(node).ok_or_else(|| ConfigError::Invalid {
        message: "job node requires a name (e.g. `job \"lint\"`)".to_owned(),
    })?;
    let mut job = RawJob::default();
    let Some(children) = node.children() else {
        return Ok((name, job));
    };
    for child in children.nodes() {
        match normalize_field(child.name().value()).as_str() {
            "run" => job.run = first_positional_string(child),
            "fix" => job.fix = first_positional_string(child),
            "glob" => job.glob = all_positional_strings(child),
            "exclude" => job.exclude = all_positional_strings(child),
            "tags" => job.tags = all_positional_strings(child),
            "skip" => job.skip = first_positional_string(child),
            "only" => job.only = first_positional_string(child),
            "root" => job.root = first_positional_string(child).map(PathBuf::from),
            "stage_fixed" => job.stage_fixed = first_positional_bool(child),
            "interactive" => job.interactive = first_positional_bool(child),
            "timeout" => job.timeout = first_positional_string(child),
            "fail_text" => job.fail_text = first_positional_string(child),
            "reads" => job.reads = all_positional_strings(child),
            "writes" => job.writes = all_positional_strings(child),
            "network" => job.network = first_positional_bool(child),
            "concurrent_safe" => job.concurrent_safe = first_positional_bool(child),
            "builtin" => job.builtin = first_positional_string(child),
            "env" => {
                // `env KEY="value" OTHER="value"` — all properties.
                for entry in child.entries() {
                    if let Some(ident) = entry.name()
                        && let Some(value) = entry.value().as_string()
                    {
                        job.env.insert(ident.value().to_owned(), value.to_owned());
                    }
                }
            }
            "isolate" => job.isolate = Some(parse_isolate(child)?),
            other => {
                return Err(ConfigError::Invalid {
                    message: format!("unknown job field: {other}"),
                });
            }
        }
    }
    Ok((name, job))
}

fn parse_isolate(node: &KdlNode) -> ConfigResult<RawIsolate> {
    // Shorthand: `isolate "eslint"` — a single positional string.
    let positional: Vec<&KdlEntry> = node
        .entries()
        .iter()
        .filter(|e| e.name().is_none())
        .collect();
    let properties: Vec<(&str, &KdlEntry)> = node
        .entries()
        .iter()
        .filter_map(|e| e.name().map(|n| (n.value(), e)))
        .collect();

    if properties.is_empty()
        && positional.len() == 1
        && let Some(s) = positional[0].value().as_string()
    {
        return Ok(RawIsolate::Name(s.to_owned()));
    }

    // Table form: `isolate tool="cargo" target-dir="per-worktree"`
    let mut table = RawIsolateTable::default();
    for (key, entry) in properties {
        match normalize_field(key).as_str() {
            "name" => {
                table.name = entry.value().as_string().map(str::to_owned);
            }
            "tool" => {
                table.tool = entry.value().as_string().map(str::to_owned);
            }
            "slots" => {
                table.slots = entry
                    .value()
                    .as_integer()
                    .and_then(|i| usize::try_from(i).ok());
            }
            "target_dir" => {
                table.target_dir = entry.value().as_string().map(str::to_owned);
            }
            other => {
                return Err(ConfigError::Invalid {
                    message: format!("unknown isolate property: {other}"),
                });
            }
        }
    }
    Ok(RawIsolate::Table(table))
}

// ============================================================================
// Small helpers
// ============================================================================

/// Convert `kebab-case` field names to `snake_case`. Both are accepted
/// in KDL source so users can pick whichever reads best.
fn normalize_field(name: &str) -> String {
    name.replace('-', "_")
}

fn first_positional_string(node: &KdlNode) -> Option<String> {
    for entry in node.entries() {
        if entry.name().is_none()
            && let Some(s) = entry.value().as_string()
        {
            return Some(s.to_owned());
        }
    }
    None
}

fn all_positional_strings(node: &KdlNode) -> Vec<String> {
    node.entries()
        .iter()
        .filter(|e| e.name().is_none())
        .filter_map(|e| e.value().as_string().map(str::to_owned))
        .collect()
}

fn first_positional_bool(node: &KdlNode) -> Option<bool> {
    for entry in node.entries() {
        if entry.name().is_none()
            && let Some(b) = entry.value().as_bool()
        {
            return Some(b);
        }
    }
    None
}

fn first_positional_integer(node: &KdlNode) -> ConfigResult<Option<u32>> {
    for entry in node.entries() {
        if entry.name().is_none()
            && let Some(i) = entry.value().as_integer()
        {
            return u32::try_from(i)
                .map(Some)
                .map_err(|_| ConfigError::Invalid {
                    message: format!(
                        "integer {i} out of range for field `{}`",
                        node.name().value()
                    ),
                });
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use crate::config::parse::{Format, parse_bytes};

    const KDL_SOURCE: &str = r#"
meta {
    version 1
}

hook "pre-commit" {
    parallel #true
    priority "lint" "test"

    job "lint" {
        run "eslint --cache --fix {staged_files}"
        glob "*.ts" "*.tsx"
        stage-fixed #true
        isolate "eslint"
        timeout "60s"
    }

    job "test" {
        run "cargo test --quiet"
        isolate tool="cargo" target-dir="per-worktree"
    }
}
"#;

    const TOML_SOURCE: &str = r#"
[meta]
version = 1

[hooks.pre-commit]
parallel = true
priority = ["lint", "test"]

[hooks.pre-commit.jobs.lint]
run = "eslint --cache --fix {staged_files}"
glob = ["*.ts", "*.tsx"]
stage_fixed = true
isolate = "eslint"
timeout = "60s"

[hooks.pre-commit.jobs.test]
run = "cargo test --quiet"

[hooks.pre-commit.jobs.test.isolate]
tool = "cargo"
target_dir = "per-worktree"
"#;

    #[test]
    fn kdl_parses_and_lowers_identically_to_toml() {
        let from_kdl = parse_bytes(KDL_SOURCE, Format::Kdl, "betterhook.kdl")
            .unwrap()
            .lower()
            .unwrap();
        let from_toml = parse_bytes(TOML_SOURCE, Format::Toml, "betterhook.toml")
            .unwrap()
            .lower()
            .unwrap();
        assert_eq!(
            from_kdl, from_toml,
            "KDL and TOML must lower to identical Config"
        );
    }

    #[test]
    fn kdl_snapshot() {
        let config = parse_bytes(KDL_SOURCE, Format::Kdl, "betterhook.kdl")
            .unwrap()
            .lower()
            .unwrap();
        insta::assert_debug_snapshot!("parsed_config_from_kdl", config);
    }

    #[test]
    fn kdl_accepts_snake_case_fields() {
        let source = r#"
hook "pre-commit" {
    parallel #true
    job "lint" {
        run "eslint"
        stage_fixed #true
    }
}
"#;
        let cfg = parse_bytes(source, Format::Kdl, "t.kdl")
            .unwrap()
            .lower()
            .unwrap();
        assert!(cfg.hooks["pre-commit"].jobs[0].stage_fixed);
    }

    #[test]
    fn kdl_rejects_unknown_top_level_node() {
        let source = r#"
mystery "value"
"#;
        let err = parse_bytes(source, Format::Kdl, "t.kdl").unwrap_err();
        assert!(format!("{err}").contains("mystery"));
    }
}
