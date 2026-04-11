//! betterhook — a memory-efficient, worktree-native git hooks manager built
//! for the AI agent era.
//!
//! This crate is the implementation library. The `betterhook` CLI binary lives
//! in the sibling `apps/cli` crate and depends on this one.

pub mod config;
pub mod error;
pub mod git;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
