//! Unix-socket listener and per-connection handler.
//!
//! The daemon accepts connections on a socket path provided by the
//! client (or spawner) and serves `Request → Response` frames until
//! the client disconnects. Exits after [`IDLE_LINGER`] of no active
//! connections.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use super::lifecycle::IDLE_LINGER;
use super::registry::{HeldPermit, Registry};
use crate::lock::protocol::{LockKey, LockToken, PROTOCOL_VERSION, Request, Response};

/// Track the number of currently-connected clients. When this hits 0
/// we arm the idle-exit timer.
#[derive(Default, Debug)]
struct Connections {
    active: AtomicU64,
}

/// Run the daemon's accept loop on `socket_path`. Returns when the
/// socket is removed or an idle-exit fires.
pub async fn serve(socket_path: &Path) -> std::io::Result<()> {
    // Clean up a stale socket file if one is lingering from a previous
    // crash — only after verifying no one is listening on it.
    if socket_path.exists() {
        if UnixStream::connect(socket_path).await.is_err() {
            let _ = tokio::fs::remove_file(socket_path).await;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "another betterhookd is already serving this socket",
            ));
        }
    }
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = UnixListener::bind(socket_path)?;
    tracing::info!(path = %socket_path.display(), "betterhookd listening");

    let registry = Registry::new();
    let connections = Arc::new(Connections::default());
    let next_token = Arc::new(AtomicU64::new(1));

    loop {
        let accept = listener.accept();
        let idle_timer = async {
            if connections.active.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(IDLE_LINGER).await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            conn = accept => {
                let (stream, _) = conn?;
                connections.active.fetch_add(1, Ordering::SeqCst);
                let registry = registry.clone();
                let connections = connections.clone();
                let next_token = next_token.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, registry, next_token).await {
                        tracing::warn!(error = %e, "connection handler ended with error");
                    }
                    connections.active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            () = idle_timer => {
                tracing::info!("idle linger elapsed, exiting");
                break;
            }
        }
    }

    let _ = tokio::fs::remove_file(socket_path).await;
    Ok(())
}

async fn handle_connection(
    stream: UnixStream,
    registry: Registry,
    next_token: Arc<AtomicU64>,
) -> std::io::Result<()> {
    let codec = LengthDelimitedCodec::builder()
        .length_field_length(4)
        .big_endian()
        .new_codec();
    let mut framed = Framed::new(stream, codec);

    // Each connection holds its acquired permits by token so explicit
    // `Release` requests can drop exactly one, while connection close
    // drops them all (via the map's Drop).
    let held: Mutex<HashMap<LockToken, HeldPermit>> = Mutex::new(HashMap::new());

    while let Some(frame) = framed.next().await {
        let frame = frame?;
        let request: Request = match crate::lock::protocol::decode_frame(&frame) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::Error {
                    message: format!("decode error: {e}"),
                };
                send(&mut framed, &resp).await?;
                continue;
            }
        };

        let response = match request {
            Request::Hello { .. } => Response::Hello {
                server_version: PROTOCOL_VERSION,
            },
            Request::Ping => Response::Pong,
            Request::Status => Response::Status {
                locks: registry.snapshot().await,
            },
            Request::Acquire { key, timeout_ms } => {
                handle_acquire(&registry, key, timeout_ms, &next_token, &held).await
            }
            Request::Release { token } => {
                held.lock().await.remove(&token);
                Response::Released
            }
        };

        send(&mut framed, &response).await?;
    }

    Ok(())
}

async fn handle_acquire(
    registry: &Registry,
    key: LockKey,
    timeout_ms: u64,
    next_token: &AtomicU64,
    held: &Mutex<HashMap<LockToken, HeldPermit>>,
) -> Response {
    let sem = match registry.semaphore(&key).await {
        Ok(s) => s,
        Err(message) => return Response::Error { message },
    };
    let acquire_fut = sem.acquire_owned();
    let permit = if timeout_ms == 0 {
        match acquire_fut.await {
            Ok(p) => p,
            Err(e) => {
                return Response::Error {
                    message: format!("semaphore closed: {e}"),
                };
            }
        }
    } else {
        match tokio::time::timeout(Duration::from_millis(timeout_ms), acquire_fut).await {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                return Response::Error {
                    message: format!("semaphore closed: {e}"),
                };
            }
            Err(_) => return Response::Timeout,
        }
    };
    let token = LockToken(next_token.fetch_add(1, Ordering::SeqCst));
    held.lock()
        .await
        .insert(token, HeldPermit { token, permit });
    Response::Granted { token }
}

async fn send<S, T>(framed: &mut Framed<S, LengthDelimitedCodec>, msg: &T) -> std::io::Result<()>
where
    S: tokio::io::AsyncWrite + tokio::io::AsyncRead + Unpin,
    T: serde::Serialize,
{
    let body = match bincode::serde::encode_to_vec(msg, bincode::config::standard()) {
        Ok(b) => b,
        Err(e) => {
            return Err(std::io::Error::other(format!("encode: {e}")));
        }
    };
    framed.send(Bytes::from(body)).await?;
    Ok(())
}
