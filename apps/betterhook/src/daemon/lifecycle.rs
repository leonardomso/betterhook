//! Idle-linger timer.

use std::time::Duration;

/// How long the daemon keeps running past the last client disconnect
/// before exiting. Phase 14 default matches the plan: 60 seconds.
pub const IDLE_LINGER: Duration = Duration::from_secs(60);
