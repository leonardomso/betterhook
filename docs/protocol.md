# Betterhook daemon IPC protocol

The `betterhookd` coordinator daemon serves a tiny request/response
protocol over a Unix domain socket. This document is the normative
reference for third-party agent harnesses (Conductor, Cursor, Codex,
Aider, …) that want to integrate betterhook directly.

## Transport

- **Socket path**: `<git-common-dir>/betterhook/sock`. If that path
  would exceed the macOS 104-char limit, the daemon falls back to
  `/tmp/bh-<sha8>.sock` and writes the chosen path into
  `<git-common-dir>/betterhook/sockpath`.
- **Discovery env var**: `BETTERHOOK_DAEMON_SOCK`. When set, the
  client skips auto-discovery and connects to the value directly.
- **Framing**: big-endian length-prefixed (`tokio_util::codec::LengthDelimitedCodec`
  with `length_field_length = 4`).
- **Encoding**: [`bincode` v2](https://docs.rs/bincode) with
  `bincode::config::standard()` using serde.
- **Protocol version**: see `PROTOCOL_VERSION` in
  `apps/betterhook/src/lock/protocol.rs`. Currently `1`. Bumped on
  any backwards-incompatible change.

## Lifecycle

1. Client connects to the socket.
2. Client sends `Hello { client_version }`. Server replies
   `Hello { server_version }`. A mismatching major version is an
   immediate abort on the client side.
3. Client sends zero or more `Acquire` / `Release` / `Status` /
   `Ping` requests. Each request gets exactly one response.
4. Client disconnects. The daemon drops every permit the connection
   held (explicit or implicit) and decrements the active-connection
   count.
5. When the active-connection count reaches zero and 60 seconds pass
   without a new connection, the daemon exits and removes the socket
   file.

## Requests

```rust
enum Request {
    Hello { client_version: u32 },
    Acquire { key: LockKey, timeout_ms: u64 },
    Release { token: u64 },
    Status,
    Ping,
}

struct LockKey {
    scope: Scope,       // "tool" | "tool_path"
    name: String,       // opaque identifier; see key derivation below
    permits: u32,       // capacity hint; 1 = mutex, N = sharded
}
```

### Key derivation

The client computes `LockKey` from its local `IsolateSpec`:

| `IsolateSpec` variant                                   | `key.name`                         | `permits` |
|--------------------------------------------------------|------------------------------------|-----------|
| `Tool { name }`                                        | `tool:<name>`                      | 1         |
| `Sharded { name, slots }`                              | `sharded:<name>`                   | `slots`   |
| `ToolPath { tool, PerWorktree }`                       | `tool-path:<tool>:<worktree>`      | 1         |
| `ToolPath { tool, Path(p) }`                           | `tool-path:<tool>:<p>`             | 1         |

Per-worktree keys always have `permits = 1` because each worktree
gets a distinct key; contention only happens if two things inside
the same worktree race, which shouldn't.

## Responses

```rust
enum Response {
    Hello { server_version: u32 },
    Granted { token: u64 },
    Timeout,
    Released,
    Pong,
    Status { locks: Vec<LockStatus> },
    Error { message: String },
}

struct LockStatus {
    key: LockKey,
    active_permits: u32,
    waiters: u32,
}
```

## Request semantics

### `Acquire { key, timeout_ms }`

Blocks until a permit on `key` is free, or `timeout_ms` elapses. The
daemon lazily creates the semaphore on the first acquire, using
`key.permits` as its capacity. Subsequent acquires may pass any value
— the first caller wins capacity assignment and later mismatches are
accepted without resizing.

On success: `Response::Granted { token }` where `token` is a
monotonic per-daemon counter. Clients must remember the token to call
`Release` later.

On timeout: `Response::Timeout`. No token is returned.

On a semaphore error: `Response::Error { message }`.

### `Release { token }`

Drops exactly one held permit by token. Idempotent — releasing an
unknown token is a no-op that still returns `Response::Released`. If
the client disconnects without calling Release, the per-connection
`HashMap<token, HeldPermit>` drops, which releases every permit the
connection held.

### `Status`

Returns a snapshot of every live `LockKey` with `active_permits`
(permits currently held) and `waiters` (not yet populated as of v1;
phase 16+ will wire it in).

### `Ping` / `Hello`

Liveness and version handshake. The client uses `Ping` to verify the
daemon is alive after spawn-retry; `Hello` is the version gate.

## Error handling

- **Wrong protocol version** (client rejects): the client `Hello` returns
  `Response::Hello { server_version }` — if it doesn't match the
  client's major, the client closes the connection and surfaces a
  setup error to the hook runner.
- **Decode error**: the daemon sends `Response::Error { message: "decode error: ..." }`
  and continues reading the next frame. Framing is recoverable;
  malformed bodies are not fatal.
- **Socket unreachable** (`ECONNREFUSED` or file missing): the client
  tries to spawn `betterhookd --socket <path>`. If spawn fails, it
  falls back to `fs4` advisory flock on
  `<common-dir>/betterhook/locks/<key>.lock` — same mutex semantics,
  loses sharded and status support.

## Bypass

Setting `BETTERHOOK_NO_LOCKS=1` in the agent environment or passing
`--no-locks` to the dispatch command skips the daemon and the file
fallback entirely. Jobs that declared `isolate` run unlocked with a
one-line warning on stderr.

## Example Python client

```python
import os, socket, struct

SOCK = os.environ.get(
    "BETTERHOOK_DAEMON_SOCK",
    os.path.join(".git", "betterhook", "sock"),
)

def send(sock, frame: bytes):
    sock.send(struct.pack(">I", len(frame)) + frame)

def recv(sock) -> bytes:
    (n,) = struct.unpack(">I", sock.recv(4))
    return sock.recv(n)

# Bincode-encoded bodies go here. Use a bincode binding (e.g.
# `bincode2-py`) or regenerate the wire format from the Rust sources.
```

## Versioning

`PROTOCOL_VERSION` is bumped for any backwards-incompatible change.
Additive changes (new Request or Response variants) may land without
a bump as long as existing variants keep their tags and layouts. Any
breaking change ships alongside a documented upgrade path in
`CHANGELOG.md`.
