#![allow(clippy::cast_sign_loss)]
//! Comprehensive tests for the content-addressable cache system.

use std::time::{Duration, SystemTime};

use betterhook::cache::{
    ArgsHash, CacheKey, CachedResult, ContentHash, Store, ToolHash, args_hash, derive_key,
    hash_bytes, hash_file, inputs_fresh, lookup_blocking, snapshot_inputs, store_result_blocking,
};
use betterhook::config::{IsolateSpec, Job};
use betterhook::runner::{OutputEvent, Stream};
use std::collections::BTreeMap;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn fake_key(shard: &str) -> CacheKey {
    assert!(shard.len() >= 2, "shard prefix must be at least 2 chars");
    CacheKey {
        content: ContentHash("c".repeat(64)),
        tool: ToolHash(shard[..2].to_owned() + &"e".repeat(62)),
        args: ArgsHash("0".repeat(64)),
    }
}

fn empty_result() -> CachedResult {
    CachedResult {
        exit: 0,
        events: Vec::new(),
        created_at: SystemTime::now(),
        inputs: Vec::new(),
    }
}

fn result_with_events(exit: i32, events: Vec<OutputEvent>) -> CachedResult {
    CachedResult {
        exit,
        events,
        created_at: SystemTime::now(),
        inputs: Vec::new(),
    }
}

fn mk_job(run: &str) -> Job {
    Job {
        name: "test-job".into(),
        run: run.to_owned(),
        fix: None,
        glob: Vec::new(),
        exclude: Vec::new(),
        tags: Vec::new(),
        skip: None,
        only: None,
        env: BTreeMap::new(),
        root: None,
        stage_fixed: false,
        isolate: None::<IsolateSpec>,
        timeout: None,
        interactive: false,
        fail_text: None,
        priority: 0,
        reads: Vec::new(),
        writes: Vec::new(),
        network: false,
        concurrent_safe: false,
        builtin: None,
    }
}

// ---------------------------------------------------------------------------
// Store tests
// ---------------------------------------------------------------------------

#[test]
fn store_put_get_round_trip() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path());
    let key = fake_key("ab");

    let events = vec![
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
    ];

    let result = result_with_events(0, events);
    store.put(&key, &result).unwrap();

    let back = store.get(&key).unwrap().expect("entry should exist");
    assert_eq!(back.exit, 0);
    assert_eq!(back.events.len(), 3);
}

#[test]
fn store_get_missing_returns_none() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path());
    let key = fake_key("ab");

    assert!(store.get(&key).unwrap().is_none());
}

#[test]
fn store_remove_existing() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path());
    let key = fake_key("ab");

    store.put(&key, &empty_result()).unwrap();
    assert!(store.remove(&key).unwrap());
    assert!(store.get(&key).unwrap().is_none());
}

#[test]
fn store_remove_nonexistent() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path());
    let key = fake_key("ab");

    assert!(!store.remove(&key).unwrap());
}

#[test]
fn store_len_counts_across_shards() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path());

    store.put(&fake_key("ab"), &empty_result()).unwrap();
    store.put(&fake_key("cd"), &empty_result()).unwrap();
    store.put(&fake_key("ef"), &empty_result()).unwrap();

    assert_eq!(store.len().unwrap(), 3);
}

#[test]
fn store_clear_removes_all() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path());

    store.put(&fake_key("ab"), &empty_result()).unwrap();
    store.put(&fake_key("cd"), &empty_result()).unwrap();
    store.put(&fake_key("ef"), &empty_result()).unwrap();

    let removed = store.clear().unwrap();
    assert_eq!(removed, 3);
    assert_eq!(store.len().unwrap(), 0);
}

#[test]
fn store_stats_tallies_bytes() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path());

    store.put(&fake_key("ab"), &empty_result()).unwrap();

    let stats = store.stats().unwrap();
    assert_eq!(stats.entries, 1);
    assert!(stats.total_bytes > 0);
}

#[test]
fn store_verify_clean_store() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path());

    store.put(&fake_key("ab"), &empty_result()).unwrap();

    let corrupt = store.verify().unwrap();
    assert!(
        corrupt.is_empty(),
        "freshly written entries should verify clean"
    );
}

// ---------------------------------------------------------------------------
// Hash tests
// ---------------------------------------------------------------------------

#[test]
fn hash_bytes_deterministic() {
    let a = hash_bytes(b"betterhook");
    let b = hash_bytes(b"betterhook");
    assert_eq!(a, b);
}

#[test]
fn hash_bytes_distinct_inputs() {
    let a = hash_bytes(b"alpha");
    let b = hash_bytes(b"beta");
    assert_ne!(a, b);
}

#[test]
fn hash_file_matches_hash_bytes() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sample.txt");
    let data = b"content-addressable cache test";
    std::fs::write(&path, data).unwrap();

    let file_hash = hash_file(&path).unwrap();
    assert_eq!(file_hash.0, hash_bytes(data));
}

#[test]
fn args_hash_order_matters() {
    let a = args_hash(&["a".to_owned(), "b".to_owned()]);
    let b = args_hash(&["b".to_owned(), "a".to_owned()]);
    assert_ne!(a, b);
}

#[test]
fn args_hash_empty_is_stable() {
    let a = args_hash(&[]);
    let b = args_hash(&[]);
    assert_eq!(a, b);
}

#[test]
fn cache_key_relative_path_has_two_components() {
    let key = CacheKey {
        content: ContentHash("c".repeat(64)),
        tool: ToolHash("ab".to_owned() + &"f".repeat(62)),
        args: ArgsHash("a".repeat(64)),
    };
    let rel = key.relative_path();
    let components: Vec<_> = rel.components().collect();
    assert_eq!(
        components.len(),
        2,
        "relative path should be shard_dir/filename"
    );
}

// ---------------------------------------------------------------------------
// snapshot / freshness tests
// ---------------------------------------------------------------------------

#[test]
fn snapshot_inputs_captures_mtime() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.ts");
    std::fs::write(&path, b"hello").unwrap();

    let inputs = snapshot_inputs(&[path]);
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0].modified_at.is_some());
}

#[test]
fn snapshot_inputs_missing_file_none_mtime() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("does-not-exist.ts");

    let inputs = snapshot_inputs(&[missing]);
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0].modified_at.is_none());
}

#[test]
fn inputs_fresh_passes_for_unchanged() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.ts");
    std::fs::write(&path, b"content").unwrap();

    let inputs = snapshot_inputs(&[path]);
    assert!(inputs_fresh(&inputs));
}

#[test]
fn inputs_fresh_fails_after_delete() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.ts");
    std::fs::write(&path, b"content").unwrap();

    let inputs = snapshot_inputs(std::slice::from_ref(&path));
    std::fs::remove_file(&path).unwrap();
    assert!(!inputs_fresh(&inputs));
}

#[test]
fn inputs_fresh_empty_always_true() {
    assert!(inputs_fresh(&[]));
}

// ---------------------------------------------------------------------------
// derive_key tests
// ---------------------------------------------------------------------------

#[test]
fn derive_key_deterministic() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.ts");
    std::fs::write(&path, b"alpha").unwrap();

    let job = mk_job("eslint {files}");
    let files = vec![path];

    let k1 = derive_key(&job, &files).unwrap();
    let k2 = derive_key(&job, &files).unwrap();
    assert_eq!(k1, k2);
}

#[test]
fn derive_key_changes_on_content() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.ts");

    let job = mk_job("eslint {files}");
    let files = vec![path.clone()];

    std::fs::write(&path, b"version-1").unwrap();
    let k1 = derive_key(&job, &files).unwrap();

    std::fs::write(&path, b"version-2").unwrap();
    let k2 = derive_key(&job, &files).unwrap();

    assert_ne!(k1.content, k2.content);
}

// ---------------------------------------------------------------------------
// lookup / store round-trip tests
// ---------------------------------------------------------------------------

#[test]
fn lookup_blocking_miss_then_hit() {
    let common = TempDir::new().unwrap();
    let file_dir = TempDir::new().unwrap();
    let path = file_dir.path().join("a.ts");
    std::fs::write(&path, b"alpha").unwrap();

    let job = mk_job("eslint {files}");
    let files = vec![path];

    // miss
    assert!(
        lookup_blocking(common.path(), &job, &files)
            .unwrap()
            .is_none()
    );

    // store
    let result = CachedResult {
        exit: 0,
        events: vec![OutputEvent::Line {
            job: "test-job".to_owned(),
            stream: Stream::Stdout,
            line: "ok".to_owned(),
        }],
        created_at: SystemTime::now(),
        inputs: snapshot_inputs(&files),
    };
    store_result_blocking(common.path(), &job, &files, &result).unwrap();

    // hit
    let cached = lookup_blocking(common.path(), &job, &files).unwrap();
    assert!(cached.is_some());
    assert_eq!(cached.unwrap().exit, 0);
}

#[test]
fn lookup_blocking_rejects_stale_mtime() {
    let common = TempDir::new().unwrap();
    let file_dir = TempDir::new().unwrap();
    let path = file_dir.path().join("a.ts");
    std::fs::write(&path, b"alpha").unwrap();

    let job = mk_job("eslint {files}");
    let files = vec![path.clone()];

    let result = CachedResult {
        exit: 0,
        events: Vec::new(),
        created_at: SystemTime::now(),
        inputs: snapshot_inputs(&files),
    };
    store_result_blocking(common.path(), &job, &files, &result).unwrap();

    // touch the file without changing content — mtime moves
    let later = SystemTime::now() + Duration::from_secs(5);
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_modified(later).unwrap();
    drop(f);

    assert!(
        lookup_blocking(common.path(), &job, &files)
            .unwrap()
            .is_none(),
        "stale mtime should cause a cache miss"
    );
}

#[test]
fn concurrent_puts_dont_corrupt() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path());

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let store = store.clone();
            let key = CacheKey {
                content: ContentHash(format!("{i:064x}")),
                tool: ToolHash(format!("{i:02x}") + &"0".repeat(62)),
                args: ArgsHash("0".repeat(64)),
            };
            let result = result_with_events(
                i,
                vec![OutputEvent::Line {
                    job: format!("job-{i}"),
                    stream: Stream::Stdout,
                    line: format!("line-{i}"),
                }],
            );
            std::thread::spawn(move || {
                store.put(&key, &result).unwrap();
                let back = store.get(&key).unwrap().expect("round-trip");
                assert_eq!(back.exit, i);
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread should not panic");
    }

    assert_eq!(store.len().unwrap(), 10);
}
