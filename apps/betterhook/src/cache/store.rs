//! On-disk cache store.
//!
//! Layout:
//!
//! ```text
//! <common-dir>/betterhook/cache/
//!   ab/                                # sharding prefix (first 2 tool-hex)
//!     cdef...__<content>__<args>.json  # one entry, atomic write
//! ```
//!
//! Entries are JSON-encoded `CachedResult`s. Writes go through a
//! `NamedTempFile::persist` so concurrent writers never see partial
//! files; reads are a plain `read` + `serde_json::from_slice`.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::runner::OutputEvent;

use super::hash::CacheKey;

/// Cache subdirectory under `<common-dir>/betterhook/`.
pub const CACHE_SUBDIR: &str = "cache";

/// Return the absolute cache directory for a given common-dir.
#[must_use]
pub fn cache_dir(common_dir: &Path) -> PathBuf {
    common_dir.join("betterhook").join(CACHE_SUBDIR)
}

/// A cached result for a single job run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResult {
    /// The exit code the job's last command returned.
    pub exit: i32,
    /// The sequence of `OutputEvent`s we captured. Replayed through
    /// the multiplexer on cache hit so the user sees the same output
    /// as if the job had just run.
    pub events: Vec<OutputEvent>,
    /// Wall-clock time when the entry was written. Used by
    /// `cache stats` and `cache verify` plus phase 39's freshness check.
    #[serde(with = "systemtime_as_secs")]
    pub created_at: SystemTime,
}

mod systemtime_as_secs {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        let secs = t
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        secs.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(UNIX_EPOCH + Duration::from_secs(secs))
    }
}

/// Handle to a cache store anchored at a `common-dir`.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// Errors returned by the cache store.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum StoreError {
    #[error("io error at {path}")]
    #[diagnostic(code(betterhook::cache::io))]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("cache json serialize/deserialize failed at {path}")]
    #[diagnostic(code(betterhook::cache::json))]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

pub type StoreResult<T> = Result<T, StoreError>;

impl Store {
    /// Open (or lazily create) the store for a common-dir.
    #[must_use]
    pub fn new(common_dir: &Path) -> Self {
        Self {
            root: cache_dir(common_dir),
        }
    }

    /// Absolute path for an entry's JSON file.
    #[must_use]
    pub fn entry_path(&self, key: &CacheKey) -> PathBuf {
        self.root.join(key.relative_path())
    }

    /// Fetch a cached result, if one exists.
    pub fn get(&self, key: &CacheKey) -> StoreResult<Option<CachedResult>> {
        let path = self.entry_path(key);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let result: CachedResult =
                    serde_json::from_slice(&bytes).map_err(|source| StoreError::Json {
                        path: path.clone(),
                        source,
                    })?;
                Ok(Some(result))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    /// Store a result atomically via a tempfile + persist.
    pub fn put(&self, key: &CacheKey, result: &CachedResult) -> StoreResult<()> {
        let path = self.entry_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| StoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let bytes = serde_json::to_vec(result).map_err(|source| StoreError::Json {
            path: path.clone(),
            source,
        })?;
        let parent = path.parent().unwrap_or(Path::new("."));
        let tmp = NamedTempFile::new_in(parent).map_err(|source| StoreError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        std::fs::write(tmp.path(), &bytes).map_err(|source| StoreError::Io {
            path: tmp.path().to_path_buf(),
            source,
        })?;
        tmp.persist(&path).map_err(|e| StoreError::Io {
            path: path.clone(),
            source: e.error,
        })?;
        Ok(())
    }

    /// Remove an entry if it exists.
    pub fn remove(&self, key: &CacheKey) -> StoreResult<bool> {
        let path = self.entry_path(key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    /// True when the store has no entries on disk.
    pub fn is_empty(&self) -> StoreResult<bool> {
        Ok(self.len()? == 0)
    }

    /// Return the total number of entries currently in the store.
    /// Walks the on-disk tree — O(entries). Fine for `cache stats`
    /// on reasonable cache sizes; a future phase can add a stats
    /// sidecar if this ever shows up in a profile.
    pub fn len(&self) -> StoreResult<usize> {
        if !self.root.is_dir() {
            return Ok(0);
        }
        let mut count = 0usize;
        for shard in std::fs::read_dir(&self.root).map_err(|source| StoreError::Io {
            path: self.root.clone(),
            source,
        })? {
            let shard = shard.map_err(|source| StoreError::Io {
                path: self.root.clone(),
                source,
            })?;
            if !shard.file_type().ok().is_some_and(|t| t.is_dir()) {
                continue;
            }
            for entry in std::fs::read_dir(shard.path()).map_err(|source| StoreError::Io {
                path: shard.path(),
                source,
            })? {
                if entry.is_ok() {
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::hash::{ArgsHash, ContentHash, ToolHash};
    use crate::runner::Stream;
    use std::time::Duration;
    use tempfile::TempDir;

    fn fake_key() -> CacheKey {
        CacheKey {
            content: ContentHash("c".repeat(64)),
            tool: ToolHash("ab".to_owned() + &"e".repeat(62)),
            args: ArgsHash("0".repeat(64)),
        }
    }

    #[test]
    fn put_get_round_trip() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        let key = fake_key();

        assert!(store.get(&key).unwrap().is_none());

        let result = CachedResult {
            exit: 0,
            events: vec![
                OutputEvent::JobStarted {
                    job: "lint".to_owned(),
                    cmd: "eslint a.ts".to_owned(),
                },
                OutputEvent::Line {
                    job: "lint".to_owned(),
                    stream: Stream::Stdout,
                    line: "a.ts: clean".to_owned(),
                },
                OutputEvent::JobFinished {
                    job: "lint".to_owned(),
                    exit: 0,
                    duration: Duration::from_millis(312),
                },
            ],
            created_at: SystemTime::now(),
        };

        store.put(&key, &result).unwrap();
        let back = store.get(&key).unwrap().expect("entry exists");
        assert_eq!(back.exit, 0);
        assert_eq!(back.events.len(), 3);
    }

    #[test]
    fn len_counts_entries_across_shards() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        assert_eq!(store.len().unwrap(), 0);

        let mut k = fake_key();
        let result = CachedResult {
            exit: 0,
            events: Vec::new(),
            created_at: SystemTime::now(),
        };
        store.put(&k, &result).unwrap();

        k.tool = ToolHash("cd".to_owned() + &"1".repeat(62));
        store.put(&k, &result).unwrap();

        assert_eq!(store.len().unwrap(), 2);
    }

    #[test]
    fn remove_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        let key = fake_key();
        assert!(!store.remove(&key).unwrap());

        store
            .put(
                &key,
                &CachedResult {
                    exit: 0,
                    events: Vec::new(),
                    created_at: SystemTime::now(),
                },
            )
            .unwrap();
        assert!(store.remove(&key).unwrap());
        assert!(!store.remove(&key).unwrap());
    }
}
