use std::path::PathBuf;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum ConfigError {
    #[error("failed to read config file at {path}")]
    #[diagnostic(code(betterhook::config::io))]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("unsupported config format for {path}: expected .toml, .yml, .yaml, or .json")]
    #[diagnostic(
        code(betterhook::config::unsupported_format),
        help("rename the file to betterhook.toml, betterhook.yml, or betterhook.json")
    )]
    UnsupportedFormat { path: PathBuf },

    #[error("TOML parse error: {message}")]
    #[diagnostic(code(betterhook::config::toml))]
    Toml {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("{message}")]
        span: Option<SourceSpan>,
    },

    #[error("YAML parse error: {message}")]
    #[diagnostic(code(betterhook::config::yaml))]
    Yaml {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("{message}")]
        span: Option<SourceSpan>,
    },

    #[error("JSON parse error: {message}")]
    #[diagnostic(code(betterhook::config::json))]
    Json {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("{message}")]
        span: Option<SourceSpan>,
    },

    #[error("KDL parse error: {message}")]
    #[diagnostic(code(betterhook::config::kdl))]
    Kdl {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("{message}")]
        span: Option<SourceSpan>,
    },

    #[error("invalid config: {message}")]
    #[diagnostic(code(betterhook::config::invalid))]
    Invalid { message: String },

    #[error("invalid duration '{input}' in job '{job}': {source}")]
    #[diagnostic(
        code(betterhook::config::duration),
        help("use humantime durations like '30s', '1m30s', '500ms'")
    )]
    Duration {
        job: String,
        input: String,
        #[source]
        source: humantime::DurationError,
    },
}

pub type ConfigResult<T> = Result<T, ConfigError>;
