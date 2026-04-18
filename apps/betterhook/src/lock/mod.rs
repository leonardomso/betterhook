//! Coordinator-daemon client, protocol, and filesystem fallback.
//!
//! The daemon lives in `crate::daemon`. This module is the *client*
//! side of the wire and the protocol types both sides share.

pub mod client;
pub mod flock;
pub mod protocol;

pub use client::{LockGuard, acquire_job_lock, key_for_spec, lock_dir};
pub use flock::FileLock;
pub use protocol::{
    LockKey, LockStatus, PROTOCOL_VERSION, Request, Response, Scope, decode_frame, encode_frame,
};
