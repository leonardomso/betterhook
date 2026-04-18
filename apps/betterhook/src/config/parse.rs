//! Format-agnostic config file parser.

use std::fs;
use std::path::Path;

use miette::{NamedSource, SourceOffset, SourceSpan};

use super::schema::RawConfig;
use crate::error::{ConfigError, ConfigResult};

/// Supported config file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Toml,
    Yaml,
    Json,
    Kdl,
}

impl Format {
    /// Infer a format from a file path's extension.
    pub fn from_path(path: &Path) -> ConfigResult<Self> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("toml") => Ok(Self::Toml),
            Some("yml" | "yaml") => Ok(Self::Yaml),
            Some("json") => Ok(Self::Json),
            Some("kdl") => Ok(Self::Kdl),
            _ => Err(ConfigError::UnsupportedFormat {
                path: path.to_path_buf(),
            }),
        }
    }
}

/// Read and parse a config file from disk.
pub fn parse_file(path: &Path) -> ConfigResult<RawConfig> {
    let format = Format::from_path(path)?;
    let source = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let name = path.display().to_string();
    parse_bytes(&source, format, &name)
}

/// Parse config bytes in a specified format.
///
/// `name` is used in diagnostic output (typically the file path).
pub fn parse_bytes(source: &str, format: Format, name: &str) -> ConfigResult<RawConfig> {
    match format {
        Format::Toml => parse_toml(source, name),
        Format::Yaml => parse_yaml(source, name),
        Format::Json => parse_json(source, name),
        Format::Kdl => super::kdl::parse_kdl(source, name),
    }
}

fn parse_toml(source: &str, name: &str) -> ConfigResult<RawConfig> {
    toml::from_str::<RawConfig>(source).map_err(|err| {
        let span = err.span().map(std_range_to_span);
        ConfigError::Toml {
            message: err.message().to_owned(),
            src: NamedSource::new(name, source.to_owned()),
            span,
        }
    })
}

fn parse_yaml(source: &str, name: &str) -> ConfigResult<RawConfig> {
    serde_yaml_ng::from_str::<RawConfig>(source).map_err(|err| {
        let span = err.location().map(|loc| {
            let offset = SourceOffset::from_location(source, loc.line(), loc.column());
            SourceSpan::new(offset, 0)
        });
        ConfigError::Yaml {
            message: err.to_string(),
            src: NamedSource::new(name, source.to_owned()),
            span,
        }
    })
}

fn parse_json(source: &str, name: &str) -> ConfigResult<RawConfig> {
    serde_json::from_str::<RawConfig>(source).map_err(|err| {
        let span = if err.line() > 0 {
            let offset = SourceOffset::from_location(source, err.line(), err.column());
            Some(SourceSpan::new(offset, 0))
        } else {
            None
        };
        ConfigError::Json {
            message: err.to_string(),
            src: NamedSource::new(name, source.to_owned()),
            span,
        }
    })
}

fn std_range_to_span(range: std::ops::Range<usize>) -> SourceSpan {
    SourceSpan::new(SourceOffset::from(range.start), range.end - range.start)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    const YAML_SOURCE: &str = r#"
meta:
  version: 1
hooks:
  pre-commit:
    parallel: true
    priority:
      - lint
      - test
    jobs:
      lint:
        run: "eslint --cache --fix {staged_files}"
        glob:
          - "*.ts"
          - "*.tsx"
        stage_fixed: true
        isolate: "eslint"
        timeout: "60s"
      test:
        run: "cargo test --quiet"
        isolate:
          tool: "cargo"
          target_dir: "per-worktree"
"#;

    const JSON_SOURCE: &str = r#"
{
  "meta": { "version": 1 },
  "hooks": {
    "pre-commit": {
      "parallel": true,
      "priority": ["lint", "test"],
      "jobs": {
        "lint": {
          "run": "eslint --cache --fix {staged_files}",
          "glob": ["*.ts", "*.tsx"],
          "stage_fixed": true,
          "isolate": "eslint",
          "timeout": "60s"
        },
        "test": {
          "run": "cargo test --quiet",
          "isolate": {
            "tool": "cargo",
            "target_dir": "per-worktree"
          }
        }
      }
    }
  }
}
"#;

    #[test]
    fn parses_all_three_formats_to_identical_config() {
        let from_toml = parse_bytes(TOML_SOURCE, Format::Toml, "betterhook.toml")
            .expect("toml parse")
            .lower()
            .expect("toml lower");
        let from_yaml = parse_bytes(YAML_SOURCE, Format::Yaml, "betterhook.yml")
            .expect("yaml parse")
            .lower()
            .expect("yaml lower");
        let from_json = parse_bytes(JSON_SOURCE, Format::Json, "betterhook.json")
            .expect("json parse")
            .lower()
            .expect("json lower");

        assert_eq!(
            from_toml, from_yaml,
            "toml and yaml must produce identical Config"
        );
        assert_eq!(
            from_yaml, from_json,
            "yaml and json must produce identical Config"
        );
    }

    #[test]
    fn toml_snapshot() {
        let config = parse_bytes(TOML_SOURCE, Format::Toml, "betterhook.toml")
            .unwrap()
            .lower()
            .unwrap();
        insta::assert_debug_snapshot!("parsed_config_from_toml", config);
    }

    #[test]
    fn priority_ordering_is_preserved() {
        let config = parse_bytes(TOML_SOURCE, Format::Toml, "betterhook.toml")
            .unwrap()
            .lower()
            .unwrap();
        let hook = &config.hooks["pre-commit"];
        let names: Vec<&str> = hook.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["lint", "test"]);
    }

    #[test]
    fn missing_run_is_an_error() {
        let source = r#"
[hooks.pre-commit.jobs.bad]
glob = ["*.ts"]
"#;
        let err = parse_bytes(source, Format::Toml, "t.toml")
            .unwrap()
            .lower()
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("'bad'"), "unexpected error: {msg}");
        assert!(msg.contains("run"), "unexpected error: {msg}");
    }

    #[test]
    fn bad_duration_is_an_error() {
        let source = r#"
[hooks.pre-commit.jobs.x]
run = "echo"
timeout = "not-a-duration"
"#;
        let err = parse_bytes(source, Format::Toml, "t.toml")
            .unwrap()
            .lower()
            .unwrap_err();
        assert!(format!("{err}").contains("duration"));
    }

    #[test]
    fn unknown_field_is_rejected() {
        let source = r#"
[hooks.pre-commit.jobs.x]
run = "echo"
totally_not_a_field = true
"#;
        let err = parse_bytes(source, Format::Toml, "t.toml").unwrap_err();
        assert!(format!("{err}").contains("unknown"));
    }
}
