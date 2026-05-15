//! `WakeRouter` trait implementation for `TallyTeamDO`.
//!
//! Per Phase 0 §3.1-§3.5 (`register_handler`, `unregister_handler`) and
//! the dispatch sub-PR Phase 0 Decision 10 (`dispatch`). The trait's
//! `dispatch` method remains `unimplemented!()` because the inherent
//! method `TallyTeamDO::dispatch_with_caller` (a `pub(crate)` item) is
//! the operational implementation — see the doc-comment on `dispatch`
//! below for the trait-vs-persistence-driven-caller framing.
//!
//! **worker-rs 0.8.3 upgrade (impedance with stoa's `&mut self` trait
//! signatures):** Cloudflare's `DurableObject::fetch(&self, ...)` is
//! now `&self`, so `handle_register` / `handle_unregister` (the HTTP
//! handlers) only have `&self` available — they cannot invoke
//! `WakeRouter::register_handler(self, ...)` which requires
//! `&mut self`. Resolution mirrors the `dispatch_with_caller` pattern
//! (Phase 0 Decision 10): the operational implementation lives in
//! `pub(crate)` `&self` inherent methods on `TallyTeamDO`
//! (`register_handler_inherent` / `unregister_handler_inherent`); the
//! trait impl continues to satisfy the stoa contract but is a
//! loud-failure stub because no `&mut TallyTeamDO` is reachable from
//! any tally call site (the `#[durable_object]` macro owns the
//! instance and dispatches only through `&self`). Forward-compatibility:
//! if stoa relaxes the trait to `&self`, the stubs collapse into thin
//! forwarders.

use std::collections::BTreeSet;
use std::time::Duration;

use async_trait::async_trait;

use stoa::types::Identity;
use stoa::wake_router::{WakePayload, WakeResponse, WakeRouter};
use stoa::{StoaError, WakeError};

use crate::durable_object::TallyTeamDO;

#[async_trait(?Send)]
impl WakeRouter for TallyTeamDO {
    /// Register eligibility — C-A-1 loud-failure stub.
    ///
    /// The operational implementation is `TallyTeamDO`'s inherent
    /// `register_handler_inherent` (`&self`). This trait stub exists
    /// only to satisfy the `WakeRouter` contract; it is not reachable
    /// from production because no call site holds an
    /// `&mut TallyTeamDO` (Cloudflare's runtime owns the instance and
    /// invokes everything through `&self` methods on the trait).
    async fn register_handler(
        &mut self,
        _identity: &Identity,
        _context: &[u8],
    ) -> Result<(), StoaError> {
        unimplemented!(
            "TallyTeamDO::WakeRouter::register_handler: use the &self inherent \
             register_handler_inherent (&mut self trait signature is unreachable \
             after the worker-rs 0.8.3 upgrade — DurableObject::fetch is &self)"
        )
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

    /// Unregister eligibility — C-A-1 loud-failure stub. See
    /// [`Self::register_handler`] for the framing.
    async fn unregister_handler(
        &mut self,
        _identity: &Identity,
        _context: &[u8],
    ) -> Result<(), StoaError> {
        unimplemented!(
            "TallyTeamDO::WakeRouter::unregister_handler: use the &self inherent \
             unregister_handler_inherent (&mut self trait signature is unreachable \
             after the worker-rs 0.8.3 upgrade — DurableObject::fetch is &self)"
        )
    }
}

impl TallyTeamDO {
    /// Inherent register-handler implementation per Phase 0 §3.1 —
    /// operational counterpart to the loud-failure [`WakeRouter::register_handler`]
    /// trait stub.
    ///
    /// Called from [`crate::durable_object::TallyTeamDO::handle_register`]
    /// via inherent dispatch. `&self` (not `&mut self`) — `DurableObject`
    /// trait methods are `&self` as of worker-rs 0.6.0.
    pub(crate) async fn register_handler_inherent(
        &self,
        identity: &Identity,
        context: &[u8],
    ) -> Result<(), StoaError> {
        let context_str = std::str::from_utf8(context).map_err(|_| {
            StoaError::Wake(WakeError::Other("context must be valid UTF-8".to_string()))
        })?;

        let key = format!("agent:{}:handlers", identity.to_url_safe_b64());
        // worker 0.7+ `storage.get` Option return: missing row → start
        // with an empty set (first-handler insert path).
        let mut set: BTreeSet<String> = self
            .state
            .storage()
            .get(&key)
            .await
            .unwrap_or_default()
            .unwrap_or_default();
        set.insert(context_str.to_string());
        self.state.storage().put(&key, &set).await.map_err(|e| {
            StoaError::Wake(WakeError::Other(format!("storage write failed: {}", e)))
        })?;
        Ok(())
    }

    /// Inherent unregister-handler implementation per Phase 0 §3.2
    /// (delete-on-empty) — operational counterpart to the loud-failure
    /// [`WakeRouter::unregister_handler`] trait stub.
    pub(crate) async fn unregister_handler_inherent(
        &self,
        identity: &Identity,
        context: &[u8],
    ) -> Result<(), StoaError> {
        let context_str = std::str::from_utf8(context).map_err(|_| {
            StoaError::Wake(WakeError::Other("context must be valid UTF-8".to_string()))
        })?;

        let key = format!("agent:{}:handlers", identity.to_url_safe_b64());
        // worker 0.7+ tri-state:
        //   Ok(Some(set)) — handlers exist; proceed to remove + delete-on-empty
        //   Ok(None) — no handlers registered; idempotent no-op (matches the
        //     prior Err(_) early-return behavior, now disambiguated)
        //   Err(_) — genuine storage failure; surface as a real error
        let mut set: BTreeSet<String> =
            match self.state.storage().get::<BTreeSet<String>>(&key).await {
                Ok(Some(s)) => s,
                Ok(None) => return Ok(()),
                Err(e) => {
                    return Err(StoaError::Wake(WakeError::Other(format!(
                        "storage read handlers failed: {}",
                        e
                    ))));
                }
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
