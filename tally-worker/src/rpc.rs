//! Internal Worker↔DO request/response types per Phase 0 §2.6.
//!
//! The Worker layer deserializes incoming public HTTP requests, constructs
//! these types, and forwards to the appropriate `TallyTeamDO` instance via
//! the DO's `fetch` handler. The DO's path-based dispatcher deserializes
//! these from its incoming request body.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub identity_b64: String,
    pub context_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UnregisterRequest {
    pub identity_b64: String,
    pub context_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ReadInboxQuery {
    pub wait_seconds: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct ValidateApiKeyRequest {
    pub identity_b64: String,
    pub api_key: String,
}

// DispatchRequest and CompleteRequest are deferred to the dispatch sub-PR.

#[derive(Debug, Serialize)]
pub struct OkResponse;

#[derive(Debug, Serialize)]
pub struct ValidateApiKeyResponse {
    pub valid: bool,
}

#[derive(Debug, Serialize)]
pub struct ReadInboxResponse {
    pub wakes: Vec<WakeSummary>,
}

#[derive(Debug, Serialize)]
pub struct WakeSummary {
    pub wake_id: String,
    pub caller_identity_b64: String,
    pub context_id: String,
    pub payload_b64: String,
}
