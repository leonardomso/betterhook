//! Tests for `betterhook::daemon::watcher` — filesystem event delivery.
//!
//! These tests use real `notify` watchers on temporary directories.
//! Event timing varies by platform (inotify vs `FSEvents`), so all
//! assertions use generous timeouts.

use std::time::Duration;

use betterhook::daemon::watcher::WatcherHandle;
use notify::EventKind;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// active / disabled state
// ---------------------------------------------------------------------------

#[test]
fn watch_on_valid_dir_is_active() {
    let dir = TempDir::new().unwrap();
    let handle = WatcherHandle::watch(dir.path(), &[]);
    assert!(handle.is_active());
    assert!(handle.disabled_reason.is_none());
    assert_eq!(handle.watched_paths.len(), 1);
}

#[test]
fn invalid_exclude_glob_disables_watcher() {
    let dir = TempDir::new().unwrap();
    let handle = WatcherHandle::watch(dir.path(), &["[invalid".to_owned()]);
    assert!(!handle.is_active());
    assert!(handle.disabled_reason.is_some());
    let reason = handle.disabled_reason.as_deref().unwrap();
    assert!(reason.contains("exclude globs invalid"), "got: {reason}");
    assert!(handle.events.is_none());
}

// ---------------------------------------------------------------------------
// event delivery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_event_is_delivered() {
    let dir = TempDir::new().unwrap();
    let mut handle = WatcherHandle::watch(dir.path(), &[]);
    let rx = handle.events.as_mut().unwrap();

    std::fs::write(dir.path().join("new.txt"), "hello").unwrap();

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("should receive event within 2s")
        .expect("channel should not be closed");
    assert!(
        matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)),
        "expected Create or Modify, got {:?}",
        event.kind
    );
    assert!(!event.paths.is_empty());
}

#[tokio::test]
async fn modify_event_is_delivered() {
    let dir = TempDir::new().unwrap();
    let existing = dir.path().join("file.txt");
    std::fs::write(&existing, "original").unwrap();

    let mut handle = WatcherHandle::watch(dir.path(), &[]);
    let rx = handle.events.as_mut().unwrap();

    // Small delay to let the watcher settle after initial setup.
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::fs::write(&existing, "modified").unwrap();

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("should receive event within 2s")
        .expect("channel should not be closed");
    // macOS FSEvents may coalesce a modify into Create(File); the key
    // assertion is that we get *some* event for the changed file.
    assert!(
        matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)),
        "expected Create or Modify, got {:?}",
        event.kind
    );
}

#[tokio::test]
async fn remove_event_is_delivered() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("doomed.txt");
    std::fs::write(&target, "bye").unwrap();

    let mut handle = WatcherHandle::watch(dir.path(), &[]);
    let rx = handle.events.as_mut().unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    std::fs::remove_file(&target).unwrap();

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("should receive event within 2s")
        .expect("channel should not be closed");
    // macOS FSEvents may deliver Create/Modify instead of Remove; the
    // important thing is that a filesystem change is detected.
    assert!(
        matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ),
        "expected any filesystem event, got {:?}",
        event.kind
    );
}

#[tokio::test]
async fn exclude_filter_blocks_events() {
    let dir = TempDir::new().unwrap();
    let excluded_dir = dir.path().join("target");
    std::fs::create_dir_all(&excluded_dir).unwrap();

    // Let the filesystem settle after mkdir so the watcher does not
    // pick up the directory-creation event on macOS FSEvents.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut handle = WatcherHandle::watch(dir.path(), &["**/target/**".to_owned()]);
    let rx = handle.events.as_mut().unwrap();

    // Give the watcher time to register before writing.
    tokio::time::sleep(Duration::from_millis(200)).await;

    std::fs::write(excluded_dir.join("output.o"), "binary").unwrap();

    let result = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
    assert!(result.is_err(), "excluded paths should not produce events");
}

#[tokio::test]
async fn non_excluded_file_still_arrives_with_filter() {
    let dir = TempDir::new().unwrap();
    let excluded_dir = dir.path().join("target");
    std::fs::create_dir_all(&excluded_dir).unwrap();

    let mut handle = WatcherHandle::watch(dir.path(), &["**/target/**".to_owned()]);
    let rx = handle.events.as_mut().unwrap();

    std::fs::write(dir.path().join("src.rs"), "fn main() {}").unwrap();

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("non-excluded file should produce event")
        .expect("channel should not be closed");
    assert!(!event.paths.is_empty());
}

// ---------------------------------------------------------------------------
// drop behavior
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drop_closes_event_channel() {
    let dir = TempDir::new().unwrap();
    let mut handle = WatcherHandle::watch(dir.path(), &[]);
    let mut rx = handle.events.take().unwrap();
    drop(handle);

    // Drain any events from the watcher noticing its own drop / cleanup,
    // then the channel should close.
    let result = tokio::time::timeout(Duration::from_secs(1), async {
        while rx.recv().await.is_some() {}
    })
    .await;
    assert!(
        result.is_ok(),
        "channel should close when handle is dropped"
    );
}
