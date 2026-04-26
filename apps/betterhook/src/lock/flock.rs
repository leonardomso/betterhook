//! `fs4` advisory-lock backend for coarse mutual exclusion.
//!
//! It provides one lockfile per key, which matches simple mutex
//! semantics but cannot express sharded capacity or richer daemon-side
//! coordination.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

/// A held fs2/fs4 exclusive advisory lock. Drops release the lock.
#[derive(Debug)]
pub struct FileLock {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
}

impl FileLock {
    /// Acquire an exclusive flock on `<common-dir>/betterhook/locks/<key>.lock`.
    /// Creates the file if it doesn't exist. Blocks until the lock is free.
    pub fn acquire(common_dir: &Path, key: &str) -> std::io::Result<Self> {
        let dir = common_dir.join("betterhook").join("locks");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.lock", sanitize(key)));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        file.lock_exclusive()?;
        Ok(Self { file, path })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn acquire_and_release() {
        let dir = TempDir::new().unwrap();
        {
            let _lock = FileLock::acquire(dir.path(), "tool:eslint").unwrap();
        }
        // Re-acquire after release should work.
        let _lock = FileLock::acquire(dir.path(), "tool:eslint").unwrap();
    }

    #[test]
    fn sanitize_scrubs_separators() {
        assert_eq!(sanitize("tool:eslint"), "tool_eslint");
        assert_eq!(sanitize("cargo@/tmp/wt"), "cargo__tmp_wt");
    }
}
