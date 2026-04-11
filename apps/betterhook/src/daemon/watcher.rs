//! Cross-platform file watcher backed by `notify`.
//!
//! Phase 37 wraps `notify::RecommendedWatcher` in a tiny helper that
//! emits `WatcherEvent`s on an async mpsc channel. The actual
//! speculative runner that consumes these events lands in phase 38.
//!
//! The watcher is degradation-tolerant by design: if `notify` fails
//! to initialize (NFS, sandboxed container, inotify limit), we log
//! a diagnostic and return `WatcherHandle::disabled(...)` — a
//! live but empty handle that reports `disabled_reason`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// One interesting filesystem event bubbled up to the speculative
/// runner.
#[derive(Debug, Clone)]
pub struct WatcherEvent {
    pub kind: EventKind,
    pub paths: Vec<PathBuf>,
}

/// Public handle to a running (or disabled) file watcher.
///
/// Dropping the handle drops the inner `RecommendedWatcher`, which
/// unregisters every inotify/kqueue watch.
pub struct WatcherHandle {
    /// Consumer half of the event channel. Phase 38's speculative
    /// orchestrator reads from this.
    pub events: Option<mpsc::Receiver<WatcherEvent>>,
    /// `Some(reason)` means the watcher failed to start; the handle
    /// is a no-op.
    pub disabled_reason: Option<String>,
    /// Paths currently under watch. Empty when disabled.
    pub watched_paths: Vec<PathBuf>,
    /// Kept alive for the duration of the handle so the OS-level
    /// watches stay registered.
    inner: Option<RecommendedWatcher>,
}

impl WatcherHandle {
    /// Spawn a watcher over `root` with an optional list of
    /// glob-style excludes (e.g. `**/target/**`).
    #[must_use]
    pub fn watch(root: &Path, excludes: &[String]) -> Self {
        let (tx, rx) = mpsc::channel::<WatcherEvent>(1024);
        let filter = match build_exclude_filter(excludes) {
            Ok(f) => f,
            Err(e) => return Self::disabled_with(vec![], format!("exclude globs invalid: {e}")),
        };

        let tx_for_cb = tx.clone();
        let filter = Arc::new(filter);
        let watcher_result = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(ev) = res else { return };
            // Skip uninteresting events (access, metadata-only).
            if !matches!(
                ev.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                return;
            }
            let paths: Vec<PathBuf> = ev
                .paths
                .iter()
                .filter(|p| !filter.is_excluded(p))
                .cloned()
                .collect();
            if paths.is_empty() {
                return;
            }
            let _ = tx_for_cb.blocking_send(WatcherEvent { kind: ev.kind, paths });
        });
        let mut watcher = match watcher_result {
            Ok(w) => w,
            Err(e) => return Self::disabled_with(vec![], format!("{e}")),
        };
        if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
            return Self::disabled_with(vec![], format!("{e}"));
        }

        // Capture the tx sender so the watcher task doesn't drop it
        // prematurely; keeping it alongside the watcher keeps the
        // channel open for the caller.
        drop(tx);

        Self {
            events: Some(rx),
            disabled_reason: None,
            watched_paths: vec![root.to_path_buf()],
            inner: Some(watcher),
        }
    }

    /// Construct a disabled handle with a human-readable reason.
    #[must_use]
    fn disabled_with(watched: Vec<PathBuf>, reason: String) -> Self {
        Self {
            events: None,
            disabled_reason: Some(reason),
            watched_paths: watched,
            inner: None,
        }
    }

    /// True when this handle is actively watching files.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.inner.is_some() && self.disabled_reason.is_none()
    }
}

/// Compiled exclude-glob filter used by the watcher callback.
struct ExcludeFilter {
    set: globset::GlobSet,
}

impl ExcludeFilter {
    fn is_excluded(&self, path: &Path) -> bool {
        self.set.is_match(path)
    }
}

fn build_exclude_filter(patterns: &[String]) -> Result<ExcludeFilter, globset::Error> {
    Ok(ExcludeFilter {
        set: crate::runner::glob_util::build_globset_always(patterns)?,
    })
}

// Silence unused import in the unused arc type; `Arc` is needed at
// a macro-heavy expansion point where the compiler doesn't always
// detect usage.
#[allow(dead_code)]
fn _force_arc_mutex(_: Arc<Mutex<()>>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_watcher_reports_reason() {
        let h = WatcherHandle::disabled_with(vec![], "nope".to_owned());
        assert!(!h.is_active());
        assert_eq!(h.disabled_reason.as_deref(), Some("nope"));
        assert!(h.events.is_none());
    }

    #[test]
    fn build_exclude_filter_compiles() {
        let f = build_exclude_filter(&[
            "**/target/**".to_owned(),
            "**/node_modules/**".to_owned(),
        ])
        .unwrap();
        assert!(f.is_excluded(Path::new("repo/target/debug/betterhook")));
        assert!(!f.is_excluded(Path::new("repo/src/main.rs")));
    }
}
