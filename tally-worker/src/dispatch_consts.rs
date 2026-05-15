//! Dispatch-related constants per Phase 0 design notes (dispatch sub-PR).
//!
//! Source: `docs/specs/dispatch-sub-pr-phase-0.md` § Dependencies + constants.
//! Each constant is referenced by name in the design notes; values are not
//! independently re-derived here, they are mirrors of the locked spec.

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

/// Safety buffer on dispatch's [`tokio::time::timeout`] wrapper (Decision 9).
///
/// The alarm-based timeout is the primary timeout mechanism; this is the
/// defensive backstop bounding the in-memory await if the alarm path
/// somehow fails to resolve the wake (DO restart, implementation bug, etc.).
pub const SAFETY_BUFFER: Duration = Duration::from_secs(5);

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
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_buffer_nonzero() {
        // `Duration::is_zero` is non-const; assert at runtime.
        assert!(!SAFETY_BUFFER.is_zero());
    }
}
