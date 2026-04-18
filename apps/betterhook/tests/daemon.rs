//! Daemon protocol tests: spawn the daemon on a temp socket, verify
//! the full request/response protocol surface.

use std::time::Duration;

use betterhook::daemon::serve;
use betterhook::lock::protocol::{LockKey, Request, Response, Scope, encode_frame};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

async fn next_response(framed: &mut Framed<UnixStream, LengthDelimitedCodec>) -> Response {
    let frame = framed.next().await.unwrap().unwrap();
    betterhook::lock::protocol::decode_frame(&frame).unwrap()
}

async fn send_request(framed: &mut Framed<UnixStream, LengthDelimitedCodec>, req: &Request) {
    let encoded = encode_frame(req).unwrap();
    // Strip the 4-byte len prefix since the codec handles it.
    let body = Bytes::copy_from_slice(&encoded[4..]);
    framed.send(body).await.unwrap();
}

#[tokio::test]
async fn acquire_then_release_a_mutex() {
    let dir = TempDir::new().unwrap();
    let sock = dir.path().join("bh.sock");

    let sock_server = sock.clone();
    let server = tokio::spawn(async move {
        let _ = serve(&sock_server).await;
    });

    // Wait for the listener to come up.
    for _ in 0..30 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(sock.exists(), "socket did not appear");

    let stream = UnixStream::connect(&sock).await.unwrap();
    let codec = LengthDelimitedCodec::builder()
        .length_field_length(4)
        .big_endian()
        .new_codec();
    let mut framed = Framed::new(stream, codec);

    // Hello
    send_request(
        &mut framed,
        &Request::Hello {
            client_version: betterhook::lock::protocol::PROTOCOL_VERSION,
        },
    )
    .await;
    match next_response(&mut framed).await {
        Response::Hello { server_version } => {
            assert_eq!(server_version, betterhook::lock::protocol::PROTOCOL_VERSION);
        }
        other => panic!("expected Hello, got {other:?}"),
    }

    // Acquire
    let key = LockKey {
        scope: Scope::Tool,
        name: "eslint".to_owned(),
        permits: 1,
    };
    send_request(
        &mut framed,
        &Request::Acquire {
            key: key.clone(),
            timeout_ms: 1_000,
        },
    )
    .await;
    let token = match next_response(&mut framed).await {
        Response::Granted { token } => token,
        other => panic!("expected Granted, got {other:?}"),
    };

    // Release
    send_request(&mut framed, &Request::Release { token }).await;
    match next_response(&mut framed).await {
        Response::Released => {}
        other => panic!("expected Released, got {other:?}"),
    }

    // Ping
    send_request(&mut framed, &Request::Ping).await;
    match next_response(&mut framed).await {
        Response::Pong => {}
        other => panic!("expected Pong, got {other:?}"),
    }

    drop(framed);
    server.abort();
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

async fn spawn_daemon(dir: &TempDir) -> (tokio::task::JoinHandle<()>, std::path::PathBuf) {
    let sock = dir.path().join("bh.sock");
    let sock_server = sock.clone();
    let server = tokio::spawn(async move {
        let _ = serve(&sock_server).await;
    });
    for _ in 0..30 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(sock.exists(), "socket did not appear");
    (server, sock)
}

async fn connect(sock: &std::path::Path) -> Framed<UnixStream, LengthDelimitedCodec> {
    let stream = UnixStream::connect(sock).await.unwrap();
    let codec = LengthDelimitedCodec::builder()
        .length_field_length(4)
        .big_endian()
        .new_codec();
    Framed::new(stream, codec)
}

async fn hello(framed: &mut Framed<UnixStream, LengthDelimitedCodec>) {
    send_request(
        framed,
        &Request::Hello {
            client_version: betterhook::lock::protocol::PROTOCOL_VERSION,
        },
    )
    .await;
    match next_response(framed).await {
        Response::Hello { .. } => {}
        other => panic!("expected Hello, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Status request
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_request_returns_empty_on_fresh_daemon() {
    let dir = TempDir::new().unwrap();
    let (server, sock) = spawn_daemon(&dir).await;
    let mut framed = connect(&sock).await;
    hello(&mut framed).await;

    send_request(&mut framed, &Request::Status).await;
    match next_response(&mut framed).await {
        Response::Status { locks } => {
            assert!(locks.is_empty(), "fresh daemon should have no locks");
        }
        other => panic!("expected Status, got {other:?}"),
    }

    drop(framed);
    server.abort();
}

#[tokio::test]
async fn status_reflects_held_lock() {
    let dir = TempDir::new().unwrap();
    let (server, sock) = spawn_daemon(&dir).await;
    let mut framed = connect(&sock).await;
    hello(&mut framed).await;

    let key = LockKey {
        scope: Scope::Tool,
        name: "cargo".to_owned(),
        permits: 1,
    };
    send_request(
        &mut framed,
        &Request::Acquire {
            key: key.clone(),
            timeout_ms: 1000,
        },
    )
    .await;
    let _token = match next_response(&mut framed).await {
        Response::Granted { token } => token,
        other => panic!("expected Granted, got {other:?}"),
    };

    send_request(&mut framed, &Request::Status).await;
    match next_response(&mut framed).await {
        Response::Status { locks } => {
            assert_eq!(locks.len(), 1);
            assert_eq!(locks[0].key.name, "cargo");
            assert_eq!(locks[0].active_permits, 1);
        }
        other => panic!("expected Status, got {other:?}"),
    }

    drop(framed);
    server.abort();
}

// ---------------------------------------------------------------------------
// Acquire timeout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acquire_times_out_when_lock_held() {
    let dir = TempDir::new().unwrap();
    let (server, sock) = spawn_daemon(&dir).await;

    let key = LockKey {
        scope: Scope::Tool,
        name: "cargo".to_owned(),
        permits: 1,
    };

    // Client 1: acquire and hold.
    let mut c1 = connect(&sock).await;
    hello(&mut c1).await;
    send_request(
        &mut c1,
        &Request::Acquire {
            key: key.clone(),
            timeout_ms: 5000,
        },
    )
    .await;
    match next_response(&mut c1).await {
        Response::Granted { .. } => {}
        other => panic!("c1 expected Granted, got {other:?}"),
    }

    // Client 2: try to acquire with short timeout.
    let mut c2 = connect(&sock).await;
    hello(&mut c2).await;
    send_request(
        &mut c2,
        &Request::Acquire {
            key: key.clone(),
            timeout_ms: 50,
        },
    )
    .await;
    match next_response(&mut c2).await {
        Response::Timeout => {}
        other => panic!("c2 expected Timeout, got {other:?}"),
    }

    drop(c1);
    drop(c2);
    server.abort();
}

// ---------------------------------------------------------------------------
// Token monotonicity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tokens_are_monotonically_increasing() {
    let dir = TempDir::new().unwrap();
    let (server, sock) = spawn_daemon(&dir).await;
    let mut framed = connect(&sock).await;
    hello(&mut framed).await;

    let mut tokens = Vec::new();
    for i in 0..3 {
        let key = LockKey {
            scope: Scope::Tool,
            name: format!("tool-{i}"),
            permits: 1,
        };
        send_request(
            &mut framed,
            &Request::Acquire {
                key,
                timeout_ms: 1000,
            },
        )
        .await;
        match next_response(&mut framed).await {
            Response::Granted { token } => tokens.push(token),
            other => panic!("expected Granted, got {other:?}"),
        }
    }
    for pair in tokens.windows(2) {
        assert!(pair[0] < pair[1], "tokens should be strictly increasing");
    }

    drop(framed);
    server.abort();
}

// ---------------------------------------------------------------------------
// Connection drop releases held permits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connection_drop_releases_held_permits() {
    let dir = TempDir::new().unwrap();
    let (server, sock) = spawn_daemon(&dir).await;

    let key = LockKey {
        scope: Scope::Tool,
        name: "cargo".to_owned(),
        permits: 1,
    };

    // Client 1: acquire the lock, then disconnect without releasing.
    {
        let mut c1 = connect(&sock).await;
        hello(&mut c1).await;
        send_request(
            &mut c1,
            &Request::Acquire {
                key: key.clone(),
                timeout_ms: 1000,
            },
        )
        .await;
        match next_response(&mut c1).await {
            Response::Granted { .. } => {}
            other => panic!("c1 expected Granted, got {other:?}"),
        }
        // c1 is dropped here — should release the permit via RAII.
    }

    // Small delay for the daemon to process the disconnect.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Client 2: should now be able to acquire the same lock.
    let mut c2 = connect(&sock).await;
    hello(&mut c2).await;
    send_request(
        &mut c2,
        &Request::Acquire {
            key,
            timeout_ms: 500,
        },
    )
    .await;
    match next_response(&mut c2).await {
        Response::Granted { .. } => {}
        other => panic!("c2 expected Granted after c1 dropped, got {other:?}"),
    }

    drop(c2);
    server.abort();
}

// ---------------------------------------------------------------------------
// Multi-client with different keys
// ---------------------------------------------------------------------------

#[tokio::test]
async fn different_keys_do_not_block_each_other() {
    let dir = TempDir::new().unwrap();
    let (server, sock) = spawn_daemon(&dir).await;

    let mut c1 = connect(&sock).await;
    hello(&mut c1).await;
    send_request(
        &mut c1,
        &Request::Acquire {
            key: LockKey {
                scope: Scope::Tool,
                name: "eslint".to_owned(),
                permits: 1,
            },
            timeout_ms: 1000,
        },
    )
    .await;
    match next_response(&mut c1).await {
        Response::Granted { .. } => {}
        other => panic!("c1 expected Granted, got {other:?}"),
    }

    let mut c2 = connect(&sock).await;
    hello(&mut c2).await;
    send_request(
        &mut c2,
        &Request::Acquire {
            key: LockKey {
                scope: Scope::Tool,
                name: "prettier".to_owned(),
                permits: 1,
            },
            timeout_ms: 1000,
        },
    )
    .await;
    match next_response(&mut c2).await {
        Response::Granted { .. } => {}
        other => panic!("c2 expected Granted on different key, got {other:?}"),
    }

    drop(c1);
    drop(c2);
    server.abort();
}

// ---------------------------------------------------------------------------
// Stale socket cleanup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stale_socket_is_cleaned_up() {
    let dir = TempDir::new().unwrap();
    let sock = dir.path().join("stale.sock");

    // Create a dangling socket file.
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    drop(listener);
    assert!(sock.exists(), "stale socket should exist");

    // serve() should remove the stale socket and start fresh.
    let sock_server = sock.clone();
    let server = tokio::spawn(async move {
        let _ = serve(&sock_server).await;
    });

    for _ in 0..30 {
        if sock.exists() && UnixStream::connect(&sock).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let mut framed = connect(&sock).await;
    hello(&mut framed).await;

    send_request(&mut framed, &Request::Ping).await;
    match next_response(&mut framed).await {
        Response::Pong => {}
        other => panic!("expected Pong, got {other:?}"),
    }

    drop(framed);
    server.abort();
}
