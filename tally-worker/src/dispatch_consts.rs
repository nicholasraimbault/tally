//! Dispatch-related constants per Phase 0 design notes (dispatch sub-PR
//! + HTTP API surface sub-PR).
//!
//! Source: `docs/specs/dispatch-sub-pr-phase-0.md` § Dependencies + constants
//! and `docs/specs/http-api-surface-sub-pr-phase-0.md` § Pagination +
//! long-poll. Each constant is referenced by name in the design notes;
//! values are not independently re-derived here, they are mirrors of the
//! locked spec.

use std::time::Duration;

/// Minimum timeout per Phase 0 §3.7.
pub const MIN_TIMEOUT_MS: u32 = 1_000;

/// Maximum timeout per Phase 0 §3.7. Cloudflare DO supports unlimited
/// wall time for connected HTTP clients, so 300s is bounded by Phase 0
/// spec rather than platform.
pub const MAX_TIMEOUT_MS: u32 = 300_000;

/// Per Phase 0 §3.7 payload size constraint.
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024;

/// Per Phase 0 §3.7 response size constraint.
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024;

/// Per-target inbox cap per Phase 0 §3.2. Overflow evicts the head (FIFO oldest).
pub const INBOX_LIMIT: usize = 1_000;

/// Safety buffer on dispatch's `worker::Delay` safety wrapper (Decision 9).
///
/// The alarm-based timeout is the primary timeout mechanism; this is the
/// defensive backstop bounding the in-memory await if the alarm path
/// somehow fails to resolve the wake (DO restart, implementation bug, etc.).
/// `tokio::time::timeout` is unavailable on `wasm32-unknown-unknown` (no
/// timer driver); the wrapper races the oneshot Receiver against
/// [`worker::Delay`] via `futures::future::select` instead.
pub const SAFETY_BUFFER: Duration = Duration::from_secs(5);

/// Default value for the `wait_seconds` query parameter on
/// `GET /v1/teams/{team_id}/agents/{identity}/inbox`.
///
/// Per HTTP API surface sub-PR Phase 0 §3.8: when `wait_seconds = 0`
/// (the default) the inbox read returns immediately with whatever is
/// currently present. Long-poll behaviour is opt-in via a non-zero
/// `wait_seconds` value.
pub const DEFAULT_WAIT_SECONDS: u32 = 0;

/// Maximum value accepted for the `wait_seconds` query parameter.
///
/// Per HTTP API surface sub-PR Phase 0 §3.8: 30 seconds is the upper
/// bound on long-poll duration. Clients passing larger values are
/// clamped to this maximum (rather than rejected) per pagination spec.
pub const MAX_WAIT_SECONDS: u32 = 30;

/// Default page size for `GET /v1/teams/{team_id}/agents/{identity}/inbox`.
///
/// Per HTTP API surface sub-PR Phase 0 §3.8: the inbox read returns at
/// most this many [`crate::rpc::WakeSummary`] entries when no `limit`
/// query parameter is supplied.
pub const DEFAULT_LIMIT: usize = 10;

/// Maximum page size for `GET /v1/teams/{team_id}/agents/{identity}/inbox`.
///
/// Per HTTP API surface sub-PR Phase 0 §3.8: clients requesting more
/// than this many entries are clamped (rather than rejected); the
/// `more_available: bool` field signals whether additional entries
/// exist beyond the returned page.
pub const MAX_LIMIT: usize = 100;

// Compile-time invariant assertions per Phase 0 in-scope unit-test surface
// ("constant validation"). `const { assert!(..) }` evaluates at compile
// time; a const-bind test is a compile failure rather than a runtime
// failure if a constant is changed in a way that violates the invariant.
const _: () = {
    assert!(MIN_TIMEOUT_MS > 0);
    assert!(MIN_TIMEOUT_MS < MAX_TIMEOUT_MS);
    assert!(MAX_PAYLOAD_BYTES > 0);
    assert!(MAX_RESPONSE_BYTES > 0);
    assert!(INBOX_LIMIT > 0);
    // HTTP API surface sub-PR additions:
    assert!(MAX_WAIT_SECONDS > DEFAULT_WAIT_SECONDS);
    assert!(MAX_LIMIT >= DEFAULT_LIMIT);
};

/// Clamp a `limit` query parameter per HTTP API surface sub-PR Phase 0
/// §3.8 pagination spec.
///
/// `None` → [`DEFAULT_LIMIT`]; `Some(n)` → `n.clamp(1, MAX_LIMIT)`.
/// Factored as a `pub(crate)` helper so the clamp logic is unit-testable
/// without a wasm runtime.
pub(crate) fn clamp_limit(raw: Option<u32>) -> usize {
    match raw {
        None => DEFAULT_LIMIT,
        Some(n) => (n as usize).clamp(1, MAX_LIMIT),
    }
}

/// Clamp a `wait_seconds` query parameter per HTTP API surface sub-PR
/// Phase 0 §3.8 long-poll spec.
///
/// `None` → [`DEFAULT_WAIT_SECONDS`]; `Some(n)` → `n.min(MAX_WAIT_SECONDS)`.
/// Factored as a `pub(crate)` helper so the clamp logic is unit-testable
/// without a wasm runtime.
pub(crate) fn clamp_wait_seconds(raw: Option<u32>) -> u32 {
    match raw {
        None => DEFAULT_WAIT_SECONDS,
        Some(n) => n.min(MAX_WAIT_SECONDS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_buffer_nonzero() {
        // `Duration::is_zero` is non-const; assert at runtime.
        assert!(!SAFETY_BUFFER.is_zero());
    }

    #[test]
    fn wait_seconds_constants_align() {
        // DEFAULT is the minimum acceptable; MAX is the strict upper bound.
        // The MAX > DEFAULT invariant is enforced at compile time via the
        // const-block assertion above; this runtime test just pins the
        // public values so a stealth tweak to either constant fails CI.
        assert_eq!(DEFAULT_WAIT_SECONDS, 0);
        assert_eq!(MAX_WAIT_SECONDS, 30);
    }

    #[test]
    fn limit_constants_align() {
        // DEFAULT is the suggested page size; MAX is the strict upper bound.
        // The MAX >= DEFAULT invariant is enforced at compile time via the
        // const-block assertion above; this runtime test just pins the
        // public values so a stealth tweak to either constant fails CI.
        assert_eq!(DEFAULT_LIMIT, 10);
        assert_eq!(MAX_LIMIT, 100);
    }

    // ─── clamp_limit boundary cases ─────────────────────────────────

    #[test]
    fn clamp_limit_none_returns_default() {
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
    }

    #[test]
    fn clamp_limit_zero_clamped_to_one() {
        // §3.8 spec: limit defaults to 10 but the lower bound is 1
        // (returning 0 entries is meaningless). 0 → 1.
        assert_eq!(clamp_limit(Some(0)), 1);
    }

    #[test]
    fn clamp_limit_in_range_passes_through() {
        assert_eq!(clamp_limit(Some(1)), 1);
        assert_eq!(clamp_limit(Some(DEFAULT_LIMIT as u32)), DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(50)), 50);
        assert_eq!(clamp_limit(Some(MAX_LIMIT as u32)), MAX_LIMIT);
    }

    #[test]
    fn clamp_limit_above_max_clamped_to_max() {
        assert_eq!(clamp_limit(Some((MAX_LIMIT as u32) + 1)), MAX_LIMIT);
        assert_eq!(clamp_limit(Some(1_000_000)), MAX_LIMIT);
        assert_eq!(clamp_limit(Some(u32::MAX)), MAX_LIMIT);
    }

    // ─── clamp_wait_seconds boundary cases ─────────────────────────

    #[test]
    fn clamp_wait_seconds_none_returns_default() {
        assert_eq!(clamp_wait_seconds(None), DEFAULT_WAIT_SECONDS);
    }

    #[test]
    fn clamp_wait_seconds_zero_passes_through() {
        // wait_seconds = 0 is the spec-defined default (no long-poll).
        // Distinct from limit, which can't be 0.
        assert_eq!(clamp_wait_seconds(Some(0)), 0);
    }

    #[test]
    fn clamp_wait_seconds_in_range_passes_through() {
        assert_eq!(clamp_wait_seconds(Some(1)), 1);
        assert_eq!(clamp_wait_seconds(Some(15)), 15);
        assert_eq!(clamp_wait_seconds(Some(MAX_WAIT_SECONDS)), MAX_WAIT_SECONDS);
    }

    #[test]
    fn clamp_wait_seconds_above_max_clamped_to_max() {
        assert_eq!(
            clamp_wait_seconds(Some(MAX_WAIT_SECONDS + 1)),
            MAX_WAIT_SECONDS
        );
        assert_eq!(clamp_wait_seconds(Some(3600)), MAX_WAIT_SECONDS);
        assert_eq!(clamp_wait_seconds(Some(u32::MAX)), MAX_WAIT_SECONDS);
    }
}
