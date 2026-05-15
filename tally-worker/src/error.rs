//! Tally-specific error type per HTTP API surface sub-PR Phase 0 Decision 4.
//!
//! `TallyError` wraps [`stoa::StoaError`] for implementation-specific cases
//! that don't belong in stoa's dispatch-scoped [`stoa::WakeError`] surface.
//!
//! ## Why this type exists
//!
//! `stoa::WakeError` is doc-commented as "Errors from
//! `WakeRouter::dispatch`" — its variants ([`stoa::WakeError::HandlerNotFound`],
//! [`stoa::WakeError::DispatchRefused`], etc.) are dispatch protocol concerns.
//! `TallyTeamDO::complete_wake` (a `pub(crate)` inherent method) is a tally
//! implementation detail, not part of stoa's `WakeRouter` trait surface.
//! The §9.1 dispatch sub-PR fudged `WakeError::HandlerNotFound` for "wake
//! row not found" and `WakeError::DispatchRefused` for "wake not Pending" /
//! "by_identity mismatch" — three cases that aren't dispatch protocol
//! failures.
//!
//! Adding variants to stoa for tally-specific cases would erode the
//! protocol-vs-implementation boundary (stoa would become "all wake
//! errors" rather than "dispatch errors"). Wrapping instead keeps stoa
//! dispatch-scoped; tally absorbs implementation-specific cases here.
//!
//! ## Variants
//!
//! - [`TallyError::Stoa`] — pass-through for stoa's dispatch-scoped
//!   errors. `#[from]` makes `?` work naturally where stoa errors flow.
//! - [`TallyError::WakeNotFound`] — wake row not found in storage.
//!   Distinct from `WakeError::HandlerNotFound` (handler eligibility).
//! - [`TallyError::AlreadyTerminal`] — complete_wake called on a wake
//!   already in terminal state. Caller is attempting duplicate
//!   completion.
//! - [`TallyError::IdentityMismatch`] — complete_wake's `by_identity`
//!   doesn't match wake's `target_identity`.
//!
//! `dispatch_with_caller`'s return type stays `Result<WakeResponse,
//! StoaError>` — all its errors are protocol-level.

use stoa::StoaError;

/// Tally implementation-specific errors covering cases not modeled by
/// stoa's dispatch-scoped [`stoa::WakeError`].
///
/// See the [module-level documentation][self] for the reasoning.
#[derive(Debug, thiserror::Error)]
pub enum TallyError {
    /// Pass-through for stoa's dispatch-scoped errors.
    ///
    /// `#[from]` lets `?` lift `StoaError` into `TallyError` at the
    /// call sites that mix protocol and implementation failures.
    #[error(transparent)]
    Stoa(#[from] StoaError),

    /// Wake row not found in storage. Distinct from
    /// [`stoa::WakeError::HandlerNotFound`] which means handler
    /// eligibility (a dispatch-scoped concern).
    ///
    /// Maps to HTTP 404 per Phase 0 §3.1.
    #[error("wake not found")]
    WakeNotFound,

    /// `TallyTeamDO::complete_wake` called on a wake already in
    /// terminal state. Caller is attempting duplicate completion
    /// (either replaying or racing the alarm-fire path).
    ///
    /// Maps to HTTP 410 per Phase 0 §3.1.
    #[error("wake already in terminal state")]
    AlreadyTerminal,

    /// `TallyTeamDO::complete_wake`'s `by_identity` argument doesn't
    /// match the wake's `target_identity` field.
    ///
    /// MVP enforces that only the wake's target can complete the wake.
    /// Maps to HTTP 403 per Phase 0 §3.1.
    #[error("identity does not match wake target")]
    IdentityMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use stoa::WakeError;

    #[test]
    fn from_stoa_error_lifts_via_question_mark() {
        // Confirms #[from] gives StoaError → TallyError lifting.
        fn returns_stoa() -> Result<(), StoaError> {
            Err(StoaError::Wake(WakeError::InvalidTimeout))
        }
        fn lifts_to_tally() -> Result<(), TallyError> {
            returns_stoa()?;
            Ok(())
        }
        match lifts_to_tally() {
            Err(TallyError::Stoa(StoaError::Wake(WakeError::InvalidTimeout))) => {}
            other => panic!("expected Stoa(Wake(InvalidTimeout)), got {:?}", other),
        }
    }

    #[test]
    fn tally_specific_variants_display_as_human_text() {
        assert_eq!(TallyError::WakeNotFound.to_string(), "wake not found");
        assert_eq!(
            TallyError::AlreadyTerminal.to_string(),
            "wake already in terminal state"
        );
        assert_eq!(
            TallyError::IdentityMismatch.to_string(),
            "identity does not match wake target"
        );
    }

    #[test]
    fn stoa_pass_through_uses_transparent_display() {
        // #[error(transparent)] should pass through the inner display.
        let inner = StoaError::Wake(WakeError::InvalidTimeout);
        let inner_msg = inner.to_string();
        let wrapped = TallyError::Stoa(inner);
        assert_eq!(wrapped.to_string(), inner_msg);
    }
}
