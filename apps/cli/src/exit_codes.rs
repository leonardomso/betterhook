//! Documented exit-code contract. Agents parse these. Do not change
//! meanings across releases — only add new codes.
#![allow(dead_code)]

/// Every job exited cleanly.
pub const OK: i32 = 0;
/// At least one job returned a non-zero exit code.
pub const HOOK_FAILED: i32 = 1;
/// Config parse or schema error.
pub const CONFIG_ERROR: i32 = 2;
/// Coordinator lock acquisition timed out before the requested work could start.
pub const LOCK_TIMEOUT: i32 = 3;
/// An unexpected git invocation failed (e.g. stash pop conflict).
pub const GIT_ERROR: i32 = 4;
/// Install/uninstall error.
pub const INSTALL_ERROR: i32 = 5;
/// Usage error from clap. Matches the sysexits.h convention.
pub const USAGE_ERROR: i32 = 64;
/// A per-job timeout expired. Matches GNU `timeout(1)`.
pub const JOB_TIMEOUT: i32 = 124;
/// SIGINT received.
pub const INTERRUPTED: i32 = 130;
