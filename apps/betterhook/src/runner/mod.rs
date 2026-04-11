//! Tokio-based job runner. Streams subprocess stdout/stderr line by
//! line through a central multiplexer so output is always live and
//! memory usage stays constant regardless of how chatty the job is —
//! the direct fix for lefthook's "buffered then dumped on completion"
//! behavior.

pub mod executor;
pub mod output;
pub mod proc;

use std::path::PathBuf;

use miette::Diagnostic;
use thiserror::Error;

pub use executor::{ExecutionReport, RunOptions, run_hook, run_hook_with_options};
pub use output::{OutputEvent, Stream};
pub use proc::{Cancel, EXIT_CANCELLED, EXIT_TIMEOUT};

use crate::error::ConfigError;
use crate::git::GitError;

#[derive(Debug, Error, Diagnostic)]
pub enum RunError {
    #[error("git error")]
    #[diagnostic(transparent)]
    Git(#[from] GitError),

    #[error("config error")]
    #[diagnostic(transparent)]
    Config(#[from] ConfigError),

    #[error("failed to build glob pattern")]
    #[diagnostic(code(betterhook::runner::glob))]
    Glob(#[from] globset::Error),

    #[error("failed to spawn `{cmd}`")]
    #[diagnostic(code(betterhook::runner::spawn))]
    Spawn {
        cmd: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed waiting for `{cmd}` (pid {pid:?})")]
    #[diagnostic(code(betterhook::runner::wait))]
    Wait {
        cmd: String,
        pid: Option<u32>,
        #[source]
        source: std::io::Error,
    },

    #[error("io error at {path}")]
    #[diagnostic(code(betterhook::runner::io))]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type RunResult<T> = Result<T, RunError>;
