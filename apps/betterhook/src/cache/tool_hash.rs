//! Resolve a job's tool binary and hash it for the CA cache key.
//!
//! The resolver:
//!
//! 1. Parses the first whitespace-separated token of the run command
//!    as the tool invocation (e.g. `eslint --cache ...` → `eslint`).
//! 2. Resolves it on `PATH` via `which::which`.
//! 3. If the resolved path lives under `$MISE_SHIMS_DIR`, asks
//!    `mise which <cmd>` for the concrete target binary.
//! 4. Canonicalizes the result so symlinks (nvm, asdf) resolve to the
//!    underlying file.
//! 5. Hashes the resulting file via blake3 mmap.
//!
//! Failures at any step fall back to the run-string hash so the cache
//! key stays stable even when binary resolution is unavailable.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use super::hash::{ToolHash, hash_bytes, hash_file};

/// Per-process memoization cache for resolved tool hashes.
///
/// `resolve_tool_hash` is expensive: it calls `which`, maybe spawns
/// `mise which`, canonicalizes the result, and mmap-hashes the binary.
/// That adds 3-5 ms per call. In a single hook run, most jobs share a
/// handful of tools (`cargo`, `eslint`, `prettier`) — memoizing by
/// tool name saves that cost on every call after the first.
///
/// The cache lives for the duration of the process. A tool upgrade
/// mid-run is handled by the fact that `betterhook` is a short-lived
/// CLI — a new process gets a fresh cache. The daemon rebuilds its
/// cache on restart.
fn resolved_cache() -> &'static Mutex<HashMap<String, ToolHash>> {
    static CACHE: OnceLock<Mutex<HashMap<String, ToolHash>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve the tool binary for a run command and return a blake3-keyed
/// `ToolHash` of its bytes.
#[must_use]
pub fn resolve_tool_hash(run_cmd: &str) -> ToolHash {
    let Some(tool) = first_token(run_cmd) else {
        return fallback(run_cmd);
    };

    // Per-process memoization: check the cache before paying the
    // `which` + mmap cost.
    if let Some(hit) = resolved_cache()
        .lock()
        .ok()
        .and_then(|m| m.get(tool).cloned())
    {
        return hit;
    }

    let hash = compute_tool_hash(tool).unwrap_or_else(|| fallback(run_cmd));

    if let Ok(mut m) = resolved_cache().lock() {
        m.insert(tool.to_owned(), hash.clone());
    }
    hash
}

fn compute_tool_hash(tool: &str) -> Option<ToolHash> {
    let resolved = resolve_on_path(tool)?;
    let concrete = follow_mise_shim(&resolved, tool).unwrap_or(resolved);
    let canonical = std::fs::canonicalize(&concrete).unwrap_or(concrete);
    hash_file(&canonical).ok().map(|h| ToolHash(h.0))
}

fn fallback(run_cmd: &str) -> ToolHash {
    ToolHash(hash_bytes(run_cmd.as_bytes()))
}

/// Parse the first shell-token-ish thing out of a run string. We don't
/// try to be a full shell parser — if the user wrote
/// `cargo clippy -- -D warnings`, we take `cargo`.
fn first_token(run: &str) -> Option<&str> {
    run.split_whitespace().next()
}

fn resolve_on_path(cmd: &str) -> Option<PathBuf> {
    // Skip the resolution if the command already contains a path
    // separator (absolute or relative path).
    if cmd.contains('/') {
        return Some(PathBuf::from(cmd));
    }
    which::which(cmd).ok()
}

/// If `candidate` lives under `$MISE_SHIMS_DIR`, ask mise for the
/// real target binary. Returns `None` when not a mise shim or when
/// mise is unavailable.
fn follow_mise_shim(candidate: &Path, tool: &str) -> Option<PathBuf> {
    let shims_dir = std::env::var_os("MISE_SHIMS_DIR")?;
    let shims_path = PathBuf::from(shims_dir);
    if !candidate.starts_with(&shims_path) {
        return None;
    }
    let output = Command::new("mise").args(["which", tool]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

/// Convenience wrapper for callers that want to skip the fallback
/// (e.g. tests that want to know whether resolution actually worked).
pub fn try_resolve_tool_hash(run_cmd: &str) -> io::Result<ToolHash> {
    let tool = first_token(run_cmd)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "run command is empty"))?;
    let resolved = resolve_on_path(tool)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no '{tool}' on PATH")))?;
    let concrete = follow_mise_shim(&resolved, tool).unwrap_or(resolved);
    let canonical = std::fs::canonicalize(&concrete).unwrap_or(concrete);
    let h = hash_file(&canonical)?;
    Ok(ToolHash(h.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_token_extraction() {
        assert_eq!(first_token("eslint --cache a.ts"), Some("eslint"));
        assert_eq!(first_token("cargo clippy -- -D warnings"), Some("cargo"));
        assert_eq!(first_token(""), None);
    }

    #[test]
    fn absolute_path_commands_bypass_which() {
        let resolved = resolve_on_path("/bin/sh");
        assert_eq!(resolved, Some(PathBuf::from("/bin/sh")));
    }

    #[test]
    fn resolve_tool_hash_of_existing_binary_is_stable() {
        // `/bin/sh` exists on every Unix we target.
        let a = resolve_tool_hash("/bin/sh -c 'echo hi'");
        let b = resolve_tool_hash("/bin/sh -c 'echo hi'");
        assert_eq!(a, b);
    }

    #[test]
    fn resolve_tool_hash_falls_back_on_missing_binary() {
        // A clearly non-existent command should still produce a hash
        // (the fallback path), never panic.
        let h = resolve_tool_hash("this-command-definitely-does-not-exist-12345 arg");
        assert_eq!(h.0.len(), 64, "falls back to 64-char hex");
    }

    #[test]
    fn resolve_tool_hash_is_memoized_by_tool_name() {
        // The second call for the same tool should return an identical
        // hash without re-running which/mmap — we can't directly
        // observe the call count, but stability plus a per-process
        // cache means repeating the call is effectively free.
        let a = resolve_tool_hash("/bin/sh -c one");
        let b = resolve_tool_hash("/bin/sh -c two");
        // Both should resolve to the same tool hash because the tool
        // (`/bin/sh`) is identical. args_hash_from_job captures the
        // "one" vs "two" distinction elsewhere in the cache key.
        assert_eq!(a, b);
    }
}
