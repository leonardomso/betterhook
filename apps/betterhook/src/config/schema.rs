//! Typed config schema.
//!
//! Two tiers:
//! - [`RawConfig`] is what serde deserializes from TOML/YAML/JSON. Every
//!   field is `Option` or `Default` so configs are forgiving.
//! - [`Config`] is the canonical, validated representation the runner uses.
//!   Call [`RawConfig::lower`] to produce one.

use std::collections::BTreeMap;
use std::fmt;
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
    /// Monorepo packages. Each entry declares a directory path filter
    /// and optional per-package hook overlays that inherit from the
    /// root-level `hooks` map.
    #[serde(default)]
    pub packages: BTreeMap<String, RawPackage>,
}

impl RawConfig {
    /// Build a `RawConfig` pre-populated with schema version 1 meta
    /// and the given hooks map. Used by the importer modules so every
    /// importer doesn't re-spell the same `RawMeta` boilerplate.
    #[must_use]
    pub fn v1_from_hooks(hooks: BTreeMap<String, RawHook>) -> Self {
        Self {
            meta: Some(RawMeta {
                version: Some(1),
                ..RawMeta::default()
            }),
            hooks,
            ..Self::default()
        }
    }
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawPackage {
    pub path: PathBuf,
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
    pub parallel: Option<bool>,
    #[serde(default)]
    pub fail_fast: Option<bool>,
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
    pub stage_fixed: Option<bool>,
    #[serde(default)]
    pub isolate: Option<RawIsolate>,
    #[serde(default)]
    pub timeout: Option<String>,
    #[serde(default)]
    pub interactive: Option<bool>,
    #[serde(default)]
    pub fail_text: Option<String>,

    // Capability-DAG fields.
    #[serde(default)]
    pub reads: Vec<String>,
    #[serde(default)]
    pub writes: Vec<String>,
    #[serde(default)]
    pub network: Option<bool>,
    #[serde(default)]
    pub concurrent_safe: Option<bool>,

    /// Reference to a registered builtin (e.g. `"rustfmt"`, `"eslint"`).
    /// When present, the builtin's defaults are merged under the user's
    /// explicit fields at lower time, and the runner pipes subprocess
    /// output through the builtin's parser to emit `Diagnostic` events.
    #[serde(default)]
    pub builtin: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HookName(String);

impl HookName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for HookName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for HookName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for HookName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for HookName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobName(String);

impl JobName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for JobName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for JobName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for JobName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for JobName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackageName(String);

impl PackageName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for PackageName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for PackageName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for PackageName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub meta: Meta,
    pub hooks: BTreeMap<String, Hook>,
    /// Monorepo packages. Empty for single-package repos. Each
    /// package inherits the root hooks and may override them.
    pub packages: BTreeMap<String, Package>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: PackageName,
    pub path: PathBuf,
    /// Package-declared hooks. Dispatch overlays these on top of the
    /// root hooks when a package match is selected.
    pub hooks: BTreeMap<String, Hook>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub version: u32,
    pub min_betterhook: Option<semver::VersionReq>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    pub name: HookName,
    pub parallel: bool,
    pub parallel_explicit: bool,
    pub fail_fast: bool,
    pub fail_fast_explicit: bool,
    pub parallel_limit: Option<usize>,
    pub stash_untracked: bool,
    pub stash_untracked_explicit: bool,
    /// Jobs in priority order (index 0 runs first when contending).
    pub jobs: Vec<Job>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub name: JobName,
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

    // Capability-DAG inputs. The resolver compiles `reads` and
    // `writes` into `GlobSet`s and uses them to decide which jobs can
    // run in parallel.
    /// Glob patterns describing files this job reads from. Used by
    /// the DAG resolver to detect read-after-write conflicts.
    pub reads: Vec<String>,
    /// Glob patterns describing files this job writes. Used by the
    /// DAG resolver to detect write-write and read-after-write
    /// conflicts.
    pub writes: Vec<String>,
    /// True if this job reaches the network. Network jobs are
    /// serialized behind a shared lock unless `concurrent_safe`.
    pub network: bool,
    /// True if this job is safe to run speculatively on file save from
    /// the daemon watcher. Safe means: no network, no unrelated writes,
    /// and idempotent behavior.
    pub concurrent_safe: bool,
    /// If set, names a registered builtin whose `parse_output` is
    /// called on the subprocess stdout to emit structured `Diagnostic`
    /// events alongside the raw line output. The builtin's defaults
    /// were already merged at lower time; this field tells the runner
    /// *which* parser to use at execution time.
    pub builtin: Option<String>,
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
    #[must_use = "the lowered Config is needed for execution"]
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

        let mut packages = BTreeMap::new();
        for (pkg_name, raw_pkg) in self.packages {
            let mut pkg_hooks = BTreeMap::new();
            // Lower every hook the package declared. Dispatch overlays
            // these on top of the root hooks so packages can add or
            // replace per-job behavior.
            for (hook_name, raw_hook) in raw_pkg.hooks {
                let hook = lower_hook(&hook_name, raw_hook)?;
                pkg_hooks.insert(hook_name, hook);
            }
            packages.insert(
                pkg_name.clone(),
                Package {
                    name: pkg_name.into(),
                    path: raw_pkg.path,
                    hooks: pkg_hooks,
                },
            );
        }

        Ok(Config {
            meta,
            hooks,
            packages,
        })
    }
}

fn lower_hook(name: &str, raw: RawHook) -> ConfigResult<Hook> {
    let stash_untracked = raw.stash_untracked.unwrap_or(name == "pre-commit");
    let parallel = raw.parallel.unwrap_or(false);
    let fail_fast = raw.fail_fast.unwrap_or(false);

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
        name: name.into(),
        parallel,
        parallel_explicit: raw.parallel.is_some(),
        fail_fast,
        fail_fast_explicit: raw.fail_fast.is_some(),
        parallel_limit: raw.parallel_limit,
        stash_untracked,
        stash_untracked_explicit: raw.stash_untracked.is_some(),
        jobs,
    })
}

fn lower_job(name: &str, mut raw: RawJob, priority: u32) -> ConfigResult<Job> {
    // If a builtin is referenced, merge its defaults under the user's
    // explicit fields. User values always win; the builtin fills in
    // anything the user didn't set.
    let builtin_name = raw.builtin.clone();
    if let Some(ref id) = builtin_name {
        if let Some(meta) = crate::builtins::get(id) {
            if raw.run.is_none() {
                raw.run = Some(meta.run.to_owned());
            }
            if raw.fix.is_none() {
                raw.fix = meta.fix.map(str::to_owned);
            }
            if raw.glob.is_empty() {
                raw.glob = meta.glob.iter().map(|s| (*s).to_owned()).collect();
            }
            if raw.reads.is_empty() {
                raw.reads = meta.reads.iter().map(|s| (*s).to_owned()).collect();
            }
            if raw.writes.is_empty() {
                raw.writes = meta.writes.iter().map(|s| (*s).to_owned()).collect();
            }
            if raw.network.is_none() {
                raw.network = Some(meta.network);
            }
            if raw.concurrent_safe.is_none() {
                raw.concurrent_safe = Some(meta.concurrent_safe);
            }
        } else {
            return Err(ConfigError::Invalid {
                message: format!(
                    "job '{name}' references unknown builtin '{id}'; run `betterhook builtins list` for available builtins"
                ),
            });
        }
    }

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

    for pat in raw.reads.iter().chain(raw.writes.iter()) {
        globset::Glob::new(pat).map_err(|e| ConfigError::Invalid {
            message: format!("job '{name}' has an invalid capability glob '{pat}': {e}"),
        })?;
    }

    Ok(Job {
        name: name.into(),
        run,
        fix: raw.fix,
        glob: raw.glob,
        exclude: raw.exclude,
        tags: raw.tags,
        skip: raw.skip,
        only: raw.only,
        env: raw.env,
        root: raw.root,
        stage_fixed: raw.stage_fixed.unwrap_or(false),
        isolate,
        timeout,
        interactive: raw.interactive.unwrap_or(false),
        fail_text: raw.fail_text,
        priority,
        reads: raw.reads,
        writes: raw.writes,
        network: raw.network.unwrap_or(false),
        concurrent_safe: raw.concurrent_safe.unwrap_or(false),
        builtin: builtin_name,
    })
}

// ============================================================================
// Merge / overlay semantics (used by the extends resolver).
// ============================================================================

impl RawConfig {
    /// Layer `overlay` on top of `self`. Overlay fields win on conflict,
    /// but `hooks` and `jobs` maps merge recursively so partial overrides
    /// work naturally. The overlay's `extends` list is ignored — callers
    /// are expected to have already resolved it before calling this.
    pub fn merge_overlay(&mut self, overlay: RawConfig) {
        if let Some(overlay_meta) = overlay.meta {
            match &mut self.meta {
                Some(base_meta) => base_meta.merge_overlay(overlay_meta),
                slot @ None => *slot = Some(overlay_meta),
            }
        }
        for (hook_name, overlay_hook) in overlay.hooks {
            self.hooks
                .entry(hook_name)
                .and_modify(|base_hook| base_hook.merge_overlay(overlay_hook.clone()))
                .or_insert(overlay_hook);
        }
        for (pkg_name, overlay_pkg) in overlay.packages {
            self.packages
                .entry(pkg_name)
                .and_modify(|base_pkg| {
                    // Package path is replaced; hooks merge recursively.
                    overlay_pkg.path.clone_into(&mut base_pkg.path);
                    for (hook_name, hook) in overlay_pkg.hooks.clone() {
                        base_pkg
                            .hooks
                            .entry(hook_name)
                            .and_modify(|base_hook| base_hook.merge_overlay(hook.clone()))
                            .or_insert(hook);
                    }
                })
                .or_insert(overlay_pkg);
        }
    }
}

impl RawMeta {
    fn merge_overlay(&mut self, overlay: RawMeta) {
        if overlay.version.is_some() {
            self.version = overlay.version;
        }
        if overlay.min_betterhook.is_some() {
            self.min_betterhook = overlay.min_betterhook;
        }
    }
}

impl RawHook {
    /// Overlay-wins merge. Jobs with the same name merge recursively.
    pub fn merge_overlay(&mut self, overlay: RawHook) {
        if overlay.parallel.is_some() {
            self.parallel = overlay.parallel;
        }
        if overlay.fail_fast.is_some() {
            self.fail_fast = overlay.fail_fast;
        }
        if !overlay.priority.is_empty() {
            self.priority = overlay.priority;
        }
        if overlay.stash_untracked.is_some() {
            self.stash_untracked = overlay.stash_untracked;
        }
        if overlay.parallel_limit.is_some() {
            self.parallel_limit = overlay.parallel_limit;
        }
        for (job_name, overlay_job) in overlay.jobs {
            self.jobs
                .entry(job_name)
                .and_modify(|base_job| base_job.merge_overlay(overlay_job.clone()))
                .or_insert(overlay_job);
        }
    }
}

impl RawJob {
    /// Overlay-wins merge. `env` merges key-by-key; lists are replaced.
    pub fn merge_overlay(&mut self, overlay: RawJob) {
        macro_rules! take_if_some {
            ($field:ident) => {
                if overlay.$field.is_some() {
                    self.$field = overlay.$field;
                }
            };
        }
        take_if_some!(run);
        take_if_some!(fix);
        take_if_some!(skip);
        take_if_some!(only);
        take_if_some!(root);
        take_if_some!(stage_fixed);
        take_if_some!(isolate);
        take_if_some!(timeout);
        take_if_some!(interactive);
        take_if_some!(fail_text);
        take_if_some!(network);
        take_if_some!(concurrent_safe);
        take_if_some!(builtin);
        if !overlay.glob.is_empty() {
            self.glob = overlay.glob;
        }
        if !overlay.exclude.is_empty() {
            self.exclude = overlay.exclude;
        }
        if !overlay.tags.is_empty() {
            self.tags = overlay.tags;
        }
        if !overlay.reads.is_empty() {
            self.reads = overlay.reads;
        }
        if !overlay.writes.is_empty() {
            self.writes = overlay.writes;
        }
        for (k, v) in overlay.env {
            self.env.insert(k, v);
        }
    }
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
