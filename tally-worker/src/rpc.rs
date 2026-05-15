//! Internal Worker↔DO request/response types per Phase 0 §2.6 and the
//! dispatch sub-PR Phase 0 wire-format API contracts, with HTTP API
//! surface sub-PR additions for `ValidateApiKey*` and inbox pagination.
//!
//! The Worker layer deserializes incoming public HTTP requests, constructs
//! these types, and forwards to the appropriate `TallyTeamDO` instance via
//! the DO's `fetch` handler. The DO's path-based dispatcher deserializes
//! these from its incoming request body.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct RegisterRequest {
    pub identity_b64: String,
    pub context_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UnregisterRequest {
    pub identity_b64: String,
    pub context_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReadInboxQuery {
    pub wait_seconds: Option<u32>,
    pub limit: Option<u32>,
}

/// Worker→DO RPC: validate a Bearer token and resolve it to an identity.
///
/// Per HTTP API surface sub-PR Phase 0 Decision 1: the Worker forwards
/// the raw `Authorization: Bearer <token>` value; the DO interprets the
/// token, returning [`ValidateApiKeyResponse::valid`] + the resolved
/// identity (when valid).
///
/// MVP behaviour (Phase 0 §4.4): the DO attempts
/// [`stoa::types::Identity::from_url_safe_b64`] on the bearer; success
/// → `valid: true, identity_b64: Some(bearer)`; parse failure →
/// `valid: false, identity_b64: None`. Phase 2 replaces the
/// parse-as-identity logic with real key lookup against
/// `agent:{identity_b64}:api_keys`; wire contract stable across the
/// transition.
#[derive(Debug, Deserialize, Serialize)]
pub struct ValidateApiKeyRequest {
    /// Raw `Authorization: Bearer <token>` value (the part after
    /// `Bearer `). The DO interprets the token; the Worker doesn't
    /// inspect it.
    pub bearer: String,
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
#[derive(Debug, Deserialize, Serialize)]
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
#[derive(Debug, Deserialize, Serialize)]
pub struct CompleteRequest {
    pub by_identity_b64: String,
    pub wake_id: String,
    pub response_payload_b64: String,
}

#[derive(Debug, Serialize)]
pub struct OkResponse;

/// Worker→DO RPC response: validation verdict and resolved identity.
///
/// `valid = false` carries `identity_b64 = None`; `valid = true` carries
/// `identity_b64 = Some(...)`. See [`ValidateApiKeyRequest`] for the
/// validation contract.
#[derive(Debug, Deserialize, Serialize)]
pub struct ValidateApiKeyResponse {
    pub valid: bool,
    pub identity_b64: Option<String>,
}

/// Response body for `GET /v1/teams/{team_id}/agents/{identity}/inbox`.
///
/// `wakes` contains up to `limit` entries (default
/// [`crate::dispatch_consts::DEFAULT_LIMIT`], max
/// [`crate::dispatch_consts::MAX_LIMIT`]). Per HTTP API surface sub-PR
/// Phase 0 §3.8, `more_available` indicates whether the inbox has more
/// entries than were returned in this response.
#[derive(Debug, Deserialize, Serialize)]
pub struct ReadInboxResponse {
    pub wakes: Vec<WakeSummary>,
    pub more_available: bool,
}

#[derive(Debug, Deserialize, Serialize)]
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
#[derive(Debug, Deserialize, Serialize)]
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

    #[test]
    fn validate_api_key_request_round_trip() {
        // Decision 1 wire shape: { bearer: String }.
        let body = r#"{"bearer": "AAAA"}"#;
        let req: ValidateApiKeyRequest = serde_json::from_str(body).expect("deserialize");
        assert_eq!(req.bearer, "AAAA");

        let reencoded = serde_json::to_value(&req).expect("serialize");
        assert_eq!(reencoded["bearer"], "AAAA");
    }

    #[test]
    fn validate_api_key_response_valid_round_trip() {
        // Decision 1 wire shape: { valid: bool, identity_b64: Option<String> }.
        let resp = ValidateApiKeyResponse {
            valid: true,
            identity_b64: Some("AAAA".to_string()),
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        let decoded: ValidateApiKeyResponse = serde_json::from_str(&json).expect("deserialize");
        assert!(decoded.valid);
        assert_eq!(decoded.identity_b64.as_deref(), Some("AAAA"));
    }

    #[test]
    fn validate_api_key_response_invalid_round_trip() {
        let resp = ValidateApiKeyResponse {
            valid: false,
            identity_b64: None,
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        let decoded: ValidateApiKeyResponse = serde_json::from_str(&json).expect("deserialize");
        assert!(!decoded.valid);
        assert!(decoded.identity_b64.is_none());
    }

    #[test]
    fn read_inbox_response_with_more_available() {
        // Phase 0 §3.8 pagination: more_available flag in response.
        let resp = ReadInboxResponse {
            wakes: vec![WakeSummary {
                wake_id: "01HZZZZZZZZZZZZZZZZZZZZZZZ".to_string(),
                caller_identity_b64: "AAAA".to_string(),
                context_id: "ctx".to_string(),
                payload_b64: "AQID".to_string(),
            }],
            more_available: true,
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        let decoded: ReadInboxResponse = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.wakes.len(), 1);
        assert!(decoded.more_available);
        assert_eq!(decoded.wakes[0].wake_id, "01HZZZZZZZZZZZZZZZZZZZZZZZ");
    }

    #[test]
    fn read_inbox_response_no_more_available() {
        let resp = ReadInboxResponse {
            wakes: vec![],
            more_available: false,
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        let decoded: ReadInboxResponse = serde_json::from_str(&json).expect("deserialize");
        assert!(decoded.wakes.is_empty());
        assert!(!decoded.more_available);
    }
}
