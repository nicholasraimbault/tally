//! `TallyTeamDO` Durable Object — state model per Phase 0 §4 + dispatch
//! sub-PR Phase 0.
//!
//! Per Phase 0 §2.2 the DO holds `state: State` (Cloudflare's single-writer
//! guarantee makes additional locking unnecessary). The dispatch sub-PR
//! adds the `wake_resolvers` in-memory map (Decision 7) that bridges
//! between the dispatch await path (oneshot Receiver) and the resolution
//! paths (`complete_wake` / alarm-fire).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::result::Result as StdResult;
use std::str::FromStr;
use std::time::Duration;

use worker::*;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD;
use base64::Engine as _;
use futures::future::{select, Either};
use js_sys::{Object, Reflect};
use tokio::sync::oneshot;
use wasm_bindgen::JsValue;

use stoa::types::Identity;
use stoa::wake_router::{WakePayload, WakeResponse};
use stoa::{StoaError, WakeError};
use tally_core::TeamMeta;

use crate::dispatch_consts::{
    clamp_limit, clamp_wait_seconds, INBOX_LIMIT, MAX_PAYLOAD_BYTES, MAX_RESPONSE_BYTES,
    MAX_TIMEOUT_MS, MIN_TIMEOUT_MS, SAFETY_BUFFER,
};
use crate::error::TallyError;
use crate::rpc::{
    CompleteRequest, DispatchRequest, DispatchResponse, OkResponse, ReadInboxQuery,
    ReadInboxResponse, RegisterRequest, UnregisterRequest, ValidateApiKeyRequest,
    ValidateApiKeyResponse, WakeSummary,
};
use crate::wake_types::{WakeId, WakeRecord, WakeState};

const TENANCY_PREFIX_MVP: &str = "tally-cli-local";
const TEAM_META_KEY: &str = "team:meta";

/// Storage key for the alarm queue — `BTreeMap<u64, Vec<WakeId>>` keyed
/// by absolute timeout (unix millis).
const ALARM_QUEUE_KEY: &str = "alarm_queue";

/// Resolver send payload type — the `Ok` arm carries the
/// `(response, completed_at)` tuple bridged from `complete_wake`
/// (HTTP API surface sub-PR F.1 expansion). Factored as a type
/// alias to satisfy `clippy::type_complexity` for the
/// [`TallyTeamDO::wake_resolvers`] HashMap value.
type WakeResolverResult = StdResult<(WakeResponse, u64), StoaError>;

/// Build the per-target inbox storage key.
fn inbox_key(identity_b64: &str) -> String {
    format!("agent:{}:inbox", identity_b64)
}

/// Build the per-wake row storage key.
fn wake_key(wake_id: &WakeId) -> String {
    format!("wake:{}", wake_id)
}

#[durable_object]
pub struct TallyTeamDO {
    pub(crate) state: State,
    /// In-memory map from wake_id to the awaiter's oneshot `Sender`.
    ///
    /// Per Phase 0 Decision 7 + Lock 4.6.1. Populated post-storage in
    /// [`TallyTeamDO::dispatch_with_caller`]; drained post-storage in
    /// [`TallyTeamDO::complete_wake`] and in the alarm-fire handler.
    /// Cloudflare DO eviction drops this; storage persists, alarm fires
    /// after rehydration, pending wakes route to TimedOut.
    ///
    /// **F.1 expansion:** the channel payload is the tuple `(WakeResponse,
    /// u64)` carrying the response *and* the unix-millisecond timestamp
    /// at which `complete_wake`'s `put_multiple_raw` committed. The
    /// dispatching awaiter uses this timestamp to construct
    /// [`crate::rpc::DispatchResponse::completed_at`]; the Worker layer
    /// formats it as ISO-8601 for the public response. The error arm
    /// (`StoaError`) carries no timestamp — error responses include
    /// `wake_id` (the dispatch site has it) but not a `completed_at`.
    ///
    /// **`RefCell` rationale (worker-rs 0.6.0 upgrade):** `DurableObject`
    /// trait methods now take `&self` rather than `&mut self`. Interior
    /// mutability is required for the in-memory bookkeeping maps.
    /// `RefCell` (not `Mutex`) because Cloudflare's Durable Objects are
    /// single-threaded — only one event-loop task runs at a time per DO
    /// instance — so the synchronization overhead of `Mutex` would be
    /// pure cost without benefit. **All `borrow_mut()` borrows must be
    /// scoped to drop before any `.await`** to avoid runtime panics from
    /// a borrow held across a yield point.
    pub(crate) wake_resolvers: RefCell<HashMap<WakeId, oneshot::Sender<WakeResolverResult>>>,
    /// In-memory map from target identity to the long-poll waiter's
    /// oneshot `Sender` for inbox-arrival notifications.
    ///
    /// Per HTTP API surface sub-PR Phase 0 Decision 2: single-waiter
    /// per identity. `handle_read_inbox` inserts when `wait_seconds > 0`
    /// and the inbox is empty (subscribe-first ordering); the signal
    /// site in [`TallyTeamDO::dispatch_with_caller`] removes the entry
    /// post-storage and best-effort sends `()` to wake up the waiter.
    /// Cloudflare DO eviction drops this map; waiting clients receive
    /// `RecvError` and gracefully degrade to re-read.
    ///
    /// See [`Self::wake_resolvers`] for the `RefCell` rationale.
    pub(crate) inbox_waiters: RefCell<HashMap<String, oneshot::Sender<()>>>,
}

impl DurableObject for TallyTeamDO {
    fn new(state: State, _env: Env) -> Self {
        Self {
            state,
            wake_resolvers: RefCell::new(HashMap::new()),
            inbox_waiters: RefCell::new(HashMap::new()),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        // Lazy team:meta initialization on first request (Phase 0 §4.3).
        self.ensure_team_meta_initialized().await?;

        let method = req.method();
        let path = req.path();

        match (method, path.as_str()) {
            (Method::Post, "/register") => self.handle_register(&mut req).await,
            (Method::Post, "/unregister") => self.handle_unregister(&mut req).await,
            (Method::Post, "/dispatch") => self.handle_dispatch(&mut req).await,
            (Method::Get, p) if p.starts_with("/inbox/") => {
                // HTTP API surface sub-PR Decision 3: identity is a
                // routing parameter (URL path segment), not a query
                // parameter. The Worker authenticates the caller and
                // forwards the verified identity as `/inbox/{identity_b64}`.
                let identity_b64 = p["/inbox/".len()..].to_string();
                self.handle_read_inbox(&req, identity_b64).await
            }
            (Method::Post, "/complete") => self.handle_complete_wake(&mut req).await,
            (Method::Post, "/validate_api_key") => self.handle_validate_api_key(&mut req).await,
            _ => Response::error("not found", 404),
        }
    }

    /// Alarm-fire handler — Pending→TimedOut transitions for all due wakes.
    ///
    /// Per Phase 0 Decision 5 (operation shape) + Decision 6 (β.1 transition
    /// bundles target inbox writes). Reads alarm_queue; for each due wake_id,
    /// reads its wake row, defensively skipping on missing rows (Lock 2.6.8).
    /// Builds atomic `put_multiple_raw` with N transitioned wake rows + M
    /// distinct-target inboxes + updated alarm_queue, then resolves the
    /// in-memory resolvers with `TimeoutExpired` and reschedules the alarm
    /// to the next-earliest entry (or deletes the alarm if the queue is
    /// empty).
    async fn alarm(&self) -> Result<Response> {
        let now_ms = Date::now().as_millis();

        let mut alarm_queue: BTreeMap<u64, Vec<WakeId>> = self
            .state
            .storage()
            .get(ALARM_QUEUE_KEY)
            .await
            .unwrap_or_default();

        // Partition: due entries (≤ now) vs future entries (> now).
        let due_keys: Vec<u64> = alarm_queue.range(..=now_ms).map(|(k, _)| *k).collect();

        if due_keys.is_empty() {
            // Spurious fire (or already-handled): reschedule to the new
            // earliest, or delete the alarm if queue is empty.
            self.reschedule_alarm(&alarm_queue).await?;
            return Response::ok("no due wakes");
        }

        // Collect due (wake_id, absolute_timeout) pairs and remove them
        // from alarm_queue.
        let mut due_pairs: Vec<(WakeId, u64)> = Vec::new();
        for k in &due_keys {
            if let Some(ids) = alarm_queue.remove(k) {
                for id in ids {
                    due_pairs.push((id, *k));
                }
            }
        }

        // Sequential reads: read each due wake row. Defensive skip on
        // missing rows (Lock 2.6.8).
        struct DueWake {
            wake_id: WakeId,
            record: WakeRecord,
            timeout_ms: u32,
        }
        let mut transitions: Vec<DueWake> = Vec::with_capacity(due_pairs.len());
        for (wake_id, _abs_ms) in &due_pairs {
            let key = wake_key(wake_id);
            match self.state.storage().get::<WakeRecord>(&key).await {
                Ok(record) if matches!(record.state, WakeState::Pending) => {
                    let timeout_ms = record.timeout_ms;
                    transitions.push(DueWake {
                        wake_id: *wake_id,
                        record,
                        timeout_ms,
                    });
                }
                Ok(_) => {
                    // Already terminal (e.g., complete_wake transitioned
                    // it before this alarm fired). Skip; the inbox entry
                    // was removed at transition time per Decision 6.
                }
                Err(_) => {
                    tracing::warn!(
                        wake_id = %wake_id,
                        "alarm-fire references missing wake row (defensive skip per Lock 2.6.8)"
                    );
                }
            }
        }

        // Group inbox updates by target identity (M distinct targets).
        // Each target's inbox is read once and updated to remove all
        // transitioning wake_ids targeting it.
        let mut inbox_updates: HashMap<String, VecDeque<WakeId>> = HashMap::new();
        for dw in &transitions {
            let target_b64 = dw.record.target_identity.clone();
            if let std::collections::hash_map::Entry::Vacant(e) =
                inbox_updates.entry(target_b64.clone())
            {
                let key = inbox_key(&target_b64);
                let inbox: VecDeque<WakeId> =
                    self.state.storage().get(&key).await.unwrap_or_default();
                e.insert(inbox);
            }
        }
        for dw in &transitions {
            if let Some(inbox) = inbox_updates.get_mut(&dw.record.target_identity) {
                inbox.retain(|id| *id != dw.wake_id);
            }
        }

        // Compute transitioned wake rows (Pending → TimedOut).
        let transitioned_records: Vec<(WakeId, WakeRecord)> = transitions
            .iter()
            .map(|dw| {
                let mut rec = dw.record.clone();
                rec.state = WakeState::TimedOut;
                (dw.wake_id, rec)
            })
            .collect();

        // Compose put_multiple_raw object: N transitioned wake rows + M
        // target inboxes + alarm_queue = N+1+M keys (Decision 6).
        let writes = Object::new();
        for (id, rec) in &transitioned_records {
            let jsv = serde_wasm_bindgen::to_value(rec).map_err(|e| {
                Error::RustError(format!("alarm: serialize wake record {}: {}", id, e))
            })?;
            Reflect::set(&writes, &JsValue::from_str(&wake_key(id)), &jsv)
                .map_err(|_| Error::RustError(format!("alarm: Reflect::set wake key {}", id)))?;
        }
        for (target_b64, inbox) in &inbox_updates {
            let jsv = serde_wasm_bindgen::to_value(inbox).map_err(|e| {
                Error::RustError(format!("alarm: serialize inbox for {}: {}", target_b64, e))
            })?;
            Reflect::set(&writes, &JsValue::from_str(&inbox_key(target_b64)), &jsv).map_err(
                |_| Error::RustError(format!("alarm: Reflect::set inbox {}", target_b64)),
            )?;
        }
        let aq_jsv = serde_wasm_bindgen::to_value(&alarm_queue)
            .map_err(|e| Error::RustError(format!("alarm: serialize alarm_queue: {}", e)))?;
        Reflect::set(&writes, &JsValue::from_str(ALARM_QUEUE_KEY), &aq_jsv)
            .map_err(|_| Error::RustError("alarm: Reflect::set alarm_queue".to_string()))?;

        self.state
            .storage()
            .put_multiple_raw(writes)
            .await
            .map_err(|e| Error::RustError(format!("alarm: put_multiple_raw failed: {}", e)))?;

        // Post-storage: resolve in-memory resolvers (best-effort per
        // Lock 4.6.3 — Sender::send Err if Receiver dropped is no-op).
        //
        // RefCell discipline: scope `borrow_mut` to the synchronous
        // removal step. We collect the (Sender, timeout_ms) pairs into a
        // local Vec inside the borrow scope and drop the borrow before
        // calling `Sender::send` (also synchronous, but kept outside the
        // borrow as a habit-forming pattern — borrows don't compose with
        // future awaits). The next `.await` is `reschedule_alarm` below;
        // the borrow MUST be dropped before reaching it.
        let resolver_sends: Vec<(oneshot::Sender<WakeResolverResult>, u32)> = {
            let mut resolvers = self.wake_resolvers.borrow_mut();
            transitions
                .iter()
                .filter_map(|dw| resolvers.remove(&dw.wake_id).map(|s| (s, dw.timeout_ms)))
                .collect()
            // `resolvers` borrow drops at end of scope.
        };
        for (sender, timeout_ms) in resolver_sends {
            let _ = sender.send(Err(StoaError::Wake(WakeError::TimeoutExpired {
                timeout: Duration::from_millis(timeout_ms as u64),
            })));
        }

        // Reschedule alarm to the next-earliest entry (or delete it).
        self.reschedule_alarm(&alarm_queue).await?;

        Response::ok("alarm processed")
    }
}

impl TallyTeamDO {
    /// Lazy `team:meta` initialization per Phase 0 §4.3.
    ///
    /// Phase 0 §4.3 adapt clause: worker-rs `State::id()` returns an
    /// `ObjectId<'_>` whose `Display` impl produces a hex string (per the
    /// `id()` doc-comment in worker-rs 0.5: "can be converted into a hex
    /// string using its `to_string()` method"). Phase 0 §2.5's
    /// `team_id_b64` field name uses the `_b64` suffix as shorthand for
    /// "stringified DO identifier" rather than a strict format claim;
    /// populated here with the hex value per the worker-rs API.
    async fn ensure_team_meta_initialized(&self) -> Result<()> {
        if self
            .state
            .storage()
            .get::<TeamMeta>(TEAM_META_KEY)
            .await
            .is_err()
        {
            let meta = TeamMeta {
                tenancy_prefix: TENANCY_PREFIX_MVP.to_string(),
                team_id_b64: self.state.id().to_string(),
                created_at: Date::now().as_millis() as i64,
            };
            self.state.storage().put(TEAM_META_KEY, &meta).await?;
        }
        Ok(())
    }

    async fn handle_register(&self, req: &mut Request) -> Result<Response> {
        let body: RegisterRequest = req.json().await?;
        let identity = match Identity::from_url_safe_b64(&body.identity_b64) {
            Ok(id) => id,
            Err(e) => return Response::error(format!("invalid identity_b64: {}", e), 400),
        };
        let ctx_id = body.context_id.clone();
        // Call the `&self` inherent method rather than the `&mut self`
        // `WakeRouter` trait method. See `wake_router.rs` for why the
        // trait impl is a loud-failure stub post-0.6.0 upgrade.
        match self
            .register_handler_inherent(&identity, body.context_id.as_bytes())
            .await
        {
            Ok(()) => Response::from_json(&OkResponse),
            Err(e) => Ok(stoa_error_to_response(
                &e,
                &ErrorContext {
                    context_id: Some(&ctx_id),
                    ..Default::default()
                },
            )),
        }
    }

    async fn handle_unregister(&self, req: &mut Request) -> Result<Response> {
        let body: UnregisterRequest = req.json().await?;
        let identity = match Identity::from_url_safe_b64(&body.identity_b64) {
            Ok(id) => id,
            Err(e) => return Response::error(format!("invalid identity_b64: {}", e), 400),
        };
        let ctx_id = body.context_id.clone();
        // See `handle_register` for why we call the inherent method.
        match self
            .unregister_handler_inherent(&identity, body.context_id.as_bytes())
            .await
        {
            Ok(()) => Response::from_json(&OkResponse),
            Err(e) => Ok(stoa_error_to_response(
                &e,
                &ErrorContext {
                    context_id: Some(&ctx_id),
                    ..Default::default()
                },
            )),
        }
    }

    /// HTTP dispatch handler per Phase 0 Decision 9 + Lock 6.6.1.
    ///
    /// Wire-format per dispatch sub-PR Phase 0 §1 (`DispatchRequest`).
    /// Flow: deserialize → validate (caller, target, payload, timeout) →
    /// [`Self::dispatch_with_caller`] → map result to
    /// [`DispatchResponse`] (HTTP 200) or error via
    /// [`stoa_error_to_response`].
    ///
    /// **F.1 expansion:** `dispatch_with_caller` now returns
    /// `(Option<WakeId>, Result<(WakeResponse, u64), StoaError>)`. The
    /// leading `Option<WakeId>` lets the error mapper populate the
    /// `wake_id` field of §3.3's structured timeout response
    /// (`{ "error": "wake timed out", "wake_id": "...", ... }`). The
    /// success path constructs an internal [`DispatchResponse`] with
    /// `wake_id` + `completed_at` (both new F.1 fields) so the Worker
    /// layer can build the public response.
    async fn handle_dispatch(&self, req: &mut Request) -> Result<Response> {
        let body: DispatchRequest = match req.json().await {
            Ok(b) => b,
            Err(e) => return Response::error(format!("invalid request body: {}", e), 400),
        };

        // Validate identities.
        let caller = match Identity::from_url_safe_b64(&body.caller_identity_b64) {
            Ok(id) => id,
            Err(e) => {
                return Response::error(format!("invalid caller_identity_b64: {}", e), 400);
            }
        };
        let target = match Identity::from_url_safe_b64(&body.target_identity_b64) {
            Ok(id) => id,
            Err(e) => {
                return Response::error(format!("invalid target_identity_b64: {}", e), 400);
            }
        };

        // Validate context_id.
        if body.context_id.is_empty() {
            return Ok(stoa_error_to_response(
                &StoaError::Wake(WakeError::DispatchRefused {
                    reason: "context_id must be non-empty".to_string(),
                }),
                &ErrorContext::default(),
            ));
        }

        // Validate payload.
        let payload_bytes = match BASE64_URL_SAFE_NO_PAD.decode(body.payload_b64.as_bytes()) {
            Ok(b) => b,
            Err(e) => {
                return Ok(stoa_error_to_response(
                    &StoaError::Wake(WakeError::DispatchRefused {
                        reason: format!("invalid payload_b64: {}", e),
                    }),
                    &ErrorContext::default(),
                ));
            }
        };
        if payload_bytes.len() > MAX_PAYLOAD_BYTES {
            return Ok(stoa_error_to_response(
                &StoaError::Wake(WakeError::DispatchRefused {
                    reason: format!(
                        "payload {} bytes exceeds MAX_PAYLOAD_BYTES ({})",
                        payload_bytes.len(),
                        MAX_PAYLOAD_BYTES
                    ),
                }),
                &ErrorContext::default(),
            ));
        }

        // Validate timeout. Zero / out-of-range → InvalidTimeout (400 per
        // §3.1 correction).
        if body.timeout_ms < MIN_TIMEOUT_MS || body.timeout_ms > MAX_TIMEOUT_MS {
            return Ok(stoa_error_to_response(
                &StoaError::Wake(WakeError::InvalidTimeout),
                &ErrorContext::default(),
            ));
        }
        let timeout = Duration::from_millis(body.timeout_ms as u64);

        // Delegate to the inherent method. F.1: pattern-match the
        // `(Option<WakeId>, Result<...>)` tuple so error responses can
        // include `wake_id` when known.
        //
        // §3.3 HandlerNotFound includes a `context_id` field; we
        // capture the body's context_id locally for that path.
        let ctx_id = body.context_id.clone();
        let (wake_id_opt, result) = self
            .dispatch_with_caller(
                &target,
                &caller,
                body.context_id.as_bytes(),
                WakePayload(payload_bytes),
                timeout,
            )
            .await;

        match (wake_id_opt, result) {
            (Some(wake_id), Ok((response, completed_at))) => {
                let resp = DispatchResponse {
                    wake_id,
                    responding_identity_b64: body.target_identity_b64,
                    response_payload_b64: BASE64_URL_SAFE_NO_PAD.encode(&response.0),
                    completed_at,
                };
                Response::from_json(&resp)
            }
            (None, Ok(_)) => {
                // dispatch_with_caller invariant: success path
                // generates a wake_id before any await; reaching this
                // arm means the (Option<WakeId>, Result) contract is
                // violated. Surface as 500 with structured body rather
                // than panicking.
                Ok(stoa_error_to_response(
                    &StoaError::Wake(WakeError::Other(
                        "dispatch returned Ok without wake_id (invariant violation)".to_string(),
                    )),
                    &ErrorContext::default(),
                ))
            }
            (wake_id_opt, Err(e)) => Ok(stoa_error_to_response(
                &e,
                &ErrorContext {
                    wake_id: wake_id_opt.as_ref(),
                    context_id: Some(&ctx_id),
                },
            )),
        }
    }

    /// Dispatch a wake — persistence-aware inherent method per Phase 0
    /// Decision 9 + Lock 6.6.3, refined by HTTP API surface sub-PR F.1.
    ///
    /// The trait method [`WakeRouter::dispatch`] has no caller param;
    /// `WakeRecord` storage requires one (Decision 8). This inherent
    /// method is the operational implementation surface.
    ///
    /// **F.1 return-type expansion:** returns `(Option<WakeId>,
    /// StdResult<(WakeResponse, u64), StoaError>)`. The leading
    /// `Option<WakeId>` is the wake identifier *if it was already
    /// generated* — `None` for pre-generation validation errors,
    /// `Some(_)` once generation has happened (including the timeout
    /// path, where the wake_id is needed for the 408 error response's
    /// `wake_id` context field). The inner result's `Ok` arm carries
    /// `(WakeResponse, completed_at)`; the `completed_at` value flows
    /// from [`Self::complete_wake`] via the oneshot channel. See
    /// [`crate::rpc::DispatchResponse`] for the matching response shape.
    ///
    /// Flow per Decision 5 (operation shape):
    /// 1. Pre-checks (in-memory): validate timeout, payload size,
    ///    context UTF-8.
    /// 2. Sequential reads: target inbox; alarm_queue.
    /// 3. Compute new state in locals: new `WakeRecord` (Pending),
    ///    updated inbox (pushed + overflow-evicted if at INBOX_LIMIT),
    ///    updated alarm_queue (new entry at `absolute_timeout`;
    ///    overflow entry removed if any).
    /// 4. Atomic delete of overflow wake row (α.2 ordering;
    ///    overflow-only case).
    /// 5. Atomic multi-key `put_multiple_raw`: new wake row + updated
    ///    inbox + updated alarm_queue (3 keys).
    /// 6. Post-storage: insert resolver in `wake_resolvers`; call
    ///    `set_alarm` with the new earliest absolute_timeout (or
    ///    `delete_alarm` if alarm_queue ends up empty — shouldn't
    ///    happen here since we just added our entry).
    ///
    /// Awaits the oneshot Receiver raced against a `worker::Delay` of
    /// `timeout + SAFETY_BUFFER` via `futures::future::select` as a
    /// belt-and-suspenders safety net. The alarm-based timeout is the
    /// canonical path; the `worker::Delay` race is the defensive
    /// backstop. `tokio::time::timeout` is not used here because
    /// `tokio::time` has no timer driver on `wasm32-unknown-unknown`
    /// and panics at runtime; `worker::Delay` is the wasm-compatible
    /// substitute.
    pub(crate) async fn dispatch_with_caller(
        &self,
        target: &Identity,
        caller: &Identity,
        context: &[u8],
        payload: WakePayload,
        timeout: Duration,
    ) -> (Option<WakeId>, StdResult<(WakeResponse, u64), StoaError>) {
        // ── 1. Pre-checks ─────────────────────────────────────────────
        // Pre-checks fire *before* wake_id generation; their error
        // responses carry `None` for the wake_id context (no wake_id
        // has been allocated yet).
        let context_str = match std::str::from_utf8(context) {
            Ok(s) => s,
            Err(_) => {
                return (
                    None,
                    Err(StoaError::Wake(WakeError::DispatchRefused {
                        reason: "context must be valid UTF-8".to_string(),
                    })),
                );
            }
        };
        if context_str.is_empty() {
            return (
                None,
                Err(StoaError::Wake(WakeError::DispatchRefused {
                    reason: "context_id must be non-empty".to_string(),
                })),
            );
        }
        if payload.0.len() > MAX_PAYLOAD_BYTES {
            return (
                None,
                Err(StoaError::Wake(WakeError::DispatchRefused {
                    reason: format!(
                        "payload {} bytes exceeds MAX_PAYLOAD_BYTES ({})",
                        payload.0.len(),
                        MAX_PAYLOAD_BYTES
                    ),
                })),
            );
        }
        let timeout_ms_u128 = timeout.as_millis();
        if timeout_ms_u128 < MIN_TIMEOUT_MS as u128 || timeout_ms_u128 > MAX_TIMEOUT_MS as u128 {
            return (None, Err(StoaError::Wake(WakeError::InvalidTimeout)));
        }
        let timeout_ms = timeout_ms_u128 as u32;

        let target_b64 = target.to_url_safe_b64();
        let caller_b64 = caller.to_url_safe_b64();

        // ── 2. Sequential reads ───────────────────────────────────────
        let mut inbox: VecDeque<WakeId> = self
            .state
            .storage()
            .get(&inbox_key(&target_b64))
            .await
            .unwrap_or_default();

        let mut alarm_queue: BTreeMap<u64, Vec<WakeId>> = self
            .state
            .storage()
            .get(ALARM_QUEUE_KEY)
            .await
            .unwrap_or_default();

        // ── 3. Compute new state in locals ────────────────────────────
        let wake_id = WakeId::new();
        let created_at = Date::now().as_millis();
        let absolute_timeout = created_at + timeout_ms as u64;

        let new_record = WakeRecord {
            wake_id,
            target_identity: target_b64.clone(),
            caller_identity: caller_b64,
            context_id: context_str.to_string(),
            payload: payload.0,
            state: WakeState::Pending,
            created_at,
            timeout_ms,
            response_payload: None,
        };

        // Push new wake_id to inbox tail; if over INBOX_LIMIT, evict head.
        inbox.push_back(wake_id);
        let mut overflow_wake_id: Option<WakeId> = None;
        if inbox.len() > INBOX_LIMIT {
            overflow_wake_id = inbox.pop_front();
        }

        // alarm_queue insert: new wake.
        alarm_queue
            .entry(absolute_timeout)
            .or_default()
            .push(wake_id);

        // alarm_queue cleanup for overflow_wake_id: scan-based lookup
        // per Lock 2.6.1 ("Dispatch's overflow uses alarm_queue scan").
        // No overflow wake row read.
        if let Some(ovid) = overflow_wake_id {
            let mut found_at: Option<u64> = None;
            'scan: for (k, ids) in alarm_queue.iter() {
                for id in ids {
                    if *id == ovid {
                        found_at = Some(*k);
                        break 'scan;
                    }
                }
            }
            match found_at {
                Some(k) => {
                    if let Some(ids) = alarm_queue.get_mut(&k) {
                        ids.retain(|id| *id != ovid);
                    }
                    if alarm_queue.get(&k).map(|v| v.is_empty()).unwrap_or(false) {
                        alarm_queue.remove(&k);
                    }
                }
                None => {
                    // Invariant-violation defense (Lock 2.6.1): inbox
                    // held a wake_id whose alarm_queue entry was already
                    // gone. Log + proceed with skipped cleanup.
                    tracing::warn!(
                        wake_id = %ovid,
                        "dispatch overflow: alarm_queue scan-not-found for evicted wake_id \
                         (invariant violation; proceeding with skipped cleanup per Lock 2.6.1)"
                    );
                }
            }
        }

        // ── 4. Atomic delete (overflow case only — α.2 ordering) ──────
        // F.1: wake_id is now allocated; the post-generation error
        // arms below carry `Some(wake_id)` so callers can populate the
        // `wake_id` field in §3.3-style structured error responses.
        if let Some(ovid) = overflow_wake_id {
            if let Err(e) = self.state.storage().delete(&wake_key(&ovid)).await {
                return (
                    Some(wake_id),
                    Err(StoaError::Wake(WakeError::Other(format!(
                        "dispatch overflow delete failed: {}",
                        e
                    )))),
                );
            }
        }

        // ── 5. Atomic multi-key put_multiple_raw ──────────────────────
        let writes = Object::new();
        let record_jsv = match serde_wasm_bindgen::to_value(&new_record) {
            Ok(v) => v,
            Err(e) => {
                return (
                    Some(wake_id),
                    Err(StoaError::Wake(WakeError::Other(format!(
                        "serialize wake record: {}",
                        e
                    )))),
                );
            }
        };
        if Reflect::set(
            &writes,
            &JsValue::from_str(&wake_key(&wake_id)),
            &record_jsv,
        )
        .is_err()
        {
            return (
                Some(wake_id),
                Err(StoaError::Wake(WakeError::Other(
                    "Reflect::set wake key".to_string(),
                ))),
            );
        }
        let inbox_jsv = match serde_wasm_bindgen::to_value(&inbox) {
            Ok(v) => v,
            Err(e) => {
                return (
                    Some(wake_id),
                    Err(StoaError::Wake(WakeError::Other(format!(
                        "serialize inbox: {}",
                        e
                    )))),
                );
            }
        };
        if Reflect::set(
            &writes,
            &JsValue::from_str(&inbox_key(&target_b64)),
            &inbox_jsv,
        )
        .is_err()
        {
            return (
                Some(wake_id),
                Err(StoaError::Wake(WakeError::Other(
                    "Reflect::set inbox key".to_string(),
                ))),
            );
        }
        let aq_jsv = match serde_wasm_bindgen::to_value(&alarm_queue) {
            Ok(v) => v,
            Err(e) => {
                return (
                    Some(wake_id),
                    Err(StoaError::Wake(WakeError::Other(format!(
                        "serialize alarm_queue: {}",
                        e
                    )))),
                );
            }
        };
        if Reflect::set(&writes, &JsValue::from_str(ALARM_QUEUE_KEY), &aq_jsv).is_err() {
            return (
                Some(wake_id),
                Err(StoaError::Wake(WakeError::Other(
                    "Reflect::set alarm_queue key".to_string(),
                ))),
            );
        }
        if let Err(e) = self.state.storage().put_multiple_raw(writes).await {
            return (
                Some(wake_id),
                Err(StoaError::Wake(WakeError::Other(format!(
                    "put_multiple_raw: {}",
                    e
                )))),
            );
        }

        // ── 6. Post-storage in-memory mutation + alarm scheduling ─────
        // F.1 channel payload: `(WakeResponse, u64)` where `u64` is the
        // `completed_at` timestamp captured by `complete_wake`'s
        // post-storage step. The error arm stays `StoaError` (timeout
        // / resolver-drop / etc.).
        let (sender, receiver) = oneshot::channel::<WakeResolverResult>();
        // RefCell discipline: scoped borrow_mut, dropped before the next
        // `.await` (reschedule_alarm). The borrow is purely synchronous.
        self.wake_resolvers.borrow_mut().insert(wake_id, sender);

        // Reschedule alarm to the new earliest entry. The queue is
        // non-empty (just added our entry); but reschedule_alarm
        // tolerates both cases.
        if let Err(e) = self.reschedule_alarm(&alarm_queue).await {
            // Storage already committed; alarm scheduling failure leaves
            // the dispatch in a half-state where the wake row exists but
            // no alarm is set. Best-effort recovery: drop the resolver,
            // surface the error.
            self.wake_resolvers.borrow_mut().remove(&wake_id);
            return (
                Some(wake_id),
                Err(StoaError::Wake(WakeError::Other(format!(
                    "set_alarm failed post-storage: {}",
                    e
                )))),
            );
        }

        // Inbox-waiter signal per HTTP API surface sub-PR Phase 0
        // Decision 2 (signal-only option c). If a `handle_read_inbox`
        // call is long-polling on this target identity, the dispatch
        // notifies it post-storage so the read can return populated.
        //
        // Best-effort: `sender.send(())` returning `Err(())` means the
        // Receiver was dropped (subscriber timed out or was replaced by
        // a newer subscribe). Not an error to log.
        //
        // RefCell discipline: extract the Sender under a scoped
        // borrow_mut, drop the borrow, then send synchronously outside
        // the borrow scope.
        let inbox_waiter = self.inbox_waiters.borrow_mut().remove(&target_b64);
        if let Some(sender) = inbox_waiter {
            let _ = sender.send(());
        }

        // ── Await resolution with safety timeout ──────────────────────
        // `tokio::time::timeout` is unusable on wasm32-unknown-unknown
        // (no timer driver; panics at runtime). Substitute the
        // wasm-compatible `worker::Delay` raced via `futures::future::select`.
        let total_timeout = timeout + SAFETY_BUFFER;
        let delay = worker::Delay::from(total_timeout);

        let result = match select(receiver, delay).await {
            Either::Left((channel_result, _delay)) => match channel_result {
                Ok(Ok((response, completed_at))) => Ok((response, completed_at)),
                Ok(Err(e)) => Err(e),
                Err(_recv_error) => Err(StoaError::Wake(WakeError::Other(
                    "resolver dropped without resolution (DO restart or bug)".to_string(),
                ))),
            },
            Either::Right(((), _receiver)) => {
                Err(StoaError::Wake(WakeError::TimeoutExpired { timeout }))
            }
        };
        (Some(wake_id), result)
    }

    /// HTTP complete handler per Phase 0 Lock 6.6.2.
    ///
    /// Wire-format per dispatch sub-PR Phase 0 §2 (`CompleteRequest`).
    async fn handle_complete_wake(&self, req: &mut Request) -> Result<Response> {
        let body: CompleteRequest = match req.json().await {
            Ok(b) => b,
            Err(e) => return Response::error(format!("invalid request body: {}", e), 400),
        };

        let by_identity = match Identity::from_url_safe_b64(&body.by_identity_b64) {
            Ok(id) => id,
            Err(e) => {
                return Response::error(format!("invalid by_identity_b64: {}", e), 400);
            }
        };

        let wake_id = match WakeId::from_str(&body.wake_id) {
            Ok(id) => id,
            Err(e) => {
                return Response::error(format!("invalid wake_id: {}", e), 400);
            }
        };

        let response_payload =
            match BASE64_URL_SAFE_NO_PAD.decode(body.response_payload_b64.as_bytes()) {
                Ok(b) => b,
                Err(e) => {
                    return Response::error(format!("invalid response_payload_b64: {}", e), 400);
                }
            };
        if response_payload.len() > MAX_RESPONSE_BYTES {
            return Ok(stoa_error_to_response(
                &StoaError::Wake(WakeError::DispatchRefused {
                    reason: format!(
                        "response {} bytes exceeds MAX_RESPONSE_BYTES ({})",
                        response_payload.len(),
                        MAX_RESPONSE_BYTES
                    ),
                }),
                &ErrorContext {
                    wake_id: Some(&wake_id),
                    ..Default::default()
                },
            ));
        }

        match self
            .complete_wake(&wake_id, &by_identity, response_payload)
            .await
        {
            // F.1 expansion: complete_wake now returns the
            // `completed_at` timestamp. For the direct
            // POST /complete path (caller is not the awaiting
            // dispatcher), the internal success response carries
            // `completed_at` so the Worker can include it in
            // [`PublicCompleteResponse`] if desired. The current
            // public spec (§3.3) for complete's success body
            // (`{ "completed": true, "wake_id": "..." }`) doesn't
            // include `completed_at`, but the internal value is
            // available here for forward-compat extension. We pass
            // through the timestamp in a JSON object so the Worker
            // can ignore it without parsing complexity.
            Ok(completed_at) => Response::from_json(&serde_json::json!({
                "completed_at": completed_at,
            })),
            Err(e) => Ok(tally_error_to_response(
                &e,
                &ErrorContext {
                    wake_id: Some(&wake_id),
                    ..Default::default()
                },
            )),
        }
    }

    /// Complete a pending wake — Pending → Completed transition per
    /// Phase 0 Lock 6.6.4 (β.1).
    ///
    /// Per HTTP API surface sub-PR Decision 4: returns `Result<u64,
    /// TallyError>` rather than `Result<(), StoaError>`. The three
    /// implementation-specific error sites (wake row not found, wake
    /// not Pending, by_identity mismatch) are not dispatch protocol
    /// errors; they map to dedicated [`TallyError`] variants rather
    /// than to fudged [`stoa::WakeError`] variants. See
    /// `crate::error` for the full reasoning.
    ///
    /// **F.1 expansion:** the `Ok` arm carries the storage-write
    /// timestamp (`Date::now().as_millis() as u64` captured immediately
    /// after `put_multiple_raw` succeeds) as the `completed_at` value.
    /// This timestamp flows through the [`Self::wake_resolvers`] oneshot
    /// channel as the second tuple element so the dispatching awaiter
    /// can populate [`DispatchResponse::completed_at`] without a
    /// separate clock read. The timestamp is ephemeral coordination
    /// data — it's the Cloudflare DO's wall-clock at write time, not
    /// a persisted [`WakeRecord`] field (WakeRecord stays at 9 fields
    /// per Decision 8).
    ///
    /// Flow per Decision 5 (operation shape):
    /// 1. Pre-checks: deserialize-time identity/wake_id validation done
    ///    in the HTTP handler.
    /// 2. Sequential reads: wake row → alarm_queue → target inbox (the
    ///    target identity comes from the wake row, so inbox read must
    ///    follow wake row read).
    /// 3. State guard: wake must be `Pending`; otherwise refuse (caller
    ///    is racing a timeout fire or replaying).
    /// 4. Identity guard: `by_identity` must equal `wake.target_identity`
    ///    (MVP — only the target completes its own wake; future
    ///    delegation semantics could relax).
    /// 5. Compute new state in locals: transitioned wake row (Pending →
    ///    Completed; response_payload populated); updated alarm_queue
    ///    (entry at `wake.created_at + wake.timeout_ms` removed; the
    ///    absolute_timeout is direct-computed since the row is in hand
    ///    per Lock 6.6.4); updated target inbox (wake_id removed per
    ///    Decision 6 transition contract).
    /// 6. Atomic `put_multiple_raw`: 3 keys (transitioned wake row +
    ///    target inbox + alarm_queue). Capture `Date::now().as_millis()`
    ///    immediately after for the `completed_at` return value.
    /// 7. Post-storage: resolve the in-memory resolver if present
    ///    (best-effort) with `(WakeResponse, completed_at)`; call
    ///    `set_alarm` or `delete_alarm` for the new alarm_queue state.
    pub(crate) async fn complete_wake(
        &self,
        wake_id: &WakeId,
        by_identity: &Identity,
        response_payload: Vec<u8>,
    ) -> StdResult<u64, TallyError> {
        // ── 2. Sequential reads ───────────────────────────────────────
        let wake = match self
            .state
            .storage()
            .get::<WakeRecord>(&wake_key(wake_id))
            .await
        {
            Ok(r) => r,
            Err(_) => return Err(TallyError::WakeNotFound),
        };

        // ── 3. State guard ────────────────────────────────────────────
        if !matches!(wake.state, WakeState::Pending) {
            return Err(TallyError::AlreadyTerminal);
        }

        // ── 4. Identity guard ─────────────────────────────────────────
        let by_b64 = by_identity.to_url_safe_b64();
        if by_b64 != wake.target_identity {
            return Err(TallyError::IdentityMismatch);
        }

        // Continue sequential reads: alarm_queue, target inbox.
        let mut alarm_queue: BTreeMap<u64, Vec<WakeId>> = self
            .state
            .storage()
            .get(ALARM_QUEUE_KEY)
            .await
            .unwrap_or_default();

        let mut inbox: VecDeque<WakeId> = self
            .state
            .storage()
            .get(&inbox_key(&wake.target_identity))
            .await
            .unwrap_or_default();

        // ── 5. Compute new state in locals ────────────────────────────
        let absolute_timeout = wake.created_at + wake.timeout_ms as u64;
        if let Some(ids) = alarm_queue.get_mut(&absolute_timeout) {
            ids.retain(|id| id != wake_id);
        }
        if alarm_queue
            .get(&absolute_timeout)
            .map(|v| v.is_empty())
            .unwrap_or(false)
        {
            alarm_queue.remove(&absolute_timeout);
        }

        inbox.retain(|id| id != wake_id);

        let mut new_record = wake.clone();
        new_record.state = WakeState::Completed;
        new_record.response_payload = Some(response_payload.clone());

        // ── 6. Atomic put_multiple_raw (3 keys per Decision 6) ────────
        let writes = Object::new();
        let record_jsv = serde_wasm_bindgen::to_value(&new_record).map_err(|e| {
            StoaError::Wake(WakeError::Other(format!("serialize wake record: {}", e)))
        })?;
        Reflect::set(&writes, &JsValue::from_str(&wake_key(wake_id)), &record_jsv)
            .map_err(|_| StoaError::Wake(WakeError::Other("Reflect::set wake key".to_string())))?;
        let inbox_jsv = serde_wasm_bindgen::to_value(&inbox)
            .map_err(|e| StoaError::Wake(WakeError::Other(format!("serialize inbox: {}", e))))?;
        Reflect::set(
            &writes,
            &JsValue::from_str(&inbox_key(&wake.target_identity)),
            &inbox_jsv,
        )
        .map_err(|_| StoaError::Wake(WakeError::Other("Reflect::set inbox key".to_string())))?;
        let aq_jsv = serde_wasm_bindgen::to_value(&alarm_queue).map_err(|e| {
            StoaError::Wake(WakeError::Other(format!("serialize alarm_queue: {}", e)))
        })?;
        Reflect::set(&writes, &JsValue::from_str(ALARM_QUEUE_KEY), &aq_jsv).map_err(|_| {
            StoaError::Wake(WakeError::Other("Reflect::set alarm_queue key".to_string()))
        })?;
        self.state
            .storage()
            .put_multiple_raw(writes)
            .await
            .map_err(|e| StoaError::Wake(WakeError::Other(format!("put_multiple_raw: {}", e))))?;

        // F.1 expansion: capture the storage-write timestamp immediately
        // after `put_multiple_raw` returns. This is the canonical
        // `completed_at` value — the moment the durable transition
        // committed. Used as the second tuple element on both the
        // resolver send (so the dispatching awaiter doesn't need a
        // separate clock read) and this function's return value (so
        // a direct `POST /complete` HTTP caller — i.e. a caller that
        // isn't the awaiting dispatcher — also receives the timestamp
        // for inclusion in [`PublicCompleteResponse`] if needed).
        let completed_at = Date::now().as_millis();

        // ── 7. Post-storage: resolve resolver + reschedule alarm ──────
        // RefCell discipline: extract the Sender under a scoped
        // borrow_mut, drop the borrow, then send synchronously outside
        // the borrow scope — the next `.await` (reschedule_alarm) MUST
        // NOT happen while a borrow is held.
        let resolver = self.wake_resolvers.borrow_mut().remove(wake_id);
        if let Some(sender) = resolver {
            let _ = sender.send(Ok((WakeResponse(response_payload), completed_at)));
        }
        self.reschedule_alarm(&alarm_queue)
            .await
            .map_err(|e| StoaError::Wake(WakeError::Other(format!("reschedule_alarm: {}", e))))?;

        Ok(completed_at)
    }

    /// Set or delete the DO alarm to match the alarm_queue's earliest
    /// entry. Always reschedules — does not optimize for "only if
    /// changed" since the tracking complexity exceeds the network-call
    /// savings (Phase 0 Decision 9 operational notes).
    async fn reschedule_alarm(&self, alarm_queue: &BTreeMap<u64, Vec<WakeId>>) -> Result<()> {
        match alarm_queue.keys().next() {
            Some(earliest_ms) => {
                // `ScheduledTime: From<i64>` interprets the value as unix
                // milliseconds. Saturating cast guards against u64 values
                // exceeding i64::MAX (well outside any realistic clock).
                let scheduled_ms: i64 = (*earliest_ms).try_into().unwrap_or(i64::MAX);
                self.state.storage().set_alarm(scheduled_ms).await
            }
            None => self.state.storage().delete_alarm().await,
        }
    }

    /// Validate API key — uniform-true MVP per HTTP API surface sub-PR
    /// Phase 0 Decision 1.
    ///
    /// MVP behaviour: the DO attempts
    /// [`Identity::from_url_safe_b64`] on the supplied bearer; on
    /// success returns `{ valid: true, identity_b64: Some(bearer) }`;
    /// on parse failure returns `{ valid: false, identity_b64: None }`.
    /// Phase 2 will replace the parse-as-identity logic with a real
    /// key lookup against `agent:{identity_b64}:api_keys`; the wire
    /// contract is stable across the transition.
    ///
    /// MVP scope boundary: real authentication deferred to Phase 2 admin
    /// tooling. The plumbing exists for Phase 2 to fill in
    /// (`agent:{identity_b64}:api_keys` storage is read here but never
    /// written until Phase 2). Tally MVP must not be deployed to
    /// publicly-accessible environments without Phase 2 auth in place
    /// (Phase 0 §4.4 deployment boundary).
    async fn handle_validate_api_key(&self, req: &mut Request) -> Result<Response> {
        let body: ValidateApiKeyRequest = req.json().await?;

        // MVP: bearer is the URL-safe-base64 identity. Parse success →
        // valid; parse failure → invalid.
        match Identity::from_url_safe_b64(&body.bearer) {
            Ok(_) => Response::from_json(&ValidateApiKeyResponse {
                valid: true,
                identity_b64: Some(body.bearer),
            }),
            Err(_) => Response::from_json(&ValidateApiKeyResponse {
                valid: false,
                identity_b64: None,
            }),
        }
    }

    /// Read inbox per HTTP API surface sub-PR Phase 0 Decisions 2 + 3.
    ///
    /// Per Decision 3: identity is no longer parsed from `?identity=`;
    /// the Worker layer extracts it from the URL path and forwards it
    /// here as a routing parameter (the second argument).
    ///
    /// Per Decision 2: when `wait_seconds > 0` and the inbox is empty
    /// after the initial read, subscribe to `inbox_waiters`
    /// (subscribe-first ordering for correctness) and race the oneshot
    /// Receiver against [`worker::Delay`] via
    /// [`futures::future::select`]. The signal site is in
    /// [`Self::dispatch_with_caller`]; cleanup on timeout is
    /// unconditional (RecvError absorbed by the re-read path).
    ///
    /// Per §3.8: `limit` clamps to `[1, MAX_LIMIT]` with default
    /// [`DEFAULT_LIMIT`]; the response carries `more_available: bool`
    /// when the underlying inbox is longer than `limit`.
    ///
    /// Missing wake rows are defensively skipped with a warn-log per
    /// Lock 2.6.9 (α.2 partial-failure orphan); a skipped row counts
    /// against the limit (the inbox still references it) so
    /// `more_available` reflects the storage state honestly.
    async fn handle_read_inbox(&self, req: &Request, identity_b64: String) -> Result<Response> {
        // Worker is responsible for the URL identity ↔ authenticated
        // identity check (per Decision 3). Validate the b64 string here
        // as a defence-in-depth check — if the Worker forwarded garbage,
        // map it to a 400 rather than panicking.
        let identity = match Identity::from_url_safe_b64(&identity_b64) {
            Ok(id) => id,
            Err(e) => return Response::error(format!("invalid identity: {}", e), 400),
        };

        let url = req.url()?;
        let mut query = ReadInboxQuery {
            wait_seconds: None,
            limit: None,
        };
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "wait_seconds" => query.wait_seconds = value.parse().ok(),
                "limit" => query.limit = value.parse().ok(),
                _ => {}
            }
        }

        // Clamp limit + wait_seconds per §3.8. We accept malformed
        // (non-numeric) query values silently by treating them as None →
        // default, matching the dispatch sub-PR's permissive parsing
        // convention. The clamp logic lives in [`dispatch_consts`] as a
        // host-testable helper.
        let limit = clamp_limit(query.limit);
        let wait_seconds = clamp_wait_seconds(query.wait_seconds);

        // Read the inbox.
        let mut inbox: VecDeque<WakeId> = self
            .state
            .storage()
            .get(&inbox_key(&identity_b64))
            .await
            .unwrap_or_default();

        // Long-poll path per Decision 2: subscribe-first when inbox
        // is empty and `wait_seconds > 0`. If signal fires or the
        // re-read finds entries, fall through to the materialisation
        // step with the now-populated inbox.
        if inbox.is_empty() && wait_seconds > 0 {
            let (tx, rx) = oneshot::channel::<()>();
            // RefCell discipline: scope each borrow_mut to the
            // synchronous mutation and drop it before the next `.await`
            // (the storage re-read below).
            self.inbox_waiters
                .borrow_mut()
                .insert(identity_b64.clone(), tx);

            // Re-read AFTER insert: catches the case where dispatch
            // appended between our initial read and our insert. The
            // signal-side wouldn't find our entry yet, so the
            // notification would otherwise be lost; the re-read catches
            // the data directly.
            inbox = self
                .state
                .storage()
                .get(&inbox_key(&identity_b64))
                .await
                .unwrap_or_default();

            if !inbox.is_empty() {
                // Inbox populated during the subscribe window; remove
                // our waiter and proceed.
                self.inbox_waiters.borrow_mut().remove(&identity_b64);
            } else {
                let delay = worker::Delay::from(Duration::from_secs(wait_seconds.into()));
                match select(rx, delay).await {
                    Either::Left((Ok(()), _)) => {
                        // Signal fired; dispatch wrote to our inbox.
                        // Re-read to materialise.
                        inbox = self
                            .state
                            .storage()
                            .get(&inbox_key(&identity_b64))
                            .await
                            .unwrap_or_default();
                    }
                    Either::Left((Err(_recv_error), _)) => {
                        // Receiver dropped — most likely a replacement
                        // subscribe-call dropped our Sender (or the
                        // timeout path removed it). Re-read to
                        // gracefully degrade; the race window is
                        // bounded.
                        inbox = self
                            .state
                            .storage()
                            .get(&inbox_key(&identity_b64))
                            .await
                            .unwrap_or_default();
                    }
                    Either::Right(((), _receiver)) => {
                        // Timeout fired. Unconditional remove of our
                        // entry (Decision 2 operational notes). If a
                        // newer subscriber raced in and replaced us,
                        // they'll see RecvError on their Receiver and
                        // route to the re-read path.
                        self.inbox_waiters.borrow_mut().remove(&identity_b64);
                        // Empty response (inbox stayed empty for the
                        // full long-poll window).
                    }
                }
            }
        }

        // Suppress the unused-variable lint for `identity` — keep it
        // bound for parsing/validation side-effects (defence-in-depth).
        let _ = identity;

        // Materialise WakeSummary entries up to `limit`. `more_available`
        // is true iff the inbox holds more entries than we returned.
        let inbox_len = inbox.len();
        let more_available = inbox_len > limit;

        let mut summaries = Vec::with_capacity(limit.min(inbox_len));
        for wid in inbox.iter().take(limit) {
            match self.state.storage().get::<WakeRecord>(&wake_key(wid)).await {
                Ok(wake) => {
                    // F.1: `expires_at_ms` is derived from
                    // `created_at + timeout_ms`. The Worker layer
                    // formats it as ISO-8601 for the public
                    // PublicWakeSummary.expires_at field.
                    let expires_at_ms = wake.created_at + wake.timeout_ms as u64;
                    summaries.push(WakeSummary {
                        wake_id: wid.to_string(),
                        caller_identity_b64: wake.caller_identity,
                        context_id: wake.context_id,
                        payload_b64: BASE64_URL_SAFE_NO_PAD.encode(&wake.payload),
                        expires_at_ms,
                    });
                }
                Err(_) => {
                    // Defensive skip per Lock 2.6.9: inbox entry
                    // references a wake row that no longer exists; most
                    // likely caused by dispatch's overflow-handling α.2
                    // partial-failure (delete succeeded; put_multiple_raw
                    // failed). Bounded by α.2 partial-failure rate.
                    tracing::warn!(
                        wake_id = %wid,
                        "inbox references missing wake row (likely α.2 partial-failure orphan)"
                    );
                }
            }
        }

        Response::from_json(&ReadInboxResponse {
            wakes: summaries,
            more_available,
        })
    }
}

/// Per-error contextual fields for the structured-JSON error response.
///
/// HTTP API surface sub-PR F.1 expansion: §3.3 error response examples
/// include contextual fields (e.g., `wake_id`, `context_id`,
/// `timeout_seconds`) alongside the bare `error` string. Callers
/// populate the context fields they have in scope; the error mapper
/// drops unset fields from the response body.
#[derive(Default)]
pub(crate) struct ErrorContext<'a> {
    /// Wake identifier — included for 404/408/410 responses.
    pub(crate) wake_id: Option<&'a WakeId>,
    /// Context identifier — included for 422 `HandlerNotFound`.
    pub(crate) context_id: Option<&'a str>,
}

/// Build a structured-JSON error response per HTTP API surface sub-PR
/// Phase 0 F.1 expansion.
///
/// Wire shape: `{ "error": "...", + contextual fields }`. Status is set
/// via [`Response::with_status`] on a [`Response::from_json`] body.
/// `with_status` is infallible; we only fail to construct a response if
/// `Response::from_json` itself fails (which is essentially impossible
/// for a serde_json::Value::Object built locally — included for
/// completeness).
pub(crate) fn json_error(
    status: u16,
    error: &str,
    extras: &[(&str, serde_json::Value)],
) -> Response {
    let mut obj = serde_json::Map::new();
    obj.insert("error".to_string(), serde_json::Value::String(error.into()));
    for (k, v) in extras {
        obj.insert((*k).to_string(), v.clone());
    }
    match Response::from_json(&serde_json::Value::Object(obj)) {
        Ok(r) => r.with_status(status),
        Err(_) => Response::empty().unwrap_or_else(|_| {
            // Construction of an empty response itself failing is a
            // platform-runtime invariant violation; degrade to a
            // panic-equivalent by surfacing a default placeholder
            // Response::error which won't fail for trivial inputs.
            Response::error("internal error", status).unwrap()
        }),
    }
}

/// Map a `StoaError` to a `Response` per HTTP API surface sub-PR
/// Phase 0 §3.1 error code mapping + F.1 structured-JSON bodies.
///
/// Corrections relative to the dispatch sub-PR's mapping:
/// - `HandlerNotFound` → 422 (was 404). The condition is "client
///   request describes a handler that doesn't exist" — semantically
///   "unprocessable" rather than "resource missing".
/// - `DispatchRefused` → 422 (was 400). Same reasoning — semantic
///   validity, not malformed input.
/// - `TimeoutExpired` → 408 (was 504). The wake timeout is the
///   client's requested timeout; 408 (Request Timeout) is the
///   conventional code for client-supplied-deadline exceeded.
///
/// **F.1 structured-body additions:** the response body is now JSON
/// with an `error` field plus contextual fields per §3.3 examples:
/// - 408 `TimeoutExpired`: `{ error, wake_id, timeout_seconds }`
/// - 422 `HandlerNotFound`: `{ error, context_id }`
/// - other codes: `{ error }` only.
pub(crate) fn stoa_error_to_response(err: &StoaError, ctx: &ErrorContext<'_>) -> Response {
    match err {
        StoaError::Wake(WakeError::HandlerNotFound) => {
            // §3.3 example: `{ "error": "target has no registered handler
            // for context_id", "context_id": "task-routing" }`. The
            // context_id is the public field name; the internal call
            // sites pass it via ErrorContext::context_id.
            let extras: Vec<(&str, serde_json::Value)> = match ctx.context_id {
                Some(cid) => vec![("context_id", serde_json::Value::String(cid.into()))],
                None => vec![],
            };
            json_error(
                422,
                "target has no registered handler for context_id",
                &extras,
            )
        }
        StoaError::Wake(WakeError::DispatchRefused { reason }) => {
            // §3.3 doesn't show DispatchRefused with extras; the reason
            // string is the only context (already in `error`).
            json_error(422, reason, &[])
        }
        StoaError::Wake(WakeError::TimeoutExpired { timeout }) => {
            // §3.3 example: `{ "error": "wake timed out", "wake_id":
            // "01J5...", "timeout_seconds": 30 }`. timeout_seconds is
            // computed from the carried Duration; wake_id flows via
            // ErrorContext from the dispatch site.
            let mut extras: Vec<(&str, serde_json::Value)> = Vec::with_capacity(2);
            if let Some(wid) = ctx.wake_id {
                extras.push(("wake_id", serde_json::Value::String(wid.to_string())));
            }
            extras.push((
                "timeout_seconds",
                serde_json::Value::Number(serde_json::Number::from(timeout.as_secs())),
            ));
            json_error(408, "wake timed out", &extras)
        }
        StoaError::Wake(WakeError::InvalidTimeout) => json_error(400, "timeout must be > 0", &[]),
        StoaError::Wake(WakeError::Other(msg)) => {
            json_error(500, &format!("internal error: {}", msg), &[])
        }
        _ => json_error(500, "internal error", &[]),
    }
}

/// Map a [`TallyError`] to a `Response` per HTTP API surface sub-PR
/// Phase 0 §3.1 + F.1 structured-JSON bodies.
///
/// Delegates [`TallyError::Stoa`] to [`stoa_error_to_response`] so the
/// dispatch-scoped error mapping stays in one place. Tally-specific
/// variants:
/// - [`TallyError::WakeNotFound`] → 404 with `{ error, wake_id }`
/// - [`TallyError::AlreadyTerminal`] → 410 with `{ error, wake_id }`
/// - [`TallyError::IdentityMismatch`] → 403 with `{ error }` only
pub(crate) fn tally_error_to_response(err: &TallyError, ctx: &ErrorContext<'_>) -> Response {
    match err {
        TallyError::Stoa(stoa_err) => stoa_error_to_response(stoa_err, ctx),
        TallyError::WakeNotFound => {
            let extras: Vec<(&str, serde_json::Value)> = match ctx.wake_id {
                Some(wid) => vec![("wake_id", serde_json::Value::String(wid.to_string()))],
                None => vec![],
            };
            json_error(404, "wake not found", &extras)
        }
        TallyError::AlreadyTerminal => {
            let extras: Vec<(&str, serde_json::Value)> = match ctx.wake_id {
                Some(wid) => vec![("wake_id", serde_json::Value::String(wid.to_string()))],
                None => vec![],
            };
            json_error(410, "wake already in terminal state", &extras)
        }
        TallyError::IdentityMismatch => json_error(403, "identity does not match wake target", &[]),
    }
}
