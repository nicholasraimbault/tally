//! Wake-routing domain types per Phase 0 design notes (dispatch sub-PR).
//!
//! Three types:
//!
//! - [`WakeId`] — ULID-based wake identifier (Decision 3). Private inner
//!   field enforces that direct [`ulid::Ulid`] manipulation goes through
//!   the accessor methods.
//! - [`WakeState`] — terminal-terminal state machine (Decision 1).
//!   `Pending → Completed` via `complete_wake`; `Pending → TimedOut` via
//!   alarm-fire; terminal states do not transition further.
//! - [`WakeRecord`] — persisted per-wake row (Decision 8). 9 fields;
//!   `payload` and `response_payload` use `#[serde(with = "serde_bytes")]`
//!   for binary serialization at the storage layer.
//!
//! Storage key: `wake:{wake_id}` where `{wake_id}` is the 26-char
//! Crockford-base32 form of the underlying [`ulid::Ulid`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Wake identifier — wraps a 128-bit ULID.
///
/// Per Phase 0 Decision 3, the inner [`ulid::Ulid`] is private; callers
/// must use the accessor methods. Constructed via [`WakeId::new`] for
/// fresh dispatches; parsed via [`WakeId::from_str`] for wakes
/// referenced from inbox entries or HTTP request bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WakeId(ulid::Ulid);

impl WakeId {
    /// Generate a fresh wake identifier from the current clock.
    ///
    /// ULID's timestamp prefix uses unix milliseconds; on wasm32 the
    /// clock source is supplied by `web-time` (transitive dep of
    /// `ulid`), keeping `new()` valid inside Cloudflare Workers.
    pub fn new() -> Self {
        Self(ulid::Ulid::new())
    }

    /// Borrow the underlying [`ulid::Ulid`].
    ///
    /// Provided per Phase 0 Decision 3 to give callers escape-hatch
    /// access for ULID-specific operations (e.g., timestamp extraction
    /// via `as_ulid().timestamp_ms()`) without exposing the inner
    /// field directly.
    pub fn as_ulid(&self) -> ulid::Ulid {
        self.0
    }

    /// Render as the 26-char Crockford-base32 form.
    ///
    /// Note: this is an inherent method (per Phase 0 Decision 3),
    /// distinct from any blanket [`ToString`] impl. It mirrors the
    /// underlying [`ulid::Ulid::to_string`].
    #[allow(clippy::inherent_to_string_shadow_display)]
    pub fn to_string(&self) -> String {
        self.0.to_string()
    }
}

impl FromStr for WakeId {
    type Err = ulid::DecodeError;

    /// Parse a 26-char Crockford-base32 ULID string into a [`WakeId`].
    ///
    /// Returns the underlying [`ulid::DecodeError`] on malformed input.
    ///
    /// Phase 0 Decision 3 specifies `WakeId::from_str(s)` as the
    /// parse-entry surface. Implemented via [`FromStr`] (rather than as
    /// an inherent method) so the [`WakeId::from_str`] call form remains
    /// available — `<WakeId as FromStr>::from_str` is reachable through
    /// path-call syntax `WakeId::from_str(s)` — while satisfying
    /// `clippy::should_implement_trait`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ulid::Ulid::from_str(s).map(Self)
    }
}

impl Default for WakeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WakeId {
    /// Format as the 26-char Crockford-base32 ULID form.
    ///
    /// Enables `format!("wake:{}", wake_id)` storage-key construction
    /// (Phase 0 §11) and `tracing::warn!(wake_id = %wake_id, ...)`
    /// structured logging (Phase 0 Decision 11) without callers
    /// reaching for the inherent [`WakeId::to_string`] method.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// Wake lifecycle state — terminal-terminal model per Phase 0 Decision 1.
///
/// Three variants only; terminal states ([`WakeState::Completed`],
/// [`WakeState::TimedOut`]) do not transition further. State guards in
/// `complete_wake` and the alarm-fire handler prevent double transitions
/// and make Cloudflare's alarm retry behavior safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeState {
    /// Wake dispatched; not yet completed or timed out.
    Pending,
    /// `complete_wake` transitioned the wake out of `Pending`.
    Completed,
    /// Alarm-fire transitioned the wake out of `Pending` after timeout.
    TimedOut,
}

/// Per-wake persisted row — 9 fields per Phase 0 Decision 8.
///
/// Stored under key `wake:{wake_id}` (Crockford-base32 ULID). Both
/// `payload` and `response_payload` use `#[serde(with = "serde_bytes")]`
/// because `serde_wasm_bindgen`'s default `Vec<u8>` path serializes to
/// a JSON-style integer array (~3x the binary size); `serde_bytes`
/// keeps the storage representation as native bytes.
///
/// Note: `absolute_timeout` is intentionally NOT a stored field per
/// Decision 2; it's computed from `created_at + timeout_ms as u64` at
/// the call sites that need it. The arithmetic is total and
/// deterministic since both source fields are immutable post-dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeRecord {
    /// Unique wake identifier.
    pub wake_id: WakeId,

    /// URL-safe-base64 (no padding) of the target [`stoa::types::Identity`].
    pub target_identity: String,

    /// URL-safe-base64 (no padding) of the caller [`stoa::types::Identity`].
    pub caller_identity: String,

    /// Routing context (UTF-8); opaque to this layer.
    pub context_id: String,

    /// Caller-supplied opaque payload.
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,

    /// Current lifecycle state.
    pub state: WakeState,

    /// Wake creation time (unix milliseconds).
    pub created_at: u64,

    /// Original requested timeout (milliseconds); immutable post-dispatch.
    pub timeout_ms: u32,

    /// Target-supplied response, populated on `Pending → Completed`.
    ///
    /// `None` on `Pending` and `TimedOut`. `Option<Vec<u8>>` carries
    /// the `serde_bytes` annotation so the `Some` branch serializes
    /// the inner bytes binary-style.
    #[serde(with = "serde_bytes", default)]
    pub response_payload: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_id_crockford_base32_round_trip() {
        let original = WakeId::new();
        let s = original.to_string();
        // ULID Crockford-base32 form is exactly 26 chars (128-bit
        // payload at 5 bits/char rounded up).
        assert_eq!(s.len(), 26);
        let parsed = WakeId::from_str(&s).expect("ULID parses back");
        assert_eq!(parsed, original);
    }

    #[test]
    fn wake_id_display_matches_to_string() {
        let id = WakeId::new();
        assert_eq!(format!("{}", id), id.to_string());
    }

    #[test]
    fn wake_id_as_ulid_round_trip() {
        let id = WakeId::new();
        // as_ulid lets us reach Ulid-specific APIs (e.g.,
        // timestamp_ms) while keeping the inner field private.
        let inner = id.as_ulid();
        assert_eq!(WakeId::from_str(&inner.to_string()).unwrap(), id);
    }

    #[test]
    fn wake_state_serde_snake_case() {
        // Confirms #[serde(rename_all = "snake_case")] on WakeState.
        assert_eq!(
            serde_json::to_string(&WakeState::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&WakeState::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&WakeState::TimedOut).unwrap(),
            "\"timed_out\""
        );
    }

    #[test]
    fn wake_record_serde_round_trip() {
        let original = WakeRecord {
            wake_id: WakeId::new(),
            target_identity: "AAAA".to_string(),
            caller_identity: "BBBB".to_string(),
            context_id: "ctx".to_string(),
            payload: vec![1, 2, 3, 4],
            state: WakeState::Pending,
            created_at: 1_700_000_000_000,
            timeout_ms: 30_000,
            response_payload: None,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let decoded: WakeRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.wake_id, original.wake_id);
        assert_eq!(decoded.target_identity, original.target_identity);
        assert_eq!(decoded.caller_identity, original.caller_identity);
        assert_eq!(decoded.context_id, original.context_id);
        assert_eq!(decoded.payload, original.payload);
        assert_eq!(decoded.state, original.state);
        assert_eq!(decoded.created_at, original.created_at);
        assert_eq!(decoded.timeout_ms, original.timeout_ms);
        assert_eq!(decoded.response_payload, original.response_payload);
    }

    #[test]
    fn wake_record_serde_with_response() {
        let original = WakeRecord {
            wake_id: WakeId::new(),
            target_identity: "AAAA".to_string(),
            caller_identity: "BBBB".to_string(),
            context_id: "ctx".to_string(),
            payload: vec![1],
            state: WakeState::Completed,
            created_at: 1_700_000_000_000,
            timeout_ms: 30_000,
            response_payload: Some(vec![42, 43, 44]),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let decoded: WakeRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.response_payload, original.response_payload);
        assert_eq!(decoded.state, WakeState::Completed);
    }
}
