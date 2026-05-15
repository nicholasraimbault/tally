# Dispatch sub-PR — Phase 0 design notes

**Date:** 2026-05-14
**Scope:** tally Phase 0 §9.1 dispatch sub-PR — producer-side wake-routing lifecycle in TallyTeamDO
**Status:** Pre-implementation. Architectural commitments locked through Pattern C deliberation + categorization pass. Ready for strategic-layer review of design notes; after approval, commit to tally repo and proceed to implementation PR prompt drafting.
**Provenance:** Pattern C deliberation across multiple chat sessions. Full deliberation locks (41 entries with categorization analysis) preserved at `/home/nick/Documents/dispatch-sub-pr-locks-2026-05-14.md` as deliberation record. This Phase 0 design notes document is the canonical commitment for implementation; supersedes the locks summary for that purpose.

## Summary

Implements the producer side of the wake-routing lifecycle in TallyTeamDO. In scope:

- `handle_dispatch` HTTP handler + `dispatch_with_caller` inherent method
- `handle_complete_wake` HTTP handler + `complete_wake` inherent method
- Alarm-fire transition logic with Pending→TimedOut behavior
- `handle_read_inbox` immediate-read path (long-poll deferred to §9.2)
- `WakeRouter::dispatch` trait impl with C-A-1 framing

Deferred:
- §9.2 HTTP API surface sub-PR: long-poll trigger via `inbox_waiters` subscription; identity-source resolution conventions
- §9.3 integration tests sub-PR: end-to-end dispatch + complete + timeout scenarios

The design rests on eleven architectural decisions and three wire-format API contracts. Operational discipline lives as inline doc-comments under the architectural decisions. System properties documented as consequences in their own section.

## Architectural decisions

### 1. WakeState terminal-terminal model

**Decision:** `WakeState = Pending | Completed | TimedOut`. Terminal states (Completed, TimedOut) are terminal — no further transitions.

**Context:** The state machine captures the wake's lifecycle from dispatch through resolution. Three variants suffice: dispatch creates Pending; complete_wake transitions Pending→Completed; alarm-fire transitions Pending→TimedOut. Alternative considered: 4+ variants with retryable states. Rejected because retry semantics complicate idempotency guarantees and aren't required by Phase 0 scope. Terminal-terminal simplifies state-guard checks across complete_wake, alarm-fire, and future operations.

**Implementation contract:**

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WakeState { Pending, Completed, TimedOut }
```

**Operational notes:**
- State transitions: Pending → Completed (via `complete_wake`) | Pending → TimedOut (via alarm-fire). Pre-transition state guards in both operations prevent double-transition.
- Alarm idempotency: alarm-fire's state guard (`wake.state == Pending`) makes Cloudflare's alarm-retry behavior safe. Already-terminal wakes are skipped on retry.

### 2. absolute_timeout NOT stored

**Decision:** WakeRecord does NOT store `absolute_timeout` as a field; it's computed on-the-fly from `wake.created_at + wake.timeout_ms as u64`.

**Context:** The alarm_queue is keyed by absolute_timeout_ms (the unix-millis at which the timeout expires). At alarm-fire time, the alarm_queue's keys are the source of truth for "when does this wake time out." At complete_wake time, the wake row is read for state/identity guards anyway; computing absolute_timeout from its fields gives a deterministic value for alarm_queue lookup. Alternative: store absolute_timeout as a WakeRecord field. Rejected because the arithmetic is total and deterministic (created_at and timeout_ms are immutable post-dispatch); a stored field would carry sync-risk (divergent computation across call sites) without compensating benefit.

**Implementation contract:**
- WakeRecord has `created_at: u64` (unix millis) and `timeout_ms: u32`; no `absolute_timeout` field
- complete_wake: `let absolute_timeout = wake.created_at + wake.timeout_ms as u64;` for alarm_queue cleanup lookup
- dispatch: `let absolute_timeout = current_time_ms + timeout_ms as u64;` for alarm_queue insertion

### 3. WakeId = ULID

**Decision:** WakeId is a 128-bit ULID (lexicographically-sortable identifier with timestamp prefix + random suffix).

**Context:** Wake identifiers need uniqueness, cheap generation, and time-sortability. ULID provides all three (128-bit timestamp prefix + random suffix; lexicographically sortable). Alternatives considered: UUIDv4 (no time-sortability; would require separate created_at index for time-ordered queries); sequential IDs (requires counter coordination across DO instances; coordination cost not justified). The `ulid` crate is wasm32-compatible via the `web-time` crate for the timestamp source.

**Implementation contract:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WakeId(ulid::Ulid);  // private inner field

impl WakeId {
    pub fn new() -> Self { Self(ulid::Ulid::new()) }
    pub fn as_ulid(&self) -> ulid::Ulid { self.0 }
    pub fn to_string(&self) -> String { self.0.to_string() }
    pub fn from_str(s: &str) -> Result<Self, ulid::DecodeError> {
        ulid::Ulid::from_str(s).map(Self)
    }
}
```

**Operational notes:**
- Private inner field enforces encapsulation. Direct Ulid manipulation goes through accessor methods.

### 4. Inbox = Pending-only invariant

**Decision:** Inbox entries for a wake exist if and only if `wake.state == Pending`. All operations transitioning a wake from Pending to a terminal state remove the wake_id from the target's inbox in the same atomic write (per Decision 6).

**Context:** Two design choices considered during dispatch sub-PR deliberation:
- B.i: Inbox = Pending-only. Operations transitioning Pending→terminal write target inbox to remove wake_id. read_inbox returns all entries (all are Pending).
- B.ii: Inbox can contain non-Pending. Alarm-fire doesn't touch inboxes. read_inbox filters by state at construction.

B.i chosen. The cleaner invariant makes dispatch's overflow-handling scan-not-found a true invariant violation (severity = error). Inbox stays small (bounded by Pending count, not by all-time wake count). Implementation complexity of maintaining the invariant is bounded — see Decision 6.

**Implementation contract:**
- Dispatch's inbox write adds new wake_id (Pending state); pops head if over INBOX_LIMIT
- All Pending→terminal transitions atomically remove the inbox entry (Decision 6)
- read_inbox returns all entries without state filtering (invariant holds at lookup time)

### 5. Operation shapes via put_multiple_raw + α.2 ordering

**Decision:** Three canonical storage-operation shapes (dispatch, complete_wake, alarm-fire). Each follows the pattern: pre-checks → sequential reads → compute new state in locals → atomic delete (dispatch's overflow case only) → atomic multi-key `put_multiple_raw` → post-storage in-memory mutation. α.2 ordering (delete first, put_multiple_raw second) in dispatch's overflow path.

**Context:** worker-rs 0.5.0's `transaction(closure)` API has bounds `F: Copy + 'static, returns Result<()>` that make capturing operation data (heap-allocated String/Vec/HashMap/WakeRecord) infeasible. Substitution: per-DO single-writer guarantee provides snapshot consistency within a single method handler; `put_multiple_raw` provides atomic multi-key writes; combined, these give atomicity-equivalent semantics without explicit transaction. α.2 ordering vs α.1 (put first, delete second): α.2 trades a bounded partial-failure case (stale inbox entry handled by defensive skip) for preventing α.1's orphan-wake-row accumulation case.

**Implementation contract:**
- `put_multiple_raw` Object constructed manually via `js_sys::Object::new()` + `Reflect::set` for dynamic-string keys (avoids `serde_wasm_bindgen` HashMap-as-Map serialization gotcha)
- In-memory state mutated ONLY after `put_multiple_raw` success
- Sequential reads precede the multi-key write within each operation
- `transaction(closure)` API explicitly NOT used

Canonical Object construction pattern (inlined at each call site, ~10-15 lines):

```rust
use js_sys::{Object, Reflect};
use wasm_bindgen::JsValue;

let writes = Object::new();
let record_jsv = serde_wasm_bindgen::to_value(&new_record)
    .map_err(|e| StoaError::Wake(WakeError::Other(format!("serialize wake record: {}", e))))?;
Reflect::set(&writes, &JsValue::from_str(&format!("wake:{}", wake_id)), &record_jsv)
    .map_err(|_| StoaError::Wake(WakeError::Other("Reflect::set wake key".to_string())))?;
// ... additional Reflect::set calls for other keys ...
self.state.storage().put_multiple_raw(writes).await
    .map_err(|e| StoaError::Wake(WakeError::Other(format!("put_multiple_raw: {}", e))))?;
```

**Operational notes:**
- Operation ordering within shapes (pre-checks → reads → compute → writes → post-storage) is mandatory; reordering breaks failure-semantics guarantees
- Pre-condition validation discipline: validation failures return Err without touching storage
- Alarm `set_alarm` / `delete_alarm` calls happen post-storage operations
- Resolver operations (insert/resolve/remove from wake_resolvers) happen post-storage success
- Defensive skip on missing wake row in alarm-fire: log warning + skip; don't fail handler
- Defensive skip on missing wake row in read_inbox: log warning + skip; don't include in summaries
- complete_wake's alarm_queue cleanup uses direct computation (`wake.created_at + wake.timeout_ms as u64`) since wake row is read for guards

### 6. Pending→terminal transitions bundle target inbox writes

**Decision:** When any operation transitions a wake's state from Pending to a terminal state (Completed via complete_wake; TimedOut via alarm-fire), the operation's `put_multiple_raw` atomic write includes the target's inbox with the transitioned wake_id removed. Maintains Decision 4's Pending-only invariant.

**Context:** Decision 4 (Pending-only invariant) implies an implementation contract for every operation that transitions Pending→terminal. Originally specified only for alarm-fire (Lock 2.6.10 first draft); during Phase 0 design notes drafting, Layer 5 verification (architectural-decision-to-implementation-contract trace) surfaced that complete_wake's contract was identical but unspecified. Generalization (β.1): one lock covers both transition paths; both operations instantiate the same rule.

**Implementation contract:**
- Alarm-fire's `put_multiple_raw`: N transitioned wake rows + M target inboxes (M = distinct-target count of due wakes) + 1 alarm_queue = N+1+M keys
- complete_wake's `put_multiple_raw`: 1 transitioned wake row + 1 target inbox + 1 alarm_queue = 3 keys

complete_wake's read sequence is: wake row → alarm_queue → target inbox. Target identity comes from the wake row, so the inbox read must follow the wake row read. The reads are sequential (worker-rs storage API has no parallel-read primitive), but the single-writer guarantee makes the sequential read order semantically equivalent to a parallel-snapshot read.

**Operational notes:**
- Alarm-fire batch processing: all due timeouts handled in one alarm-handler invocation
- For typical alarm fires, M=1-2 distinct targets; pathological cases (M>>1) still well within Cloudflare's per-request operation limit
- Alarm-fire's read cost scales with M (distinct targets), not N (total transitioned wakes). A batch-alarm transitioning 50 wakes for the same target reads 1 inbox; transitioning 50 wakes for 50 distinct targets reads 50 inboxes. The latter is bounded by Cloudflare's per-request operation limit (~128); pathological cases approaching this limit should be monitored if observable in production.

### 7. wake_resolvers HashMap shape

**Decision:** `wake_resolvers: HashMap<WakeId, oneshot::Sender<Result<WakeResponse, StoaError>>>` — in-memory map from wake_id to awaiter's `tokio::sync::oneshot::Sender`.

**Context:** dispatch creates a wake and awaits its completion via an in-memory `oneshot::channel`. complete_wake and alarm-fire resolve the wake by sending through the Sender. The map enables O(1) lookup at resolution time. Alternative shapes considered: Vec (O(n) lookup), broadcast channel (multi-receiver semantics — wrong fit since dispatch-await is single-receiver). HashMap is the natural choice.

**Implementation contract:**
- Field on TallyTeamDO struct: `wake_resolvers: HashMap<WakeId, oneshot::Sender<Result<WakeResponse, StoaError>>>`
- Insert (wake_id → Sender) post-storage success in dispatch
- Lookup + resolve + remove post-storage success in complete_wake and alarm-fire

**Operational notes:**
- Inbox waiters signal-only (option c): dispatch post-storage signals `inbox_waiters` listening for the target — a one-line operational signal in this sub-PR; full subscription mechanism is §9.2 scope.
- Resolver operations are best-effort: `Sender::send` returns Err if Receiver dropped; treat as no-op (the awaiter has gone away; no error to propagate).

### 8. WakeRecord 9-field shape + serde_bytes

**Decision:** WakeRecord has 9 fields; payload and response_payload use `#[serde(with = "serde_bytes")]` for binary serialization.

**Context:** Storage schema captures wake lifecycle context: identifying fields (wake_id, target_identity, caller_identity, context_id), payload, state, timing (created_at, timeout_ms), response. serde_wasm_bindgen's default Vec<u8> serialization produces a JSON-style array of integers (~3x larger than binary representation); `serde_bytes` provides binary serialization at the storage layer. Field count is minimal — every field is required by some operation; no fields could be removed without losing functionality.

**Implementation contract:**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeRecord {
    pub wake_id: WakeId,
    pub target_identity: String,        // url-safe-b64 of stoa::Identity
    pub caller_identity: String,        // url-safe-b64 of stoa::Identity
    pub context_id: String,             // UTF-8
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,
    pub state: WakeState,
    pub created_at: u64,                // unix millis
    pub timeout_ms: u32,                // original request value
    #[serde(with = "serde_bytes", default)]
    pub response_payload: Option<Vec<u8>>,
}
```

Storage key: `wake:{wake_id_string}` where `{wake_id_string}` is the ULID's Crockford-base32 representation (26 chars).

### 9. dispatch_with_caller flow + tokio safety timeout

**Decision:** dispatch_with_caller is an inherent method on TallyTeamDO with five parameters; it awaits the oneshot Receiver raced against a `worker::Delay` of `timeout + SAFETY_BUFFER` via `futures::future::select` for belt-and-suspenders safety. The alarm-based timeout is the primary timeout mechanism; the `worker::Delay` race is the safety net.

**Context:** The await on the oneshot Receiver could in principle hang if the Sender is dropped without sending (e.g., DO restart drops in-memory state; implementation bug). The alarm-based timeout fires at `absolute_timeout` and signals the resolver with `Err(StoaError::Wake(WakeError::TimeoutExpired { timeout }))`; this is the canonical timeout path. A `worker::Delay`-based race wraps the await at `timeout + SAFETY_BUFFER` (5 seconds) as a defensive backstop — the alarm-based path is expected to fire first.

**Correction note:** an earlier draft of this design specified `tokio::time::timeout` for the safety wrapper. That API is incompatible with Cloudflare Workers' `wasm32-unknown-unknown` target which has no tokio timer driver — `tokio::time` functions panic at runtime on this target. The substitute `worker::Delay` raced via `futures::future::select` preserves the safety-wrapper semantic with wasm-compatible runtime. Implementation surfaced this during the dispatch sub-PR's stop-and-surface; strategic-layer locked the substitution. The methodology lesson: Layer 4 (representative-use drafting) must verify runtime correctness on the deployment target, not just compile-time correctness.

**Implementation contract:**

```rust
pub(crate) async fn dispatch_with_caller(
    &mut self,
    target: &Identity,
    caller: &Identity,
    context: &[u8],
    payload: WakePayload,
    timeout: Duration,
) -> Result<WakeResponse, StoaError>
```

Flow per Decision 5 (operation shape). Post-storage await step:

```rust
use futures::future::{select, Either};

let total_timeout = timeout + SAFETY_BUFFER;
let delay = worker::Delay::from(total_timeout);

match select(receiver, delay).await {
    Either::Left((result, _delay)) => match result {
        Ok(Ok(response)) => Ok(response),                           // wake completed normally
        Ok(Err(e)) => Err(e),                                        // alarm-fire's TimeoutExpired, etc.
        Err(_recv_error) => Err(StoaError::Wake(WakeError::Other(
            "resolver dropped without resolution (DO restart or bug)".to_string()
        ))),
    },
    Either::Right(((), _receiver)) => Err(StoaError::Wake(
        WakeError::TimeoutExpired { timeout }
    )),
}
```

**Operational notes:**
- handle_dispatch flow: deserialize DispatchRequest → validate (caller, target, payload, timeout) → call dispatch_with_caller → map result to DispatchResponse or error response via existing `stoa_error_to_response`
- Post-storage, call `set_alarm` with the current earliest entry in alarm_queue (or `delete_alarm` if alarm_queue is empty). Always call — don't optimize for "only if changed" since the tracking complexity exceeds the network-call savings.

### 10. WakeRouter::dispatch unimplemented!() with C-A-1 framing

**Decision:** TallyTeamDO's `WakeRouter::dispatch` trait method returns `unimplemented!()` with a doc-comment explaining the trait-vs-persistence-driven-caller-concern split. C-A-1: loud-failure-by-design.

**Context:** stoa's `WakeRouter::dispatch` trait has signature `(target, context, payload, timeout) → Result<WakeResponse, StoaError>` — no caller param. TallyTeamDO's persistence layer (WakeRecord storage per Decision 8) requires the caller's identity. The impedance is resolved at the implementation surface (inherent method `dispatch_with_caller` carries caller; trait impl is unreachable). Alternative considered (C-A-2): return error stub (`Err(...)`). Rejected because the semantic of "this trait method should not be called on this implementation" is better expressed as a programming-error panic than as a runtime error — calling it indicates an architectural mistake in the caller, not a runtime failure.

**Implementation contract:**

```rust
async fn dispatch(
    &self,
    _target: &Identity,
    _context: &[u8],
    _payload: WakePayload,
    _timeout: Duration,
) -> Result<WakeResponse, StoaError> {
    unimplemented!("TallyTeamDO::dispatch: use dispatch_with_caller (caller identity required for persistence)")
}
```

Doc-comment explains the trait-vs-persistence split and notes that if stoa's trait grows a caller param in the future, this impl shifts from `unimplemented!()` to a thin wrapper over `dispatch_with_caller`.

### 11. handle_read_inbox immediate-read in scope

**Decision:** handle_read_inbox is in dispatch sub-PR scope, implementing the immediate-read path. The long-poll trigger (block until `inbox_waiters` signals) is deferred to §9.2 sub-PR.

**Context:** Dispatched wakes need to be observable to their targets for the end-to-end dispatch + complete cycle to be testable and operationally useful. Originally scoped this to §9.2 ("producer side first; consumer side later"); strategic-layer review pushed back: complete-but-unobservable lifecycle is artificial scope boundary. Immediate-read implementation is ~25-30 lines using the existing handler pattern; the long-poll trigger is the only piece with genuine §9.2-scope design questions (subscription mechanism, identity routing).

**Implementation contract:**

```rust
async fn handle_read_inbox(&mut self, req: &Request) -> Result<Response> {
    // Parse identity from query param: /inbox?identity=...&wait_seconds=...&limit=...
    let identity = parse_identity_from_query(req)?;

    let inbox_key = format!("agent:{}:inbox", identity.to_url_safe_b64());
    let inbox: VecDeque<WakeId> = self.state.storage()
        .get(&inbox_key).await.unwrap_or_default();

    let mut summaries = Vec::with_capacity(inbox.len());
    for wake_id in inbox.iter() {
        let wake_key = format!("wake:{}", wake_id);
        match self.state.storage().get::<WakeRecord>(&wake_key).await {
            Ok(wake) => summaries.push(WakeSummary {
                wake_id: wake_id.to_string(),
                caller_identity_b64: wake.caller_identity,
                context_id: wake.context_id,
                payload_b64: base64::encode_url_safe(&wake.payload),
            }),
            Err(_) => {
                // Defensive skip per Decision 5 Operation shapes §α.2 partial-failure case.
                // Inbox entry references a wake row that no longer exists; likely caused
                // by dispatch's overflow-handling partial failure (delete succeeded;
                // put_multiple_raw failed). Bounded by α.2 partial-failure rate.
                tracing::warn!(
                    wake_id = %wake_id,
                    "inbox references missing wake row (likely α.2 partial-failure orphan)"
                );
            }
        }
    }

    Response::from_json(&ReadInboxResponse { wakes: summaries })
}
```

Long-poll trigger: `wait_seconds` parameter accepted but ignored in dispatch sub-PR scope (returns immediately regardless).

**Operational notes:**
- Defensive skip on missing wake row preserves Decision 4's invariant under α.2 partial-failure cases

## Wire-format API contracts

### DispatchRequest

```rust
#[derive(Debug, Deserialize)]
pub struct DispatchRequest {
    pub caller_identity_b64: String,
    pub target_identity_b64: String,
    pub context_id: String,
    pub payload_b64: String,
    pub timeout_ms: u32,
}
```

Validation at handler entry:
- `caller_identity_b64` and `target_identity_b64` parse via `Identity::from_url_safe_b64`
- `payload_b64` valid base64; decoded length ≤ `MAX_PAYLOAD_BYTES`
- `timeout_ms` in `[MIN_TIMEOUT_MS, MAX_TIMEOUT_MS]`
- `context_id` non-empty UTF-8

Validation failures map to `StoaError::Wake(WakeError::DispatchRefused { reason })` or `InvalidTimeout` → HTTP 400 via `stoa_error_to_response`.

### CompleteRequest

```rust
#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub by_identity_b64: String,
    pub wake_id: String,
    pub response_payload_b64: String,
}
```

Validation: `by_identity_b64` parses; `wake_id` parses as ULID (26 chars Crockford-base32); `response_payload_b64` valid base64 with decoded length ≤ `MAX_RESPONSE_BYTES`.

### DispatchResponse

```rust
#[derive(Debug, Serialize)]
pub struct DispatchResponse {
    pub responding_identity_b64: String,  // identity that called complete_wake (= wake.target in MVP)
    pub response_payload_b64: String,
}
```

HTTP 200 on success. Body is JSON-serialized DispatchResponse.

Error response cases (handled via existing `stoa_error_to_response`):
- HTTP 504 `TimeoutExpired`: alarm-based or tokio safety timeout fired
- HTTP 400 `DispatchRefused`: pre-storage validation, inbox full, etc.
- HTTP 400 `InvalidTimeout`: timeout outside `[MIN_TIMEOUT_MS, MAX_TIMEOUT_MS]`
- HTTP 404 `HandlerNotFound`: target identity has no registered handler for context
- HTTP 500 `Other`: storage failure, invariant violation, internal errors

## System properties

Properties of the implemented system, documented for operational awareness:

- **Eviction handling implicit.** Cloudflare DO eviction drops in-memory state (wake_resolvers HashMap). Storage persists; alarm fires after rehydration; pending wakes route to TimedOut. Awaiters' HTTP connections drop on eviction; caller retries via fresh dispatch.

- **Failure semantics per operation.** Per Decision 5, in-memory state is mutated only after `put_multiple_raw` success. Failures leave in-memory state consistent with last-successful storage state. Retry semantics: dispatch retry-safe via scan-based overflow lookup; complete_wake retry-safe via state guard (wake.state == Pending); alarm-fire retry-safe via state guard + Cloudflare alarm-retry mechanism.

- **Snapshot consistency via single-writer.** Cloudflare DO's per-instance single-writer guarantee provides snapshot consistency across reads within a single method handler, even across `.await` boundaries. No explicit transaction needed for read-then-write atomicity.

- **Resolver best-effort.** `oneshot::Sender::send` returns Err if Receiver dropped; treated as no-op. The Receiver going away means the awaiter has gone away; the storage state is the source of truth.

- **Retry cost bounded.** Cloudflare alarm-retries are idempotent via state guards (skip already-terminal wakes); cost is bounded by retry-count limits and exponential backoff.

- **Alarm precision 0-1s.** Cloudflare alarm scheduling has 0-1s latency. Acceptable for the 1-300s timeout range.

## Dependencies + constants

**Direct dependency additions:**
- `serde_bytes` — binary serialization for WakeRecord.payload and response_payload (Decision 8)
- `ulid` — WakeId generation; wasm32-compatible via web-time crate (Decision 3)
- `base64` — HTTP transport b64 encoding/decoding (handlers); may already be transitive
- `tokio` features `sync` (oneshot::channel). The `time` feature is intentionally excluded: `tokio::time::timeout` panics at runtime on `wasm32-unknown-unknown` (no timer driver). See Decision 9 correction note.
- `futures` — `future::select` + `Either` to race the oneshot Receiver against `worker::Delay` (Decision 9 safety wrapper)

**Constants** (placement: `tally-worker/src/dispatch_consts.rs`):

```rust
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

/// Safety buffer on dispatch's worker::Delay safety wrapper (Decision 9).
pub const SAFETY_BUFFER: Duration = Duration::from_secs(5);
```

**Deployment requirement:** Cloudflare Workers Paid plan minimum. The Free plan's 10ms CPU limit is incompatible with even minimal dispatch handler invocations.

## Scope boundaries

**In scope (this sub-PR):**
- `handle_dispatch` + `dispatch_with_caller` (Decisions 9, 11 of operational discipline)
- `handle_complete_wake` + `complete_wake` (Decision 5 operation shape; Decision 6 transition contract)
- Alarm-fire transition logic (Decisions 1, 5, 6)
- `handle_read_inbox` immediate-read path (Decision 11)
- `WakeRouter::dispatch` trait impl with C-A-1 framing (Decision 10)
- All storage operations per Decision 5's shapes
- WakeRecord storage schema (Decision 8)
- DispatchRequest / CompleteRequest / DispatchResponse wire-format types
- Constants in `dispatch_consts.rs`

**Deferred to §9.2 (HTTP API surface sub-PR):**
- `handle_read_inbox` long-poll trigger via `inbox_waiters` subscription
- Identity-source resolution conventions (query-param choice here is dispatch sub-PR's compromise)

**Deferred to §9.3 (integration tests sub-PR):**
- End-to-end dispatch + complete + timeout scenarios via HTTP
- Multi-target, multi-context test fixtures
- Alarm-fire trigger patterns for tests

## Methodology carry-forward

The dispatch sub-PR deliberation surfaced verification disciplines worth carrying forward to future Pattern C work:

### Five-layer verification of architectural commitments

Originally framed as four layers (verifying API surfaces before locking); Phase 0 design notes drafting surfaced a fifth layer.

**Layers 1-4 (API surface verification):**

1. **API existence:** signature exists; can be called
2. **API contract surface:** signature shape; trait bounds; capture types
3. **API serialization/protocol surface:** data shapes compatible with underlying contract
4. **Representative-use drafting:** actually drafting the API call against the operation's data shapes

Layers 1-3 are properties of the API that need to hold; Layer 4 is the operative discipline that forces 1-3 to be checked concretely.

**Layer 5 (architectural-decision-to-implementation-contract trace):**

For each architectural decision, enumerate every implementation operation that could violate it; verify each operation's lock fully specifies how the decision is maintained. The trace itself is the verification.

The four-layer framing is about external surfaces (API contracts; what your code calls). Layer 5 is about internal surfaces (architectural invariants; what your code maintains). Both apply at different scopes.

Worked example from this sub-PR: Decision 4 (Pending-only invariant) was traced through complete_wake's transition path during Phase 0 drafting; this surfaced a gap (Lock 6.6.4 didn't include target inbox cleanup) that Layer-1-4 verification missed. Resolution: Decision 6 generalized to cover both transition paths.

**When to apply Layer 5:** for each architectural decision introduced during Pattern C deliberation, list every operation that could violate it. Then for each operation's lock, verify the operation's contract specifies how the decision is maintained. The trace is the verification — if any operation's contract doesn't address the decision, that's the Layer 5 gap to surface.

Pattern C deliberations involving invariants (state machines, atomicity guarantees, consistency properties) particularly benefit from Layer 5 verification before design notes drafting. Decisions affecting only external API surface (wire-format types, public method signatures) are typically Layer 1-4 territory.

### Pattern C deliberation cadence

Pre-implementation lock-then-execute discipline. Items deliberated in dependency-graph order; each item progresses through inspection → finding surfacing → verification queries → strategic-layer triage → locks. The cadence trades deliberation time for execution clarity.

Pattern C originated during the TallyTeamDO state model Phase 0 deliberation (predecessor to this sub-PR). The five-category stop-and-surface framework + Pattern α reference structure emerged in the Workstream B''' substrate revision arc. This sub-PR's deliberation extended Pattern C with the four-layer API surface verification (transaction-API + put_multiple-serialization findings) and Layer 5 architectural-decision-to-implementation-contract trace (Decision 6 gap finding).

### Stop-and-surface discipline

When deviation from locked commitment surfaces during execution, stop and surface explicitly. Multi-cycle iteration is fine; the cycle count is observation, not judgment. Surfacing the gap in Decision 6 (during Phase 0 drafting) is an instance of Layer 5 verification applied via stop-and-surface discipline.

### Bidirectional scrutiny of pushback

When pushback surfaces on framing, briefly scrutinize whether the pushback might also have a gap before converging. This sub-PR's deliberation had several instances of bidirectional convergence (handle_read_inbox scope reversal; absolute_timeout restoration) — each ended in agreement, but the scrutiny step was load-bearing.

### Categorization pass before design notes

For multi-decision Pattern C deliberations, run a categorization pass (load-bearing vs operational vs documentation-of-consequences) before drafting design notes. This sub-PR's categorization reduced 41 locks → 11 architectural commitments + 3 wire-format contracts + operational discipline distributed under each architectural decision. The categorization surfaced miscategorizations (L8, L13) that would otherwise have lived as noise in the design notes.

## Implementation PR scope

Files modified/created (estimated):

- New module for WakeRecord, WakeState, WakeId types (placement: `tally-worker/src/wake_types.rs` or extension of `tally-worker/src/rpc.rs`; implementation decision)
- `tally-worker/src/rpc.rs`: `DispatchRequest`, `CompleteRequest`, `DispatchResponse` additions
- `tally-worker/src/durable_object.rs`: `dispatch_with_caller`, `complete_wake` inherent methods; `handle_dispatch`, `handle_complete_wake`, `handle_read_inbox` handler wire-up; alarm handler
- `tally-worker/src/wake_router.rs`: `WakeRouter::dispatch` impl refined doc-comment (per Decision 10)
- `tally-worker/src/dispatch_consts.rs` (new): constants per Dependencies + constants section
- `tally-worker/Cargo.toml`: dependency additions (`serde_bytes`, `ulid`, `base64`, `tokio` feature `time`)

Estimated diff: ~600-900 lines of substantive code plus ~150-250 lines of unit tests (integration tests deferred to §9.3 sub-PR; in-scope unit tests cover: type roundtrip serialization, constant validation, WakeId Crockford-base32 round-trip).

## Open questions for future sub-PRs

### Deferred to §9.2 HTTP API surface sub-PR

- `handle_read_inbox` long-poll trigger via `inbox_waiters` subscription mechanism
- Identity-source resolution conventions (current query-param choice from §9.1 may be revised)

### Deferred to §9.3 integration tests sub-PR

- End-to-end dispatch + complete + timeout scenarios via HTTP
- Multi-target, multi-context test fixtures
- Alarm-fire trigger patterns for tests

### Future stoa-side protocol work

- `WakeRouter::dispatch` trait may grow a caller param; TallyTeamDO's Decision 10 `unimplemented!()` shifts to thin wrapper over `dispatch_with_caller` at that point

### Operational monitoring (Phase 1+)

- Decision 5's invariant-violation log signal (scan-not-found in dispatch overflow): if these accumulate, indicates inbox/alarm_queue divergence — alert worth wiring
- Storage growth: TimedOut and Completed wake rows persist without GC in this sub-PR; if accumulation becomes operationally observable, GC operation may be needed
- Cloudflare DO operations-per-request limit: M-many target inbox writes in alarm-fire (Decision 6); typical M=1; pathological cases (M>>1) may approach Cloudflare's per-request operation count limit (~128 documented)
