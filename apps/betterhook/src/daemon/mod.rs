//! betterhookd — the coordinator daemon.
//!
//! Spawned on demand the first time a hook declares a lock. Exits 60s
//! after the last client disconnects. Never runs for hooks that don't
//! use isolation, so zero-RSS is the default.

pub mod lifecycle;
pub mod registry;
pub mod server;

pub use lifecycle::IDLE_LINGER;
pub use server::serve;
