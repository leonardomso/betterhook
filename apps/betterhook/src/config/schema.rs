//! Typed config schema.
//!
//! Two tiers:
//! - [`RawConfig`] is what serde deserializes from TOML/YAML/JSON. Every
//!   field is `Option` or `Default` so configs are forgiving.
//! - [`Config`] is the canonical, validated representation the runner uses.
//!   Call [`RawConfig::lower`] to produce one.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, ConfigResult};

/// Raw deserialized config — the shape of the file on disk.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    #[serde(default)]
    pub meta: Option<RawMeta>,
    #[serde(default)]
    pub extends: Vec<PathBuf>,
    #[serde(default)]
    pub hooks: BTreeMap<String, RawHook>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawMeta {
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub min_betterhook: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawHook {
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub fail_fast: bool,
    #[serde(default)]
    pub priority: Vec<String>,
    #[serde(default)]
    pub stash_untracked: Option<bool>,
    #[serde(default)]
    pub parallel_limit: Option<usize>,
    #[serde(default)]
    pub jobs: BTreeMap<String, RawJob>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawJob {
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub fix: Option<String>,
    #[serde(default)]
    pub glob: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub skip: Option<String>,
    #[serde(default)]
    pub only: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub root: Option<PathBuf>,
    #[serde(default)]
    pub stage_fixed: bool,
    #[serde(default)]
    pub isolate: Option<RawIsolate>,
    #[serde(default)]
    pub timeout: Option<String>,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub fail_text: Option<String>,
}

/// Raw, serde-friendly isolation spec.
///
/// Accepts:
/// - a bare string (shorthand for a global tool mutex): `isolate = "eslint"`
/// - a table with `{ tool, target_dir }` for per-worktree path scoping
/// - a table with `{ name, slots }` for sharded mutexes
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RawIsolate {
    Name(String),
    Table(RawIsolateTable),
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawIsolateTable {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub slots: Option<usize>,
    #[serde(default)]
    pub target_dir: Option<String>,
}

// ============================================================================
// Canonical typed config.
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub meta: Meta,
    pub hooks: BTreeMap<String, Hook>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub version: u32,
    pub min_betterhook: Option<semver::VersionReq>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    pub name: String,
    pub parallel: bool,
    pub fail_fast: bool,
    pub parallel_limit: Option<usize>,
    pub stash_untracked: bool,
    /// Jobs in priority order (index 0 runs first when contending).
    pub jobs: Vec<Job>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub name: String,
    pub run: String,
    pub fix: Option<String>,
    pub glob: Vec<String>,
    pub exclude: Vec<String>,
    pub tags: Vec<String>,
    pub skip: Option<String>,
    pub only: Option<String>,
    pub env: BTreeMap<String, String>,
    pub root: Option<PathBuf>,
    pub stage_fixed: bool,
    pub isolate: Option<IsolateSpec>,
    pub timeout: Option<Duration>,
    pub interactive: bool,
    pub fail_text: Option<String>,
    pub priority: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsolateSpec {
    /// Global mutex keyed on a tool name, shared across all worktrees.
    Tool { name: String },
    /// Sharded semaphore with N permits.
    Sharded { name: String, slots: usize },
    /// Tool scoped to a per-path target dir. The runner auto-sets the
    /// corresponding environment variable (e.g. `CARGO_TARGET_DIR`).
    ToolPath {
        tool: String,
        target_dir: ToolPathScope,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPathScope {
    PerWorktree,
    Path(PathBuf),
}

// ============================================================================
// Lowering.
// ============================================================================

impl RawConfig {
    /// Validate and lower a raw config into the canonical typed representation.
    pub fn lower(self) -> ConfigResult<Config> {
        let meta = match self.meta {
            Some(m) => Meta {
                version: m.version.unwrap_or(1),
                min_betterhook: m
                    .min_betterhook
                    .map(|s| {
                        semver::VersionReq::parse(&s).map_err(|e| ConfigError::Invalid {
                            message: format!("meta.min_betterhook is not a valid semver req: {e}"),
                        })
                    })
                    .transpose()?,
            },
            None => Meta {
                version: 1,
                min_betterhook: None,
            },
        };

        let mut hooks = BTreeMap::new();
        for (hook_name, raw_hook) in self.hooks {
            let hook = lower_hook(&hook_name, raw_hook)?;
            hooks.insert(hook_name, hook);
        }

        Ok(Config { meta, hooks })
    }
}

fn lower_hook(name: &str, raw: RawHook) -> ConfigResult<Hook> {
    let stash_untracked = raw.stash_untracked.unwrap_or(name == "pre-commit");

    let priority_index: BTreeMap<&str, u32> = raw
        .priority
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), u32::try_from(i).unwrap_or(u32::MAX)))
        .collect();
    // Jobs not mentioned in `priority` run after those that are.
    let unlisted_priority = u32::try_from(raw.priority.len()).unwrap_or(u32::MAX);

    let mut jobs: Vec<Job> = raw
        .jobs
        .into_iter()
        .map(|(job_name, raw_job)| {
            let priority = priority_index
                .get(job_name.as_str())
                .copied()
                .unwrap_or(unlisted_priority);
            lower_job(&job_name, raw_job, priority)
        })
        .collect::<ConfigResult<_>>()?;

    jobs.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.name.cmp(&b.name)));

    Ok(Hook {
        name: name.to_owned(),
        parallel: raw.parallel,
        fail_fast: raw.fail_fast,
        parallel_limit: raw.parallel_limit,
        stash_untracked,
        jobs,
    })
}

fn lower_job(name: &str, raw: RawJob, priority: u32) -> ConfigResult<Job> {
    let run = raw.run.ok_or_else(|| ConfigError::Invalid {
        message: format!("job '{name}' is missing a 'run' command"),
    })?;

    let timeout = raw
        .timeout
        .as_deref()
        .map(|input| {
            humantime::parse_duration(input).map_err(|source| ConfigError::Duration {
                job: name.to_owned(),
                input: input.to_owned(),
                source,
            })
        })
        .transpose()?;

    let isolate = raw
        .isolate
        .map(|raw_iso| lower_isolate(name, raw_iso))
        .transpose()?;

    Ok(Job {
        name: name.to_owned(),
        run,
        fix: raw.fix,
        glob: raw.glob,
        exclude: raw.exclude,
        tags: raw.tags,
        skip: raw.skip,
        only: raw.only,
        env: raw.env,
        root: raw.root,
        stage_fixed: raw.stage_fixed,
        isolate,
        timeout,
        interactive: raw.interactive,
        fail_text: raw.fail_text,
        priority,
    })
}

fn lower_isolate(job: &str, raw: RawIsolate) -> ConfigResult<IsolateSpec> {
    match raw {
        RawIsolate::Name(name) => Ok(IsolateSpec::Tool { name }),
        RawIsolate::Table(table) => {
            if let Some(tool) = table.tool {
                let target_dir = match table.target_dir.as_deref() {
                    None | Some("per-worktree") => ToolPathScope::PerWorktree,
                    Some(other) => ToolPathScope::Path(PathBuf::from(other)),
                };
                Ok(IsolateSpec::ToolPath { tool, target_dir })
            } else if let Some(name) = table.name {
                if let Some(slots) = table.slots {
                    if slots == 0 {
                        return Err(ConfigError::Invalid {
                            message: format!("job '{job}' isolate.slots must be > 0"),
                        });
                    }
                    Ok(IsolateSpec::Sharded { name, slots })
                } else {
                    Ok(IsolateSpec::Tool { name })
                }
            } else {
                Err(ConfigError::Invalid {
                    message: format!("job '{job}' isolate must set either 'name' or 'tool'"),
                })
            }
        }
    }
}
