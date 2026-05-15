//! `WakeRouter` trait implementation for `TallyTeamDO`.
//!
//! Per Phase 0 §3.1-§3.5 (`register_handler`, `unregister_handler`) and
//! the dispatch sub-PR Phase 0 Decision 10 (`dispatch`). The trait's
//! `dispatch` method remains `unimplemented!()` because the inherent
//! method `TallyTeamDO::dispatch_with_caller` (a `pub(crate)` item) is
//! the operational implementation — see the doc-comment on `dispatch`
//! below for the trait-vs-persistence-driven-caller framing.

use std::collections::BTreeSet;
use std::time::Duration;

use async_trait::async_trait;

use stoa::types::Identity;
use stoa::wake_router::{WakePayload, WakeResponse, WakeRouter};
use stoa::{StoaError, WakeError};

use crate::durable_object::TallyTeamDO;

#[async_trait(?Send)]
impl WakeRouter for TallyTeamDO {
    /// Register eligibility per Phase 0 §3.1.
    async fn register_handler(
        &mut self,
        identity: &Identity,
        context: &[u8],
    ) -> Result<(), StoaError> {
        let context_str = std::str::from_utf8(context).map_err(|_| {
            StoaError::Wake(WakeError::Other("context must be valid UTF-8".to_string()))
        })?;

        let key = format!("agent:{}:handlers", identity.to_url_safe_b64());
        let mut set: BTreeSet<String> = self.state.storage().get(&key).await.unwrap_or_default();
        set.insert(context_str.to_string());
        self.state.storage().put(&key, &set).await.map_err(|e| {
            StoaError::Wake(WakeError::Other(format!("storage write failed: {}", e)))
        })?;
        Ok(())
    }

    /// Dispatch a wake — C-A-1 loud-failure stub per dispatch sub-PR
    /// Phase 0 Decision 10.
    ///
    /// `TallyTeamDO`'s persistence layer (per Decision 8's `WakeRecord`
    /// schema) requires the caller's identity to record `WakeRecord.caller_identity`.
    /// `stoa::wake_router::WakeRouter::dispatch`'s trait signature does
    /// NOT carry a caller param. The impedance is resolved at the
    /// implementation surface: the inherent method
    /// `TallyTeamDO::dispatch_with_caller` (a `pub(crate)` item) carries
    /// the caller and is what `handle_dispatch` (the HTTP handler) routes
    /// to; the trait impl is unreachable in production.
    ///
    /// C-A-1 (loud-failure-by-design): calling this trait method
    /// `unimplemented!()`s. The semantic of "this trait method should
    /// not be called on this implementation" is better expressed as a
    /// programming-error panic than a runtime error — calling it
    /// indicates an architectural mistake in the caller, not a
    /// runtime failure. Cloudflare's runtime translates the panic to
    /// HTTP 500.
    ///
    /// Forward-compatibility: if `stoa::wake_router::WakeRouter::dispatch`
    /// grows a caller param in a future protocol revision, this impl
    /// shifts from `unimplemented!()` to a thin wrapper that forwards
    /// to `TallyTeamDO::dispatch_with_caller`.
    async fn dispatch(
        &self,
        _target: &Identity,
        _context: &[u8],
        _payload: WakePayload,
        _timeout: Duration,
    ) -> Result<WakeResponse, StoaError> {
        unimplemented!(
            "TallyTeamDO::dispatch: use dispatch_with_caller (caller identity required for persistence)"
        )
    }

    /// Unregister eligibility with delete-on-empty per Phase 0 §3.2.
    async fn unregister_handler(
        &mut self,
        identity: &Identity,
        context: &[u8],
    ) -> Result<(), StoaError> {
        let context_str = std::str::from_utf8(context).map_err(|_| {
            StoaError::Wake(WakeError::Other("context must be valid UTF-8".to_string()))
        })?;

        let key = format!("agent:{}:handlers", identity.to_url_safe_b64());
        let Ok(mut set) = self.state.storage().get::<BTreeSet<String>>(&key).await else {
            return Ok(());
        };
        set.remove(context_str);

        if set.is_empty() {
            self.state.storage().delete(&key).await.map_err(|e| {
                StoaError::Wake(WakeError::Other(format!("storage delete failed: {}", e)))
            })?;
        } else {
            self.state.storage().put(&key, &set).await.map_err(|e| {
                StoaError::Wake(WakeError::Other(format!("storage write failed: {}", e)))
            })?;
        }
        Ok(())
    }
}
