//! Storage types for wake lifecycle.
//!
//! [`WakeRecord`] is written by dispatch (deferred to a subsequent
//! Workstream C PR) and read by dispatch and complete_wake. The type
//! is defined here in this PR so the dispatch sub-PR can consume it
//! without a separate type-introduction commit.

use serde::{Deserialize, Serialize};

/// Persistent record of a wake's lifecycle. One row per wake_id under
/// the `wake:{wake_id}` storage key. Written by dispatch (deferred to
/// a subsequent PR); read by dispatch and complete_wake.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WakeRecord {
    pub caller_identity_b64: String,
    pub target_identity_b64: String,
    pub context_id: String,
    pub payload_b64: String,
    pub timeout_ms: u32,
    pub state: WakeState,
    pub response_b64: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

/// Lifecycle state of a wake row.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WakeState {
    Pending,
    Completed,
    TimedOut,
}
