//! High-level cache lookup / store built on top of `hash` + `store`.
//!
//! Phase 29 wires the hashing primitives and the disk store into a
//! single entry point the runner will call in phase 30. The tool
//! binary hash is a placeholder here — phase 31 replaces it with a
//! mise/nvm-aware `which`-based lookup.

use std::io;
use std::path::{Path, PathBuf};

use crate::config::Job;

use super::hash::{ArgsHash, CacheKey, ContentHash, ToolHash, args_hash, hash_bytes, hash_file};
use super::store::{CachedResult, Store, StoreError, StoreResult};
use super::tool_hash::resolve_tool_hash;

/// Combined blake3 hash of every file in `files`, sorted by path.
///
/// Missing files are represented with a stable sentinel so a deleted
/// file doesn't collide with a present one of the same path.
pub fn hash_file_set(files: &[PathBuf]) -> io::Result<ContentHash> {
    let mut paths: Vec<&PathBuf> = files.iter().collect();
    paths.sort();
    let mut hasher = blake3::Hasher::new();
    for path in paths {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        if path.is_file() {
            hasher.update(hash_file(path)?.0.as_bytes());
        } else {
            hasher.update(b"<missing>");
        }
        hasher.update(&[0]);
    }
    Ok(ContentHash(hasher.finalize().to_hex().to_string()))
}

/// Phase 29's placeholder tool-binary hash, kept as a fallback for
/// environments where `which`-based resolution doesn't work (CI
/// sandboxes, containers with stripped PATH, etc.). Hashes the run
/// string directly.
#[must_use]
pub fn tool_hash_proxy(run: &str) -> ToolHash {
    ToolHash(hash_bytes(run.as_bytes()))
}

/// Derive an `ArgsHash` that captures the job's command, fix variant,
/// extra env, and the glob/exclude patterns. Anything that affects
/// what the subprocess actually does must feed into this hash.
#[must_use]
pub fn args_hash_from_job(job: &Job) -> ArgsHash {
    let mut components: Vec<String> = Vec::with_capacity(4 + job.env.len());
    components.push(format!("run:{}", job.run));
    if let Some(fix) = &job.fix {
        components.push(format!("fix:{fix}"));
    }
    for g in &job.glob {
        components.push(format!("glob:{g}"));
    }
    for e in &job.exclude {
        components.push(format!("exclude:{e}"));
    }
    for (k, v) in &job.env {
        components.push(format!("env:{k}={v}"));
    }
    args_hash(&components)
}

/// Derive the full `CacheKey` for a `(job, files)` pair. Uses the
/// phase-31 real tool resolver (mise/nvm-aware) and falls back to
/// hashing the run string if binary resolution fails.
pub fn derive_key(job: &Job, files: &[PathBuf]) -> io::Result<CacheKey> {
    Ok(CacheKey {
        content: hash_file_set(files)?,
        tool: resolve_tool_hash(&job.run),
        args: args_hash_from_job(job),
    })
}

/// Query the cache for a prior run of `job` against `files`. Returns
/// the cached result on hit, `None` on miss, or a `StoreError` on
/// I/O / decode failure.
pub fn lookup(
    common_dir: &Path,
    job: &Job,
    files: &[PathBuf],
) -> StoreResult<Option<CachedResult>> {
    let key = derive_key(job, files).map_err(|source| StoreError::Io {
        path: common_dir.to_path_buf(),
        source,
    })?;
    Store::new(common_dir).get(&key)
}

/// Store a result for `(job, files)` in the cache. Best-effort —
/// callers log-and-continue on error; cache writes should never fail
/// a hook run.
pub fn store(
    common_dir: &Path,
    job: &Job,
    files: &[PathBuf],
    result: &CachedResult,
) -> StoreResult<()> {
    let key = derive_key(job, files).map_err(|source| StoreError::Io {
        path: common_dir.to_path_buf(),
        source,
    })?;
    Store::new(common_dir).put(&key, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::store::CachedResult;
    use crate::runner::OutputEvent;
    use std::collections::BTreeMap;
    use std::time::SystemTime;
    use tempfile::TempDir;

    fn job_with(run: &str, env: &[(&str, &str)]) -> Job {
        let mut env_map = BTreeMap::new();
        for (k, v) in env {
            env_map.insert((*k).to_owned(), (*v).to_owned());
        }
        Job {
            name: "lint".to_owned(),
            run: run.to_owned(),
            fix: None,
            glob: Vec::new(),
            exclude: Vec::new(),
            tags: Vec::new(),
            skip: None,
            only: None,
            env: env_map,
            root: None,
            stage_fixed: false,
            isolate: None,
            timeout: None,
            interactive: false,
            fail_text: None,
            priority: 0,
            reads: Vec::new(),
            writes: Vec::new(),
            network: false,
            concurrent_safe: false,
        }
    }

    #[test]
    fn args_hash_depends_on_env() {
        let a = args_hash_from_job(&job_with("eslint", &[]));
        let b = args_hash_from_job(&job_with("eslint", &[("CI", "1")]));
        assert_ne!(a, b);
    }

    #[test]
    fn tool_hash_proxy_depends_on_run() {
        assert_ne!(
            tool_hash_proxy("eslint --fix"),
            tool_hash_proxy("eslint --cache")
        );
    }

    #[test]
    fn hash_file_set_stable_across_sort_order() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.ts");
        let b = dir.path().join("b.ts");
        std::fs::write(&a, b"alpha").unwrap();
        std::fs::write(&b, b"beta").unwrap();

        let h1 = hash_file_set(&[a.clone(), b.clone()]).unwrap();
        let h2 = hash_file_set(&[b, a]).unwrap();
        assert_eq!(h1, h2, "input order should not affect the content hash");
    }

    #[test]
    fn hash_file_set_picks_up_content_change() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.ts");
        std::fs::write(&a, b"alpha").unwrap();
        let h1 = hash_file_set(std::slice::from_ref(&a)).unwrap();
        std::fs::write(&a, b"beta").unwrap();
        let h2 = hash_file_set(std::slice::from_ref(&a)).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn missing_file_has_stable_sentinel() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("never-existed.ts");
        let h1 = hash_file_set(std::slice::from_ref(&missing)).unwrap();
        let h2 = hash_file_set(std::slice::from_ref(&missing)).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn lookup_and_store_round_trip() {
        let common = TempDir::new().unwrap();
        let file_dir = TempDir::new().unwrap();
        let a = file_dir.path().join("a.ts");
        std::fs::write(&a, b"alpha").unwrap();

        let job = job_with("eslint --cache {files}", &[]);
        let files = vec![a.clone()];

        assert!(lookup(common.path(), &job, &files).unwrap().is_none());

        let result = CachedResult {
            exit: 0,
            events: vec![OutputEvent::Line {
                job: "lint".to_owned(),
                stream: crate::runner::Stream::Stdout,
                line: "a.ts: ok".to_owned(),
            }],
            created_at: SystemTime::now(),
        };
        store(common.path(), &job, &files, &result).unwrap();

        let cached = lookup(common.path(), &job, &files).unwrap();
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().exit, 0);

        // Modifying the file invalidates the lookup.
        std::fs::write(&a, b"beta").unwrap();
        assert!(lookup(common.path(), &job, &files).unwrap().is_none());
    }
}
