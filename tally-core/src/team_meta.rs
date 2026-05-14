//! Per-DO metadata.
//!
//! [`TeamMeta`] is lazy-initialized on the first request to a
//! `TallyTeamDO` instance; written once at that point and read on
//! every subsequent request (for tenancy verification).

use serde::{Deserialize, Serialize};

/// Per-DO metadata. Lazy-initialized on first request; written once.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamMeta {
    pub tenancy_prefix: String,
    pub team_id_b64: String,
    pub created_at: i64,
}
