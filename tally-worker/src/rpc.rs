//! Internal Worker↔DO request/response types per Phase 0 §2.6 and the
//! dispatch sub-PR Phase 0 wire-format API contracts, with HTTP API
//! surface sub-PR additions for `ValidateApiKey*` and inbox pagination.
//!
//! The Worker layer deserializes incoming **public** HTTP requests (the
//! `Public*` types in this module — wire-shape per
//! `phase-1b-sub-pr-1-phase-0.md` §3.3), translates field names and
//! injects authenticated-identity / URL-path data, then forwards the
//! resulting **internal** RPC body (the un-prefixed types in this
//! module) to the DO. The DO returns internal shapes; the Worker
//! translates those back to `Public*` response bodies before serving
//! the public HTTP response.
//!
//! ## Public vs internal coexistence (HTTP API surface sub-PR F.1 expansion)
//!
//! Internal types ([`DispatchRequest`], [`CompleteRequest`],
//! [`DispatchResponse`], [`RegisterRequest`], [`UnregisterRequest`],
//! [`ReadInboxResponse`], [`WakeSummary`]) carry the `_b64` field-name
//! suffix and millisecond units that the DO operates in. Public types
//! ([`PublicDispatchRequest`], [`PublicCompleteRequest`],
//! [`PublicRegisterRequest`], [`PublicDispatchResponse`],
//! [`PublicReadInboxResponse`], [`PublicWakeSummary`]) match the §3.3
//! public spec verbatim — no `_b64` suffixes, ISO-8601 strings for
//! timestamps, seconds for human-meaningful durations. The Worker
//! layer performs the field-name rename, identity injection, and
//! seconds↔milliseconds + epoch-millis↔ISO-8601 conversions.

use serde::{Deserialize, Serialize};

use crate::wake_types::WakeId;

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

/// Per-wake summary in the internal [`ReadInboxResponse`].
///
/// `expires_at_ms` carries the raw unix-millisecond expiry (computed
/// from the underlying [`crate::wake_types::WakeRecord`]'s
/// `created_at + timeout_ms`). The Worker layer formats this as
/// ISO-8601 when building the public [`PublicWakeSummary`].
#[derive(Debug, Deserialize, Serialize)]
pub struct WakeSummary {
    pub wake_id: String,
    pub caller_identity_b64: String,
    pub context_id: String,
    pub payload_b64: String,
    pub expires_at_ms: u64,
}

/// Dispatch response — wire-format per dispatch sub-PR Phase 0 §3, with
/// the HTTP API surface sub-PR F.1 expansion that adds `wake_id` and
/// `completed_at` to the internal shape.
///
/// HTTP 200 on success. In MVP, `responding_identity_b64` is the
/// identity that called `complete_wake` (`= wake.target` since only
/// the target completes its own wakes); the field is named generically
/// to keep room for future protocol revisions (e.g., delegated
/// completion).
///
/// **§9.1 mid-implementation correction:** the dispatch sub-PR's
/// internal `DispatchResponse` (committed in §9.1) carried only
/// `responding_identity_b64 + response_payload_b64`. The public
/// response shape from `phase-1b-sub-pr-1-phase-0.md` §3.3 requires
/// `wake_id` and `completed_at` — neither of which was reachable from
/// the §9.1 internal shape. The HTTP API surface sub-PR F.1 expansion
/// adds these fields to the internal shape so the Worker layer's
/// public-shape translation has the source data. `completed_at` is
/// unix-millisecond epoch time captured by `complete_wake`'s
/// post-storage step; the Worker formats it as ISO-8601.
#[derive(Debug, Deserialize, Serialize)]
pub struct DispatchResponse {
    pub wake_id: WakeId,
    pub responding_identity_b64: String,
    pub response_payload_b64: String,
    pub completed_at: u64,
}

// ─── Public wire shapes (HTTP API surface sub-PR F.1 expansion) ─────────────
//
// The types below mirror the public HTTP request/response bodies
// specified verbatim in `phase-1b-sub-pr-1-phase-0.md` §3.3. They are
// the Worker's deserialization targets for inbound requests and
// serialization sources for outbound responses. The Worker translates
// field names + units between Public* and the internal types above.

/// Public request body for `POST /v1/teams/{team_id}/wakes`.
///
/// Wire shape per `phase-1b-sub-pr-1-phase-0.md` §3.3:
///
/// ```json
/// {
///   "target_identity": "base64(target_bytes)",
///   "context_id": "task-routing",
///   "payload": "base64(opaque ciphertext)",
///   "timeout_seconds": 30
/// }
/// ```
///
/// Translation to internal [`DispatchRequest`]:
/// - `target_identity` → `target_identity_b64`
/// - `payload` → `payload_b64`
/// - `timeout_seconds * 1000` → `timeout_ms`
/// - `context_id` → `context_id` (unchanged)
/// - `caller_identity_b64` is injected by the Worker from the
///   authenticated bearer; not a public field.
#[derive(Debug, Deserialize, Serialize)]
pub struct PublicDispatchRequest {
    pub target_identity: String,
    pub context_id: String,
    pub payload: String,
    pub timeout_seconds: u32,
}

/// Public request body for
/// `POST /v1/teams/{team_id}/wakes/{wake_id}/complete`.
///
/// Wire shape per §3.3:
///
/// ```json
/// { "response": "base64(opaque ciphertext)" }
/// ```
///
/// Translation to internal [`CompleteRequest`]:
/// - `response` → `response_payload_b64`
/// - `wake_id` injected by the Worker from the URL path segment.
/// - `by_identity_b64` injected by the Worker from the authenticated
///   bearer.
#[derive(Debug, Deserialize, Serialize)]
pub struct PublicCompleteRequest {
    pub response: String,
}

/// Public request body for
/// `POST /v1/teams/{team_id}/agents/{identity}/register`.
///
/// Wire shape per §3.3:
///
/// ```json
/// {
///   "context_id": "task-routing",
///   "metadata": { "role": "engineer" }
/// }
/// ```
///
/// Translation to internal [`RegisterRequest`]:
/// - `context_id` → `context_id` (unchanged)
/// - `identity_b64` injected by the Worker from the URL path
///   segment (authenticated against the bearer).
/// - `metadata` is opaque JSON the DO is **specced to store** but
///   doesn't interpret. MVP scope boundary: the field is accepted
///   here (deserialized into a free-form `serde_json::Value` so the
///   public contract is honored) but **not forwarded to the DO**.
///   Phase 2 wires storage of `agent:{identity_b64}:metadata` and
///   the Worker stops dropping the field. The contract here matches
///   §3.3 verbatim; behavioural fidelity is deferred.
#[derive(Debug, Deserialize, Serialize)]
pub struct PublicRegisterRequest {
    pub context_id: String,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Public response body for `POST /v1/teams/{team_id}/wakes`.
///
/// Wire shape per §3.3:
///
/// ```json
/// {
///   "wake_id": "01J5...",
///   "response": "base64(opaque ciphertext)",
///   "completed_at": "2026-05-13T20:00:00Z"
/// }
/// ```
///
/// Translation from internal [`DispatchResponse`]:
/// - `wake_id` → `wake_id` (Crockford-base32 ULID, unchanged)
/// - `response_payload_b64` → `response`
/// - `completed_at: u64` (unix ms) → `completed_at: String`
///   (ISO-8601 second-precision UTC)
/// - `responding_identity_b64` is internal-only; dropped from public.
#[derive(Debug, Deserialize, Serialize)]
pub struct PublicDispatchResponse {
    pub wake_id: String,
    pub response: String,
    pub completed_at: String,
}

/// Public response body for `GET /v1/teams/{team_id}/agents/{identity}/inbox`.
///
/// Wire shape per §3.3:
///
/// ```json
/// {
///   "wakes": [ ... ],
///   "more_available": false
/// }
/// ```
///
/// Translation from internal [`ReadInboxResponse`]: per-entry
/// [`WakeSummary`] → [`PublicWakeSummary`]; `more_available` flows
/// through.
#[derive(Debug, Deserialize, Serialize)]
pub struct PublicReadInboxResponse {
    pub wakes: Vec<PublicWakeSummary>,
    pub more_available: bool,
}

/// Per-wake summary in the public inbox response.
///
/// Wire shape per §3.3:
///
/// ```json
/// {
///   "wake_id": "01J5...",
///   "caller_identity": "base64(...)",
///   "context_id": "task-routing",
///   "payload": "base64(...)",
///   "expires_at": "2026-05-13T20:01:00Z"
/// }
/// ```
///
/// Translation from internal [`WakeSummary`]:
/// - `caller_identity_b64` → `caller_identity`
/// - `payload_b64` → `payload`
/// - `expires_at_ms: u64` → `expires_at: String` (ISO-8601 UTC)
#[derive(Debug, Deserialize, Serialize)]
pub struct PublicWakeSummary {
    pub wake_id: String,
    pub caller_identity: String,
    pub context_id: String,
    pub payload: String,
    pub expires_at: String,
}

/// Public success response body for
/// `POST /v1/teams/{team_id}/agents/{identity}/register`.
///
/// Wire shape per §3.3: `{ "registered": true, "context_id": "task-routing" }`.
#[derive(Debug, Deserialize, Serialize)]
pub struct PublicRegisterResponse {
    pub registered: bool,
    pub context_id: String,
}

/// Public success response body for
/// `POST /v1/teams/{team_id}/wakes/{wake_id}/complete`.
///
/// Wire shape per §3.3: `{ "completed": true, "wake_id": "01J5..." }`.
#[derive(Debug, Deserialize, Serialize)]
pub struct PublicCompleteResponse {
    pub completed: bool,
    pub wake_id: String,
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
        // F.1 refinement: internal DispatchResponse carries wake_id +
        // completed_at so the Worker layer can build the public
        // response shape per §3.3 (`wake_id`, `response`, `completed_at`).
        let resp = DispatchResponse {
            wake_id: WakeId::new(),
            responding_identity_b64: "BBBB".to_string(),
            response_payload_b64: "AQID".to_string(),
            completed_at: 1_700_000_000_000,
        };
        let json = serde_json::to_value(&resp).expect("serialize");
        assert!(json["wake_id"].is_string());
        assert_eq!(json["responding_identity_b64"], "BBBB");
        assert_eq!(json["response_payload_b64"], "AQID");
        assert_eq!(json["completed_at"], 1_700_000_000_000_u64);
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
        // F.1 expansion: internal WakeSummary carries expires_at_ms so
        // the Worker can format ISO-8601 `expires_at` in
        // PublicWakeSummary.
        let resp = ReadInboxResponse {
            wakes: vec![WakeSummary {
                wake_id: "01HZZZZZZZZZZZZZZZZZZZZZZZ".to_string(),
                caller_identity_b64: "AAAA".to_string(),
                context_id: "ctx".to_string(),
                payload_b64: "AQID".to_string(),
                expires_at_ms: 1_700_000_030_000,
            }],
            more_available: true,
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        let decoded: ReadInboxResponse = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.wakes.len(), 1);
        assert!(decoded.more_available);
        assert_eq!(decoded.wakes[0].wake_id, "01HZZZZZZZZZZZZZZZZZZZZZZZ");
        assert_eq!(decoded.wakes[0].expires_at_ms, 1_700_000_030_000);
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

    // ─── Public* wire shapes (HTTP API surface sub-PR F.1 expansion) ─────

    #[test]
    fn public_dispatch_request_round_trip() {
        // §3.3 verbatim: no `_b64` suffixes, `timeout_seconds` not `timeout_ms`.
        let body = r#"{
            "target_identity": "BBBB",
            "context_id": "task-routing",
            "payload": "AQID",
            "timeout_seconds": 30
        }"#;
        let req: PublicDispatchRequest = serde_json::from_str(body).expect("deserialize");
        assert_eq!(req.target_identity, "BBBB");
        assert_eq!(req.context_id, "task-routing");
        assert_eq!(req.payload, "AQID");
        assert_eq!(req.timeout_seconds, 30);
    }

    #[test]
    fn public_complete_request_round_trip() {
        // §3.3 verbatim: single `response` field (Worker injects wake_id
        // from URL path + by_identity from authenticated bearer).
        let body = r#"{ "response": "BAUG" }"#;
        let req: PublicCompleteRequest = serde_json::from_str(body).expect("deserialize");
        assert_eq!(req.response, "BAUG");
    }

    #[test]
    fn public_register_request_round_trip_with_metadata() {
        // §3.3 verbatim: { context_id, metadata? }. Metadata is opaque
        // JSON that the public deserializer accepts but the Worker
        // currently drops (MVP scope boundary; Phase 2 wires storage).
        let body = r#"{
            "context_id": "task-routing",
            "metadata": { "role": "engineer" }
        }"#;
        let req: PublicRegisterRequest = serde_json::from_str(body).expect("deserialize");
        assert_eq!(req.context_id, "task-routing");
        assert!(req.metadata.is_some());
        assert_eq!(req.metadata.as_ref().unwrap()["role"], "engineer");
    }

    #[test]
    fn public_register_request_round_trip_without_metadata() {
        // metadata is optional per §3.3 (omitted in the basic example).
        let body = r#"{ "context_id": "task-routing" }"#;
        let req: PublicRegisterRequest = serde_json::from_str(body).expect("deserialize");
        assert_eq!(req.context_id, "task-routing");
        assert!(req.metadata.is_none());
    }

    #[test]
    fn public_dispatch_response_serializes() {
        // §3.3 verbatim:
        // { "wake_id": "...", "response": "...", "completed_at": "..." }
        // Note `response` (not `response_payload_b64`) and ISO-8601
        // string `completed_at`.
        let resp = PublicDispatchResponse {
            wake_id: "01J5XXXXXXXXXXXXXXXXXXXXXX".to_string(),
            response: "AQID".to_string(),
            completed_at: "2026-05-13T20:00:00Z".to_string(),
        };
        let json = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json["wake_id"], "01J5XXXXXXXXXXXXXXXXXXXXXX");
        assert_eq!(json["response"], "AQID");
        assert_eq!(json["completed_at"], "2026-05-13T20:00:00Z");
        // Strictly: response_payload_b64 must not appear.
        assert!(json.get("response_payload_b64").is_none());
        assert!(json.get("responding_identity_b64").is_none());
    }

    #[test]
    fn public_read_inbox_response_serializes() {
        // §3.3 verbatim: per-wake entry uses caller_identity / payload /
        // expires_at (no `_b64` suffixes; ISO-8601 expires_at).
        let resp = PublicReadInboxResponse {
            wakes: vec![PublicWakeSummary {
                wake_id: "01J5XXXXXXXXXXXXXXXXXXXXXX".to_string(),
                caller_identity: "AAAA".to_string(),
                context_id: "task-routing".to_string(),
                payload: "AQID".to_string(),
                expires_at: "2026-05-13T20:01:00Z".to_string(),
            }],
            more_available: false,
        };
        let json = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json["wakes"][0]["caller_identity"], "AAAA");
        assert_eq!(json["wakes"][0]["payload"], "AQID");
        assert_eq!(json["wakes"][0]["expires_at"], "2026-05-13T20:01:00Z");
        assert_eq!(json["more_available"], false);
        // Strictly: internal `_b64` fields must not appear.
        assert!(json["wakes"][0].get("caller_identity_b64").is_none());
        assert!(json["wakes"][0].get("payload_b64").is_none());
        assert!(json["wakes"][0].get("expires_at_ms").is_none());
    }

    #[test]
    fn public_register_response_serializes() {
        // §3.3 verbatim: { "registered": true, "context_id": "..." }.
        let resp = PublicRegisterResponse {
            registered: true,
            context_id: "task-routing".to_string(),
        };
        let json = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json["registered"], true);
        assert_eq!(json["context_id"], "task-routing");
    }

    #[test]
    fn public_complete_response_serializes() {
        // §3.3 verbatim: { "completed": true, "wake_id": "..." }.
        let resp = PublicCompleteResponse {
            completed: true,
            wake_id: "01J5XXXXXXXXXXXXXXXXXXXXXX".to_string(),
        };
        let json = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json["completed"], true);
        assert_eq!(json["wake_id"], "01J5XXXXXXXXXXXXXXXXXXXXXX");
    }
}
