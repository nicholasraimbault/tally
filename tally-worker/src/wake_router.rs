//! `WakeRouter` trait implementation for `TallyTeamDO`.
//!
//! Per Phase 0 §3.1-§3.5. The implementation is bounded to
//! `register_handler` and `unregister_handler` (real work); `dispatch`
//! is `unimplemented!()` per the scope-boundary stub in §3.3.

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

    /// Dispatch a wake — scope-boundary stub per Phase 0 §3.3.
    ///
    /// The full implementation lands in the dispatch sub-PR. Calling
    /// `dispatch` in this PR's deployed state panics; Cloudflare's
    /// Worker runtime translates the panic to HTTP 500.
    async fn dispatch(
        &self,
        _target: &Identity,
        _context: &[u8],
        _payload: WakePayload,
        _timeout: Duration,
    ) -> Result<WakeResponse, StoaError> {
        unimplemented!("TallyTeamDO::dispatch lands in the dispatch sub-PR")
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
