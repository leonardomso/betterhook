//! Extends resolution and `betterhook.local.*` override layering.
//!
//! The public entry point is [`resolve`], which returns a flattened
//! [`RawConfig`] with every `extends` already merged in (depth-first,
//! overlay-wins) and any sibling `betterhook.local.*` override applied
//! last.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::parse::parse_file;
use super::schema::RawConfig;
use crate::error::{ConfigError, ConfigResult};

const LOCAL_CANDIDATE_EXTENSIONS: &[&str] = &["toml", "yml", "yaml", "json", "kdl"];

/// Resolve a config file, flattening all `extends` and applying any
/// sibling `betterhook.local.*` override.
pub fn resolve(path: &Path) -> ConfigResult<RawConfig> {
    let mut seen = BTreeSet::new();
    resolve_impl(path, &mut seen, true)
}

fn resolve_impl(
    path: &Path,
    seen: &mut BTreeSet<PathBuf>,
    apply_local: bool,
) -> ConfigResult<RawConfig> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !seen.insert(canonical.clone()) {
        return Err(ConfigError::Invalid {
            message: format!(
                "circular extends detected at {} (visited via {})",
                path.display(),
                canonical.display()
            ),
        });
    }

    let main = parse_file(path)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

    // Resolve each extends path recursively (no local override on those).
    let mut merged = RawConfig::default();
    for ext in &main.extends {
        let abs = if ext.is_absolute() {
            ext.clone()
        } else {
            base_dir.join(ext)
        };
        let resolved = resolve_impl(&abs, seen, false)?;
        merged.merge_overlay(resolved);
    }

    // Apply the main file's own content on top of its extends.
    let mut main_content = main;
    main_content.extends.clear();
    merged.merge_overlay(main_content);

    // Apply the sibling betterhook.local.* override, if one exists.
    if apply_local && let Some(local_path) = find_local_override(path)? {
        let local = parse_file(&local_path)?;
        merged.merge_overlay(local);
    }

    seen.remove(&canonical);
    Ok(merged)
}

/// Find a sibling `betterhook.local.*` file next to `main_path`.
fn find_local_override(main_path: &Path) -> ConfigResult<Option<PathBuf>> {
    let dir = main_path.parent().unwrap_or_else(|| Path::new("."));
    let mut found = None;
    for ext in LOCAL_CANDIDATE_EXTENSIONS {
        let candidate = dir.join(format!("betterhook.local.{ext}"));
        if candidate.is_file() {
            if let Some(prev) = &found {
                return Err(ConfigError::Invalid {
                    message: format!(
                        "multiple betterhook.local.* files found: {} and {}",
                        PathBuf::from(prev).display(),
                        candidate.display()
                    ),
                });
            }
            found = Some(candidate);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn extends_merges_jobs_from_base_and_main() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "base.toml",
            r#"
[hooks.pre-commit]
parallel = true

[hooks.pre-commit.jobs.lint]
run = "eslint {staged_files}"

[hooks.pre-commit.jobs.fmt]
run = "prettier --write {staged_files}"
"#,
        );
        let main = write(
            dir.path(),
            "betterhook.toml",
            r#"
extends = ["./base.toml"]

[hooks.pre-commit.jobs.test]
run = "cargo test"
"#,
        );

        let merged = resolve(&main).unwrap();
        let lowered = merged.lower().unwrap();
        let hook = &lowered.hooks["pre-commit"];
        let names: Vec<&str> = hook.jobs.iter().map(|j| j.name.as_str()).collect();
        assert!(hook.parallel, "parallel should be inherited from base");
        assert_eq!(names, vec!["fmt", "lint", "test"]);
    }

    #[test]
    fn main_overrides_field_from_base() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "base.toml",
            r#"
[hooks.pre-commit.jobs.lint]
run = "eslint {staged_files}"
timeout = "30s"
"#,
        );
        let main = write(
            dir.path(),
            "betterhook.toml",
            r#"
extends = ["./base.toml"]

[hooks.pre-commit.jobs.lint]
timeout = "2m"
"#,
        );

        let merged = resolve(&main).unwrap();
        let lowered = merged.lower().unwrap();
        let lint = &lowered.hooks["pre-commit"].jobs[0];
        assert_eq!(lint.name, "lint");
        assert_eq!(lint.run, "eslint {staged_files}"); // inherited from base
        assert_eq!(lint.timeout, Some(std::time::Duration::from_mins(2)));
    }

    #[test]
    fn cross_format_extends_toml_extending_yaml() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "base.yml",
            r#"
hooks:
  pre-commit:
    parallel: true
    jobs:
      lint:
        run: "eslint {staged_files}"
"#,
        );
        let main = write(
            dir.path(),
            "betterhook.toml",
            r#"
extends = ["./base.yml"]

[hooks.pre-commit.jobs.test]
run = "cargo test"
"#,
        );
        let merged = resolve(&main).unwrap().lower().unwrap();
        let hook = &merged.hooks["pre-commit"];
        assert!(hook.parallel);
        let names: Vec<&str> = hook.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["lint", "test"]);
    }

    #[test]
    fn local_override_wins_over_main() {
        let dir = tempdir().unwrap();
        let main = write(
            dir.path(),
            "betterhook.toml",
            r#"
[hooks.pre-commit.jobs.lint]
run = "eslint {staged_files}"
timeout = "30s"
"#,
        );
        write(
            dir.path(),
            "betterhook.local.toml",
            r#"
[hooks.pre-commit.jobs.lint]
timeout = "5m"
"#,
        );
        let merged = resolve(&main).unwrap().lower().unwrap();
        let lint = &merged.hooks["pre-commit"].jobs[0];
        assert_eq!(lint.timeout, Some(std::time::Duration::from_mins(5)));
    }

    #[test]
    fn circular_extends_is_detected() {
        let dir = tempdir().unwrap();
        let a = write(
            dir.path(),
            "a.toml",
            r#"
extends = ["./b.toml"]

[hooks.pre-commit.jobs.x]
run = "true"
"#,
        );
        write(
            dir.path(),
            "b.toml",
            r#"
extends = ["./a.toml"]
"#,
        );
        let err = resolve(&a).unwrap_err();
        assert!(format!("{err}").contains("circular"));
    }

    #[test]
    fn env_merges_key_by_key() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "base.toml",
            r#"
[hooks.pre-commit.jobs.build]
run = "make"
env = { CC = "gcc", JOBS = "4" }
"#,
        );
        let main = write(
            dir.path(),
            "betterhook.toml",
            r#"
extends = ["./base.toml"]

[hooks.pre-commit.jobs.build]
env = { JOBS = "8", RUSTFLAGS = "-Cdebuginfo=0" }
"#,
        );
        let merged = resolve(&main).unwrap().lower().unwrap();
        let build = &merged.hooks["pre-commit"].jobs[0];
        assert_eq!(build.env.get("CC").unwrap(), "gcc");
        assert_eq!(build.env.get("JOBS").unwrap(), "8");
        assert_eq!(build.env.get("RUSTFLAGS").unwrap(), "-Cdebuginfo=0");
    }
}
