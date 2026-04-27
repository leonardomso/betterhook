//! Wire protocol for the betterhookd coordinator daemon.
//!
//! Length-prefixed frames over a Unix domain socket. Each frame is a
//! bincode-serialized [`Request`] from the client or [`Response`] from
//! the daemon. The protocol is serde-tagged for forward compat so
//! agents can fingerprint unknown variants and reject them cleanly.

use serde::{Deserialize, Serialize};

/// Bumped whenever a backwards-incompatible change is made. Clients
/// refuse to talk to daemons with a different version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Granularity of a lock key. The client computes the right variant
/// from its local `IsolateSpec`; the daemon treats each `(scope, name)`
/// tuple as an opaque identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// A global per-tool mutex or sharded semaphore.
    Tool,
    /// A `(tool, path)` pair — one permit per worktree.
    ToolPath,
}

/// A concrete key that identifies a single lock in the daemon's
/// registry. `permits` is set by the first acquirer and controls the
/// sharded-semaphore capacity; subsequent acquirers must pass the same
/// value or the daemon rejects the request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LockKey {
    pub scope: Scope,
    pub name: String,
    pub permits: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LockToken(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Identify protocol version before issuing real requests.
    Hello { client_version: u32 },
    /// Block until a permit on `key` is free, up to `timeout_ms`.
    Acquire { key: LockKey, timeout_ms: u64 },
    /// Release the permit identified by `token`. Idempotent.
    Release { token: LockToken },
    /// Snapshot of every live lock. Used by `betterhook status`.
    Status,
    /// Liveness check.
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Hello { server_version: u32 },
    Granted { token: LockToken },
    Timeout,
    Released,
    Pong,
    Status { locks: Vec<LockStatus> },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockStatus {
    pub key: LockKey,
    pub active_permits: u32,
    pub waiters: u32,
}

/// Encode a message with a 4-byte big-endian length prefix.
#[must_use = "encoded frame must be sent over the wire"]
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, bincode::error::EncodeError> {
    let body = bincode::serde::encode_to_vec(msg, bincode::config::standard())?;
    let len = u32::try_from(body.len()).expect("frame larger than 4 GiB");
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a single bincode message from a byte slice (the length
/// prefix is handled by the framed codec on the socket, so this just
/// unwraps the body).
#[must_use = "decoded message must be inspected"]
pub fn decode_frame<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
) -> Result<T, bincode::error::DecodeError> {
    let (decoded, _) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())?;
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_request() {
        let req = Request::Acquire {
            key: LockKey {
                scope: Scope::Tool,
                name: "eslint".to_owned(),
                permits: 1,
            },
            timeout_ms: 30_000,
        };
        let frame = encode_frame(&req).unwrap();
        // The 4-byte length prefix is prepended; strip it for the decode.
        let body = &frame[4..];
        let decoded: Request = decode_frame(body).unwrap();
        match decoded {
            Request::Acquire { key, timeout_ms } => {
                assert_eq!(key.name, "eslint");
                assert_eq!(key.permits, 1);
                assert_eq!(timeout_ms, 30_000);
            }
            _ => panic!("decoded wrong variant"),
        }
    }

    #[test]
    fn round_trip_response() {
        let resp = Response::Granted {
            token: LockToken(42),
        };
        let frame = encode_frame(&resp).unwrap();
        let decoded: Response = decode_frame(&frame[4..]).unwrap();
        match decoded {
            Response::Granted { token } => assert_eq!(token, LockToken(42)),
            _ => panic!(),
        }
    }
}
