//! Coordinator lock, flock fallback, and wire protocol tests.

use std::path::{Path, PathBuf};

use betterhook::config::{IsolateSpec, ToolPathScope};
use betterhook::lock::protocol::{
    LockKey, LockStatus, LockToken, PROTOCOL_VERSION, Request, Response, Scope, decode_frame,
    encode_frame,
};
use betterhook::lock::{FileLock, acquire_job_lock, key_for_spec, lock_dir};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// FileLock (flock fallback) tests
// ---------------------------------------------------------------------------

#[test]
fn flock_acquire_and_release_cycle() {
    let dir = TempDir::new().unwrap();
    {
        let _lock = FileLock::acquire(dir.path(), "tool:eslint").unwrap();
    }
    let _lock = FileLock::acquire(dir.path(), "tool:eslint").unwrap();
}

#[test]
fn flock_creates_lock_file_on_disk() {
    let dir = TempDir::new().unwrap();
    let _lock = FileLock::acquire(dir.path(), "tool:prettier").unwrap();
    let lock_path = dir.path().join("betterhook/locks/tool_prettier.lock");
    assert!(lock_path.exists(), "lock file should be created on disk");
}

#[test]
fn flock_different_keys_coexist() {
    let dir = TempDir::new().unwrap();
    let _a = FileLock::acquire(dir.path(), "tool:eslint").unwrap();
    let _b = FileLock::acquire(dir.path(), "tool:prettier").unwrap();
}

#[test]
fn flock_sanitizes_special_chars() {
    let dir = TempDir::new().unwrap();
    let _lock = FileLock::acquire(dir.path(), "tool-path:cargo:/tmp/wt-a").unwrap();
    let lock_dir = dir.path().join("betterhook/locks");
    let entries: Vec<_> = std::fs::read_dir(&lock_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(
        !entries[0].contains('/') && !entries[0].contains(':'),
        "lock filename should be sanitized"
    );
}

// ---------------------------------------------------------------------------
// key_for_spec tests
// ---------------------------------------------------------------------------

#[test]
fn key_for_spec_tool_variant() {
    let (k, permits, env) = key_for_spec(
        &IsolateSpec::Tool {
            name: "eslint".to_owned(),
        },
        Path::new("/tmp/wt"),
    );
    assert_eq!(k, "tool:eslint");
    assert_eq!(permits, 1);
    assert!(env.is_empty());
}

#[test]
fn key_for_spec_sharded_variant() {
    let (k, permits, _) = key_for_spec(
        &IsolateSpec::Sharded {
            name: "tsc".to_owned(),
            slots: 4,
        },
        Path::new("/tmp/wt"),
    );
    assert_eq!(k, "sharded:tsc");
    assert_eq!(permits, 4);
}

#[test]
fn key_for_spec_sharded_single_slot() {
    let (_, permits, _) = key_for_spec(
        &IsolateSpec::Sharded {
            name: "x".to_owned(),
            slots: 1,
        },
        Path::new("/tmp/wt"),
    );
    assert_eq!(permits, 1);
}

#[test]
fn key_for_spec_tool_path_per_worktree_cargo() {
    let (_, _, env) = key_for_spec(
        &IsolateSpec::ToolPath {
            tool: "cargo".to_owned(),
            target_dir: ToolPathScope::PerWorktree,
        },
        Path::new("/tmp/wt-a"),
    );
    assert_eq!(
        env,
        vec![("CARGO_TARGET_DIR".to_owned(), "/tmp/wt-a/target".to_owned())]
    );
}

#[test]
fn key_for_spec_tool_path_per_worktree_non_cargo() {
    let (_, _, env) = key_for_spec(
        &IsolateSpec::ToolPath {
            tool: "unknown-tool".to_owned(),
            target_dir: ToolPathScope::PerWorktree,
        },
        Path::new("/tmp/wt"),
    );
    assert!(env.is_empty(), "non-cargo tools should not inject env vars");
}

#[test]
fn key_for_spec_tool_path_custom_path() {
    let (k, permits, env) = key_for_spec(
        &IsolateSpec::ToolPath {
            tool: "cargo".to_owned(),
            target_dir: ToolPathScope::Path(PathBuf::from("/shared/target")),
        },
        Path::new("/tmp/wt"),
    );
    assert!(k.contains("/shared/target"));
    assert_eq!(permits, 1);
    assert!(env.is_empty(), "custom path scope should not inject env");
}

#[test]
fn per_worktree_keys_differ_by_worktree() {
    let spec = IsolateSpec::ToolPath {
        tool: "cargo".to_owned(),
        target_dir: ToolPathScope::PerWorktree,
    };
    let (k1, _, _) = key_for_spec(&spec, Path::new("/tmp/wt-a"));
    let (k2, _, _) = key_for_spec(&spec, Path::new("/tmp/wt-b"));
    assert_ne!(k1, k2);
}

// ---------------------------------------------------------------------------
// acquire_job_lock tests
// ---------------------------------------------------------------------------

#[test]
fn acquire_job_lock_returns_guard() {
    let dir = TempDir::new().unwrap();
    let guard = acquire_job_lock(
        dir.path(),
        &IsolateSpec::Tool {
            name: "eslint".to_owned(),
        },
        Path::new("/tmp/wt"),
    )
    .unwrap();
    assert!(guard.extra_env.is_empty());
}

#[test]
fn acquire_job_lock_cargo_populates_extra_env() {
    let dir = TempDir::new().unwrap();
    let guard = acquire_job_lock(
        dir.path(),
        &IsolateSpec::ToolPath {
            tool: "cargo".to_owned(),
            target_dir: ToolPathScope::PerWorktree,
        },
        Path::new("/tmp/wt"),
    )
    .unwrap();
    assert_eq!(guard.extra_env.len(), 1);
    assert_eq!(guard.extra_env[0].0, "CARGO_TARGET_DIR");
}

// ---------------------------------------------------------------------------
// lock_dir tests
// ---------------------------------------------------------------------------

#[test]
fn lock_dir_is_under_betterhook() {
    let p = lock_dir(Path::new("/tmp/common"));
    assert_eq!(p, PathBuf::from("/tmp/common/betterhook/locks"));
}

// ---------------------------------------------------------------------------
// Wire protocol tests
// ---------------------------------------------------------------------------

#[test]
fn protocol_version_is_one() {
    assert_eq!(PROTOCOL_VERSION, 1);
}

#[test]
fn encode_decode_request_hello() {
    let req = Request::Hello { client_version: 1 };
    let frame = encode_frame(&req).unwrap();
    let decoded: Request = decode_frame(&frame[4..]).unwrap();
    match decoded {
        Request::Hello { client_version } => assert_eq!(client_version, 1),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn encode_decode_request_acquire() {
    let req = Request::Acquire {
        key: LockKey {
            scope: Scope::Tool,
            name: "eslint".to_owned(),
            permits: 1,
        },
        timeout_ms: 5000,
    };
    let frame = encode_frame(&req).unwrap();
    let decoded: Request = decode_frame(&frame[4..]).unwrap();
    match decoded {
        Request::Acquire { key, timeout_ms } => {
            assert_eq!(key.name, "eslint");
            assert_eq!(key.permits, 1);
            assert_eq!(timeout_ms, 5000);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn encode_decode_request_release() {
    let req = Request::Release {
        token: LockToken(42),
    };
    let frame = encode_frame(&req).unwrap();
    let decoded: Request = decode_frame(&frame[4..]).unwrap();
    match decoded {
        Request::Release { token } => assert_eq!(token, LockToken(42)),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn encode_decode_request_status() {
    let frame = encode_frame(&Request::Status).unwrap();
    let decoded: Request = decode_frame(&frame[4..]).unwrap();
    assert!(matches!(decoded, Request::Status));
}

#[test]
fn encode_decode_request_ping() {
    let frame = encode_frame(&Request::Ping).unwrap();
    let decoded: Request = decode_frame(&frame[4..]).unwrap();
    assert!(matches!(decoded, Request::Ping));
}

#[test]
fn encode_decode_response_granted() {
    let resp = Response::Granted {
        token: LockToken(99),
    };
    let frame = encode_frame(&resp).unwrap();
    let decoded: Response = decode_frame(&frame[4..]).unwrap();
    match decoded {
        Response::Granted { token } => assert_eq!(token, LockToken(99)),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn encode_decode_response_timeout() {
    let frame = encode_frame(&Response::Timeout).unwrap();
    let decoded: Response = decode_frame(&frame[4..]).unwrap();
    assert!(matches!(decoded, Response::Timeout));
}

#[test]
fn encode_decode_response_released() {
    let frame = encode_frame(&Response::Released).unwrap();
    let decoded: Response = decode_frame(&frame[4..]).unwrap();
    assert!(matches!(decoded, Response::Released));
}

#[test]
fn encode_decode_response_pong() {
    let frame = encode_frame(&Response::Pong).unwrap();
    let decoded: Response = decode_frame(&frame[4..]).unwrap();
    assert!(matches!(decoded, Response::Pong));
}

#[test]
fn encode_decode_response_status() {
    let resp = Response::Status {
        locks: vec![LockStatus {
            key: LockKey {
                scope: Scope::ToolPath,
                name: "cargo:/tmp".to_owned(),
                permits: 1,
            },
            active_permits: 1,
            waiters: 0,
        }],
    };
    let frame = encode_frame(&resp).unwrap();
    let decoded: Response = decode_frame(&frame[4..]).unwrap();
    match decoded {
        Response::Status { locks } => {
            assert_eq!(locks.len(), 1);
            assert_eq!(locks[0].active_permits, 1);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn encode_decode_response_error() {
    let resp = Response::Error {
        message: "something broke".to_owned(),
    };
    let frame = encode_frame(&resp).unwrap();
    let decoded: Response = decode_frame(&frame[4..]).unwrap();
    match decoded {
        Response::Error { message } => assert_eq!(message, "something broke"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn frame_has_4_byte_length_prefix() {
    let req = Request::Ping;
    let frame = encode_frame(&req).unwrap();
    let len_bytes: [u8; 4] = frame[..4].try_into().unwrap();
    let body_len = u32::from_be_bytes(len_bytes) as usize;
    assert_eq!(body_len, frame.len() - 4);
}

#[test]
fn scope_tool_and_tool_path_are_distinct() {
    let a = Scope::Tool;
    let b = Scope::ToolPath;
    assert_ne!(a, b);
}
