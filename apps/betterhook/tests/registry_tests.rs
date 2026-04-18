//! Tests for `betterhook::daemon::registry` — the in-memory lock registry.
//!
//! Pure async tests with no external dependencies.

use std::sync::Arc;

use betterhook::daemon::registry::Registry;
use betterhook::lock::protocol::{LockKey, Scope};

fn tool_key(name: &str, permits: u32) -> LockKey {
    LockKey {
        scope: Scope::Tool,
        name: name.to_owned(),
        permits,
    }
}

// ---------------------------------------------------------------------------
// semaphore creation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_acquire_creates_semaphore() {
    let reg = Registry::new();
    let key = tool_key("eslint", 3);
    let sem = reg.semaphore(&key).await.unwrap();
    assert_eq!(sem.available_permits(), 3);
}

#[tokio::test]
async fn second_acquire_returns_same_semaphore() {
    let reg = Registry::new();
    let key = tool_key("eslint", 2);
    let sem1 = reg.semaphore(&key).await.unwrap();
    let sem2 = reg.semaphore(&key).await.unwrap();
    assert!(Arc::ptr_eq(&sem1, &sem2));
}

#[tokio::test]
async fn zero_permits_is_rejected() {
    let reg = Registry::new();
    let key = tool_key("eslint", 0);
    let err = reg.semaphore(&key).await.unwrap_err();
    assert!(err.contains("permits must be > 0"), "got: {err}");
}

// ---------------------------------------------------------------------------
// mutual exclusion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mutex_semaphore_is_exclusive() {
    let reg = Registry::new();
    let key = tool_key("cargo", 1);
    let sem = reg.semaphore(&key).await.unwrap();
    let _permit = sem.clone().acquire_owned().await.unwrap();
    assert_eq!(sem.available_permits(), 0);
    let try_result = sem.clone().try_acquire_owned();
    assert!(
        try_result.is_err(),
        "should not be able to acquire when full"
    );
}

#[tokio::test]
async fn semaphore_allows_n_concurrent() {
    let reg = Registry::new();
    let key = tool_key("eslint", 4);
    let sem = reg.semaphore(&key).await.unwrap();
    let mut permits = Vec::new();
    for _ in 0..4 {
        permits.push(sem.clone().acquire_owned().await.unwrap());
    }
    assert_eq!(sem.available_permits(), 0);
    let try_result = sem.clone().try_acquire_owned();
    assert!(try_result.is_err(), "5th acquire should fail");
}

#[tokio::test]
async fn dropping_permit_frees_slot() {
    let reg = Registry::new();
    let key = tool_key("cargo", 1);
    let sem = reg.semaphore(&key).await.unwrap();
    let permit = sem.clone().acquire_owned().await.unwrap();
    assert_eq!(sem.available_permits(), 0);
    drop(permit);
    assert_eq!(sem.available_permits(), 1);
}

// ---------------------------------------------------------------------------
// snapshot
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snapshot_empty_registry() {
    let reg = Registry::new();
    let snap = reg.snapshot().await;
    assert!(snap.is_empty());
}

#[tokio::test]
async fn snapshot_reflects_active_permits() {
    let reg = Registry::new();
    let key = tool_key("eslint", 2);
    let sem = reg.semaphore(&key).await.unwrap();
    let _p1 = sem.clone().acquire_owned().await.unwrap();
    let snap = reg.snapshot().await;
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].active_permits, 1);
    assert_eq!(snap[0].key.name, "eslint");
}

#[tokio::test]
async fn snapshot_after_release_shows_zero_active() {
    let reg = Registry::new();
    let key = tool_key("eslint", 1);
    let sem = reg.semaphore(&key).await.unwrap();
    let permit = sem.clone().acquire_owned().await.unwrap();
    drop(permit);
    let snap = reg.snapshot().await;
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].active_permits, 0);
}

#[tokio::test]
async fn snapshot_multiple_keys() {
    let reg = Registry::new();
    let _s1 = reg.semaphore(&tool_key("eslint", 1)).await.unwrap();
    let _s2 = reg.semaphore(&tool_key("prettier", 2)).await.unwrap();
    let snap = reg.snapshot().await;
    assert_eq!(snap.len(), 2);
}

// ---------------------------------------------------------------------------
// concurrent access
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_semaphore_creation_returns_same_arc() {
    let reg = Registry::new();
    let key = tool_key("eslint", 4);
    let mut handles = Vec::new();
    for _ in 0..10 {
        let r = reg.clone();
        let k = key.clone();
        handles.push(tokio::spawn(async move { r.semaphore(&k).await.unwrap() }));
    }
    let mut sems = Vec::new();
    for h in handles {
        sems.push(h.await.unwrap());
    }
    for s in &sems[1..] {
        assert!(
            Arc::ptr_eq(&sems[0], s),
            "all tasks should get the same semaphore"
        );
    }
}
