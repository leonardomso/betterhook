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

/// Aggregate snapshot returned by [`Store::stats`].
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub entries: usize,
    pub total_bytes: u64,
    #[serde(with = "systemtime_opt_secs")]
    pub oldest: Option<SystemTime>,
    #[serde(with = "systemtime_opt_secs")]
    pub newest: Option<SystemTime>,
}

#[allow(clippy::ref_option)]
mod systemtime_opt_secs {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => {
                let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                Some(secs).serialize(s)
            }
            None => Option::<u64>::None.serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
        let opt = Option::<u64>::deserialize(d)?;
        Ok(opt.map(|secs| UNIX_EPOCH + Duration::from_secs(secs)))
    }
}

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
    /// `cache stats`, `cache verify`, and freshness checks.
    #[serde(with = "systemtime_as_secs")]
    pub created_at: SystemTime,
    /// Per-input-file mtime snapshot taken when the entry was written.
    /// Used to reject a cache hit whose input files have changed since
    /// the result was captured.
    /// `#[serde(default)]` keeps older entries readable.
    #[serde(default)]
    pub inputs: Vec<CachedInput>,
}

/// One input file recorded alongside a cache entry. The runner compares
/// the current mtime against `modified_at` and treats any divergence as
/// a cache miss.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedInput {
    pub path: PathBuf,
    #[serde(with = "systemtime_opt_secs")]
    pub modified_at: Option<SystemTime>,
}

mod systemtime_as_secs {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
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
    #[must_use = "cache lookups are pure queries; ignoring the result is a bug"]
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
    #[must_use = "returns whether the entry existed"]
    pub fn remove(&self, key: &CacheKey) -> StoreResult<bool> {
        let path = self.entry_path(key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    /// True when the store has no entries on disk.
    #[must_use = "check the boolean to know whether the store is empty"]
    pub fn is_empty(&self) -> StoreResult<bool> {
        Ok(self.len()? == 0)
    }

    /// Walk every entry in the store's sharded tree, calling `f` on
    /// each `DirEntry`. The shard-walking pattern was previously
    /// duplicated across `len`, `stats`, `clear`, and `verify`; this
    /// helper is the single source of truth.
    fn for_each_entry<F>(&self, mut f: F) -> StoreResult<()>
    where
        F: FnMut(&std::fs::DirEntry) -> StoreResult<()>,
    {
        if !self.root.is_dir() {
            return Ok(());
        }
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
            let shard_path = shard.path();
            for entry in std::fs::read_dir(&shard_path).map_err(|source| StoreError::Io {
                path: shard_path.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| StoreError::Io {
                    path: shard_path.clone(),
                    source,
                })?;
                f(&entry)?;
            }
        }
        Ok(())
    }

    /// Return the total number of entries currently in the store.
    /// Walks the on-disk tree — O(entries). Fine for `cache stats`
    /// on reasonable cache sizes; a future phase can add a stats
    /// sidecar if this ever shows up in a profile.
    #[must_use = "discarding the count means wasted I/O"]
    pub fn len(&self) -> StoreResult<usize> {
        let mut count = 0usize;
        self.for_each_entry(|_| {
            count += 1;
            Ok(())
        })?;
        Ok(count)
    }

    /// Absolute root directory for this store.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Aggregate the store into a [`Stats`] snapshot by walking every
    /// shard directory and tallying entry count, total bytes, and the
    /// oldest/newest `modified` timestamp.
    #[must_use = "discarding stats means wasted I/O"]
    pub fn stats(&self) -> StoreResult<Stats> {
        let mut stats = Stats::default();
        self.for_each_entry(|entry| {
            let meta = entry.metadata().map_err(|source| StoreError::Io {
                path: entry.path(),
                source,
            })?;
            stats.entries += 1;
            stats.total_bytes += meta.len();
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            stats.oldest = Some(match stats.oldest {
                Some(t) if t < modified => t,
                _ => modified,
            });
            stats.newest = Some(match stats.newest {
                Some(t) if t > modified => t,
                _ => modified,
            });
            Ok(())
        })?;
        Ok(stats)
    }

    /// Remove every cache entry. Returns the number of files deleted.
    #[must_use = "returns the number of entries removed"]
    pub fn clear(&self) -> StoreResult<usize> {
        let mut removed = 0usize;
        self.for_each_entry(|entry| {
            std::fs::remove_file(entry.path()).map_err(|source| StoreError::Io {
                path: entry.path(),
                source,
            })?;
            removed += 1;
            Ok(())
        })?;
        Ok(removed)
    }

    /// Walk the store and return any entry whose JSON no longer
    /// deserializes cleanly. Caller-initiated `cache clear` is the
    /// remediation; `verify` itself doesn't repair anything.
    #[must_use = "inspect the list of corrupt entries"]
    pub fn verify(&self) -> StoreResult<Vec<PathBuf>> {
        let mut corrupt = Vec::new();
        self.for_each_entry(|entry| {
            let path = entry.path();
            match std::fs::read(&path) {
                Ok(bytes) => {
                    if serde_json::from_slice::<CachedResult>(&bytes).is_err() {
                        corrupt.push(path);
                    }
                }
                Err(_) => corrupt.push(path),
            }
            Ok(())
        })?;
        Ok(corrupt)
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
            inputs: Vec::new(),
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
            inputs: Vec::new(),
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
                    inputs: Vec::new(),
                },
            )
            .unwrap();
        assert!(store.remove(&key).unwrap());
        assert!(!store.remove(&key).unwrap());
    }
}
