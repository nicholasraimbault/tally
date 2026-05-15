# HTTP API surface sub-PR — Phase 0 design notes

**Date:** 2026-05-15
**Scope:** tally Phase 0 §9.2 — public HTTP API surface, Worker fetch handler, Bearer auth, long-poll trigger
**Status:** Pre-implementation. Architectural commitments locked through Pattern C deliberation.
**Provenance:** Pattern C deliberation (one session). Methodology carry-forward: see §9.1 design notes (`dispatch-sub-pr-phase-0.md`). No new methodology lessons surfaced.

## Summary

Implements the public HTTP API surface fronting TallyTeamDO. In scope:

- Worker fetch handler with declarative routing (`worker::Router`) for the 6 public routes per `phase-1b-sub-pr-1-phase-0.md` §3.3
- Bearer auth via Worker→DO `validate_api_key` RPC; identity resolution from Bearer
- Long-poll trigger via `inbox_waiters` subscription (single-waiter per identity)
- §3.1 error code mapping corrections + new `TallyError` wrapper covering complete_wake's tally-specific cases (refines §9.1's Decision 6.6.4)
- `?identity=` query-param removal from `handle_read_inbox` (Decision 11 of §9.1 was an explicit deferred compromise)

Deferred:
- §9.3 integration tests sub-PR: end-to-end HTTP flow tests
- Phase 2 admin tooling: real API key validation (this PR keeps the MVP uniform-true pattern but establishes the validate_api_key RPC seam)

The design rests on 4 architectural decisions; mechanical items follow §3.3 + §3.8 spec verbatim.

## Architectural decisions

### 1. Auth flow via Worker→DO validate_api_key RPC

**Decision:** Each authenticated request triggers a Worker→DO `validate_api_key` RPC. The DO returns `valid + identity_b64` (MVP: uniform-true with bearer-as-identity; Phase 2: real key validation lookup against `agent:{identity_b64}:api_keys`).

**Context:** §3.3 specifies "Authorization Bearer must validate against `agent:{identity_b64}:api_keys`" — implying DO-side validation. §4.4 commits MVP to uniform-true validation. The Worker→DO RPC pattern establishes the seam for Phase 2's real validation; MVP's behavior is a thin placeholder rather than a different pattern.

Alternatives considered:
- Bearer-as-identity with Worker-side parsing only; no DO RPC. Simpler MVP but harder Phase 2 transition (Bearer semantics change from "is identity" to "is opaque token").
- JWT-style self-contained tokens. Adds signing infrastructure beyond MVP scope.

Chose: Worker→DO RPC pattern. One round-trip per request (DO is co-located with Worker; ~5ms). Phase 2 transition: `validate_api_key`'s DO-internal logic changes; Worker contract stable.

**Implementation contract:**

```rust
// Wire shape (internal Worker→DO RPC; not public)
#[derive(Debug, Deserialize)]
pub struct ValidateApiKeyRequest {
    pub bearer: String,  // raw Bearer header value
}

#[derive(Debug, Serialize)]
pub struct ValidateApiKeyResponse {
    pub valid: bool,
    pub identity_b64: Option<String>,  // populated when valid
}
```

Worker per-request flow:
1. Parse Bearer from `Authorization: Bearer <token>` header; if missing → 401
2. Call DO `validate_api_key` with Bearer
3. If `valid: false` → 401
4. Extract identity from response
5. If route has URL `{identity}` path param: check it equals authenticated identity; else → 403
6. Forward to actual DO RPC with authenticated identity

MVP DO behavior: attempts `url_safe_b64_decode(bearer)`; on success returns `valid: true, identity_b64: Some(decoded_b64)`; on parse failure returns `valid: false, identity_b64: None`. Phase 2 replaces the parse-as-identity logic with real key lookup against `agent:{identity_b64}:api_keys`; wire contract stable across the transition.

### 2. inbox_waiters subscription mechanism

**Decision:** Single-waiter per identity via `inbox_waiters: HashMap<Identity, oneshot::Sender<()>>`. Subscribe-first ordering in `handle_read_inbox`; signal-side drains the entry on dispatch's post-storage step.

**Context:** §3.8 specifies "the DO maintains an in-memory wait list keyed by identity. When step 7's `agent:{target_b64}:inbox` append happens, the DO resolves all waiting inbox calls for that identity." Lock 4.6.5 from §9.1 framed this as "signal-only (option c)"; §9.1's PR #17 added a no-op signal seam in `dispatch_with_caller` (durable_object.rs:599-602).

Alternatives considered:
- Multi-waiter per identity (`Vec<Sender>`). MVP usage is one MCP plugin per identity polling its inbox; multi-waiter is a configuration mistake more than a designed-for case. YAGNI; can be added later as non-breaking change.
- `tokio::sync::broadcast` channels. Long-lived per-identity state; wasm32 compatibility unverified — Layer 4 lesson applied (extension from §9.1's tokio::time finding).

Chose: single-waiter with `tokio::sync::oneshot`. Simpler lifecycle; oneshot already verified wasm32-compatible via §9.1's `wake_resolvers` usage.

**Implementation contract:**

```rust
// TallyTeamDO struct addition
pub(crate) inbox_waiters: HashMap<Identity, oneshot::Sender<()>>,
```

`handle_read_inbox` subscribe-first flow when `wait_seconds > 0` and inbox is empty after initial read:

```rust
let (tx, rx) = oneshot::channel();
self.inbox_waiters.insert(identity.clone(), tx);  // replaces any existing

// Re-read AFTER insert to catch the case where dispatch appended to
// inbox between our initial read and our insert. Dispatch's signal-side
// wouldn't find our entry yet, so the notification would be lost; the
// re-read catches the data directly.
let inbox = read_inbox(&identity);
if !inbox.is_empty() {
    self.inbox_waiters.remove(&identity);
    return populated(inbox);
}

let delay = worker::Delay::from(Duration::from_secs(wait_seconds.into()));
match futures::future::select(rx, delay).await {
    Either::Left((Ok(()), _)) => populated(read_inbox(&identity)),  // signal fired
    Either::Left((Err(_recv_error), _)) => populated(read_inbox(&identity)),  // replaced by new subscriber; re-read
    Either::Right(((), _)) => {
        self.inbox_waiters.remove(&identity);  // unconditional remove
        empty()
    }
}
```

Signal-side (replaces no-op comment in dispatch_with_caller):

```rust
if let Some(sender) = self.inbox_waiters.remove(target) {
    // Best-effort signal. Err(()) means the Receiver was dropped
    // (subscriber timed out or was replaced); not an error to log.
    let _ = sender.send(());
}
```

**Operational notes** (load-bearing implementation detail):

- **Subscribe-first ordering is mandatory.** Read-then-subscribe has a race window where a signal fires between read and subscribe; subscribe-first ensures signals fired post-subscribe are buffered in the oneshot Receiver.
- **Unconditional cleanup with RecvError absorption.** If A times out and removes the entry while B has just subscribed, A's remove drops B's Sender; B's await receives RecvError and routes to the re-read path. The race-with-new-subscriber is bounded and gracefully degrades. Token-check would over-engineer the rare case.

### 3. Remove `?identity=` query param from handle_read_inbox

**Decision:** `handle_read_inbox` no longer accepts `?identity=` query param. Identity comes from authenticated request only.

**Context:** §9.1's Decision 11 used query-param identity as an explicit deferred compromise pending §9.2's identity-source resolution. The compromise is resolved here: identity flows from Bearer through Worker → DO RPC. The Worker layer extracts the URL path's `{identity}` segment (per §3.3 `GET /v1/teams/{team_id}/agents/{identity}/inbox`) and checks it equals authenticated identity; mismatch → 403 per §3.1.

**Implementation contract:**

- Worker: extracts `{identity}` from URL path; forwards to DO with authenticated identity
- DO `handle_read_inbox`: receives identity as a routing parameter (not query param); current `?identity=` parsing block is removed
- Behavior: a request without proper auth gets 401; a request with auth but mismatched URL identity gets 403; a request with matched auth + identity gets the read result

### 4. TallyError wrapper for complete_wake's tally-specific cases

**Decision:** `tally-worker` introduces a new error type `TallyError` covering complete_wake's implementation-specific cases. `complete_wake`'s return type revises from `Result<(), StoaError>` to `Result<(), TallyError>`.

**Context:** §9.2's error-code-mapping inspection surfaced that §9.1's `complete_wake` (per Lock 6.6.4) misuses stoa's `WakeError` variants for non-dispatch cases:
- "wake row not found" maps to `WakeError::HandlerNotFound` (dispatch-scoped variant per stoa's doc-comment)
- "wake not in Pending state" maps to `WakeError::DispatchRefused` (dispatch-scoped variant)
- "by_identity mismatch" maps to `WakeError::DispatchRefused`

stoa's `WakeError` is doc-commented as "Errors from `WakeRouter::dispatch`" — all 5 variants are dispatch protocol concerns. `complete_wake` isn't in stoa's trait surface at all; it's a tally inherent method. Stretching `WakeError` to cover complete_wake's cases erodes the protocol-vs-implementation boundary.

Alternatives considered:
- Add `WakeError::AlreadyTerminal` (and others) to stoa. Cross-repo coordination cost; protocol-vs-implementation boundary erosion (stoa would become "all wake errors" rather than "dispatch errors").
- Use `WakeError::Other(String)` with substring inspection. Brittle.

Chose: tally-side `TallyError` wrapper. stoa stays dispatch-scoped; tally absorbs implementation-specific cases.

**Implementation contract:**

```rust
// tally-worker/src/error.rs (new)
use stoa::StoaError;

#[derive(Debug, thiserror::Error)]
pub enum TallyError {
    /// Pass-through for stoa's dispatch-scoped errors.
    #[error(transparent)]
    Stoa(#[from] StoaError),

    /// Wake row not found in storage. Distinct from
    /// `HandlerNotFound` which means handler eligibility.
    #[error("wake not found")]
    WakeNotFound,

    /// complete_wake called on a wake already in terminal state.
    /// Caller is attempting duplicate completion.
    #[error("wake already in terminal state")]
    AlreadyTerminal,

    /// complete_wake's by_identity doesn't match wake's target_identity.
    #[error("identity does not match wake target")]
    IdentityMismatch,
}
```

`complete_wake`'s three error sites in durable_object.rs revise (replacing fudged WakeError variants):
- `"wake row not found"` → `TallyError::WakeNotFound` (was `HandlerNotFound`)
- `"wake not Pending"` → `TallyError::AlreadyTerminal` (was `DispatchRefused`)
- `"by_identity mismatch"` → `TallyError::IdentityMismatch` (was `DispatchRefused`)

`dispatch_with_caller`'s return type stays `Result<WakeResponse, StoaError>` — all its errors are protocol-level.

**Honest acknowledgment:** this refines §9.1's Decision 6.6.4, which used StoaError throughout complete_wake. The §9.1 deliberation didn't catch the WakeError misuse; §9.2's error-mapping inspection did. The correction lands as part of §9.2's PR rather than as a separate retroactive correction to §9.1's merged code.

## Wire-format API contracts (per §3.3 verbatim)

The 6 public routes:

| Method | Path | Backing DO RPC |
|---|---|---|
| POST | `/v1/teams/{team_id}/agents/{identity}/register` | `POST /register` |
| DELETE | `/v1/teams/{team_id}/agents/{identity}/handlers/{context_id}` | `POST /unregister` |
| POST | `/v1/teams/{team_id}/wakes` | `POST /dispatch` |
| GET | `/v1/teams/{team_id}/agents/{identity}/inbox` | `GET /inbox` |
| POST | `/v1/teams/{team_id}/wakes/{wake_id}/complete` | `POST /complete` |
| GET | `/v1/health` | (Worker-only; no DO) |

Request/response JSON shapes are specified in `phase-1b-sub-pr-1-phase-0.md` §3.3 and locked verbatim — not restated here. The Worker layer translates public HTTP requests into the internal DO request shapes (which are §9.1's `RegisterRequest`, `UnregisterRequest`, `DispatchRequest`, `CompleteRequest` plus the new `ValidateApiKeyRequest`).

### Error code mapping

Per §3.1 (single canonical mapping for both `stoa_error_to_response` corrections and the new `tally_error_to_response`):

| HTTP | Source | Variant |
|---|---|---|
| 400 | Worker / WakeError | Malformed request; `InvalidTimeout` |
| 401 | Worker | Missing or invalid Bearer |
| 403 | Worker | URL identity ≠ authenticated identity; `IdentityMismatch` |
| 404 | Worker / TallyError | Team not found; `WakeNotFound` |
| 408 | WakeError | `TimeoutExpired` *(corrected from §9.1's 504)* |
| 410 | TallyError | `AlreadyTerminal` *(new)* |
| 422 | WakeError | `HandlerNotFound` *(corrected from §9.1's 404)*; `DispatchRefused` *(corrected from §9.1's 400)* |
| 500 | WakeError / catchall | `Other`; internal runtime errors |

§9.1's `stoa_error_to_response` mapping is updated for HandlerNotFound, DispatchRefused, and TimeoutExpired corrections. New `tally_error_to_response` covers TallyError variants, delegating `TallyError::Stoa(_)` to the corrected `stoa_error_to_response`.

### Pagination + long-poll (per §3.8)

- `limit`: default 10, max 100
- `more_available: bool` in response when inbox has more entries than limit
- ULID-sortable ordering; no cursor needed
- `wait_seconds`: default 0, max 30

New constants (placement: extend `dispatch_consts.rs` rather than creating new `http_consts.rs`):

```rust
pub const DEFAULT_WAIT_SECONDS: u32 = 0;
pub const MAX_WAIT_SECONDS: u32 = 30;
pub const DEFAULT_LIMIT: usize = 10;
pub const MAX_LIMIT: usize = 100;
```

Compile-time invariants extended:

```rust
const _: () = {
    assert!(MAX_WAIT_SECONDS > DEFAULT_WAIT_SECONDS);
    assert!(MAX_LIMIT >= DEFAULT_LIMIT);
};
```

## System properties

Inherits §9.1's system properties (eviction handling implicit; snapshot consistency via single-writer; resolver best-effort; retry cost bounded; alarm precision 0-1s; failure semantics per operation).

§9.2-specific additions:
- **Inbox-waiter best-effort signal.** `sender.send(())` on a dropped Receiver returns `Err`; treated as no-op (matches Lock 4.6.3's resolver discipline).
- **Auth round-trip latency.** Every authenticated request adds one Worker→DO RPC (`validate_api_key`); ~5ms in MVP since DO is co-located. Acceptable; not a bottleneck.

## Scope boundaries

**In scope:**
- 6 public HTTP routes per §3.3
- Worker fetch handler with `worker::Router`
- Bearer auth + Worker→DO `validate_api_key` flow
- `ValidateApiKeyRequest`/`ValidateApiKeyResponse` shape change (internal RPC; no public consumers)
- `inbox_waiters` single-waiter mechanism (signal-side + subscription-side)
- `?identity=` query-param removal from `handle_read_inbox`
- `TallyError` type + `complete_wake` return-type revision (§9.1 Decision 6.6.4 refinement)
- §3.1 error code mapping corrections in `stoa_error_to_response`
- New `tally_error_to_response` for TallyError variants
- Health endpoint
- Worker-layer error mapping for auth/route errors (401/403/404)
- New constants (DEFAULT/MAX wait_seconds + limit)

**Deferred to §9.3** (integration tests sub-PR):
- End-to-end HTTP flow tests via `wrangler dev`

**Deferred to Phase 2:**
- Real API key validation (uniform-true MVP placeholder remains; behavior change is DO-internal)

## Implementation PR scope

Files modified/created:
- `tally-worker/src/lib.rs`: replace placeholder fetch with full `worker::Router` setup (6 routes); Worker-side auth flow + error mapping
- `tally-worker/src/error.rs` *(new)*: `TallyError` type + `From<StoaError>` impl
- `tally-worker/src/durable_object.rs`: add `inbox_waiters` field; subscribe-first wiring in `handle_read_inbox`; signal-side in `dispatch_with_caller`; `complete_wake` return type + error site revisions; remove `?identity=` parsing; `validate_api_key` handler updated to new shape; `stoa_error_to_response` §3.1 corrections; new `tally_error_to_response`
- `tally-worker/src/rpc.rs`: `ValidateApiKeyRequest`/`Response` shape change
- `tally-worker/src/dispatch_consts.rs`: add wait/limit constants + compile-time invariants

Estimated diff: ~800-1200 lines substantive code; ~150-250 lines unit tests (worker-layer auth happy/failure paths; inbox_waiters subscribe/signal/timeout; pagination; TallyError mapping).

## Open questions for future sub-PRs

### Deferred to §9.3
- End-to-end HTTP flow tests under `wrangler dev`

### Deferred to Phase 2
- Real API key validation logic (replace MVP uniform-true bearer-as-identity)
- Bearer token format (currently MVP: bearer = `url_safe_b64(identity_bytes)`; Phase 2 defines opaque tokens)

### Operational monitoring (Phase 1+)
- Auth round-trip latency to DO (one extra RPC per authenticated request — flag if observable)
- `inbox_waiters` HashMap entry count per DO (monitor for runaway growth; should be bounded by active polling clients)
