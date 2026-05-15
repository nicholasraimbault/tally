//! Internal Worker↔DO request/response types per Phase 0 §2.6 and the
//! dispatch sub-PR Phase 0 wire-format API contracts.
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

/// Dispatch a wake — wire-format per dispatch sub-PR Phase 0 §1.
///
/// Validation at handler entry:
/// - `caller_identity_b64` and `target_identity_b64` parse via
///   [`stoa::types::Identity::from_url_safe_b64`].
/// - `payload_b64` valid base64; decoded length ≤ `MAX_PAYLOAD_BYTES`.
/// - `timeout_ms` in `[MIN_TIMEOUT_MS, MAX_TIMEOUT_MS]`.
/// - `context_id` non-empty UTF-8.
///
/// Validation failures map to [`stoa::StoaError::Wake`] variants
/// ([`stoa::WakeError::DispatchRefused`] / [`stoa::WakeError::InvalidTimeout`])
/// and surface through `stoa_error_to_response`.
#[derive(Debug, Deserialize)]
pub struct DispatchRequest {
    pub caller_identity_b64: String,
    pub target_identity_b64: String,
    pub context_id: String,
    pub payload_b64: String,
    pub timeout_ms: u32,
}

/// Complete a pending wake — wire-format per dispatch sub-PR Phase 0 §2.
///
/// Validation: `by_identity_b64` parses; `wake_id` parses as ULID
/// (26 chars Crockford-base32); `response_payload_b64` valid base64
/// with decoded length ≤ `MAX_RESPONSE_BYTES`.
#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub by_identity_b64: String,
    pub wake_id: String,
    pub response_payload_b64: String,
}

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

/// Dispatch response — wire-format per dispatch sub-PR Phase 0 §3.
///
/// HTTP 200 on success. In MVP, `responding_identity_b64` is the
/// identity that called `complete_wake` (`= wake.target` since only
/// the target completes its own wakes); the field is named generically
/// to keep room for future protocol revisions (e.g., delegated
/// completion).
#[derive(Debug, Serialize)]
pub struct DispatchResponse {
    pub responding_identity_b64: String,
    pub response_payload_b64: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_request_round_trip() {
        let body = r#"{
            "caller_identity_b64": "AAAA",
            "target_identity_b64": "BBBB",
            "context_id": "ctx",
            "payload_b64": "AQID",
            "timeout_ms": 30000
        }"#;
        let req: DispatchRequest = serde_json::from_str(body).expect("deserialize");
        assert_eq!(req.caller_identity_b64, "AAAA");
        assert_eq!(req.target_identity_b64, "BBBB");
        assert_eq!(req.context_id, "ctx");
        assert_eq!(req.payload_b64, "AQID");
        assert_eq!(req.timeout_ms, 30_000);
    }

    #[test]
    fn complete_request_round_trip() {
        let body = r#"{
            "by_identity_b64": "BBBB",
            "wake_id": "01HZZZZZZZZZZZZZZZZZZZZZZZ",
            "response_payload_b64": "BAUG"
        }"#;
        let req: CompleteRequest = serde_json::from_str(body).expect("deserialize");
        assert_eq!(req.by_identity_b64, "BBBB");
        assert_eq!(req.wake_id, "01HZZZZZZZZZZZZZZZZZZZZZZZ");
        assert_eq!(req.response_payload_b64, "BAUG");
    }

    #[test]
    fn dispatch_response_serializes() {
        let resp = DispatchResponse {
            responding_identity_b64: "BBBB".to_string(),
            response_payload_b64: "AQID".to_string(),
        };
        let json = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json["responding_identity_b64"], "BBBB");
        assert_eq!(json["response_payload_b64"], "AQID");
    }
}
