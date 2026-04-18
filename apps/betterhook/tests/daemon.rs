//! End-to-end protocol smoke test: spawn the daemon on a temp socket,
//! connect, hello/acquire/release/ping, disconnect.

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
