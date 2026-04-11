//! Coordinator-daemon client, protocol, and filesystem fallback.
//!
//! The daemon lives in `crate::daemon`. This module is the *client*
//! side of the wire and the protocol types both sides share.

pub mod protocol;

pub use protocol::{
    LockKey, LockStatus, PROTOCOL_VERSION, Request, Response, Scope, decode_frame, encode_frame,
};
