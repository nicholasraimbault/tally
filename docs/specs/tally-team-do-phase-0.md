# TallyTeamDO State Model — Phase 0 Design Notes
**Status**: Phase 0 draft, awaiting strategic-layer approval. Lives at `tally/docs/specs/tally-team-do-phase-0.md` once approved.
**Eventual home**: `tally/docs/specs/tally-team-do-phase-0.md` (after Phase 0 PR merges).
**Scope**: The TallyTeamDO state model implementation — register_handler / unregister_handler / validate_api_key / read_inbox / team:meta lazy initialization. Dispatch and complete_wake are explicitly deferred to a subsequent PR. This document covers everything in TallyTeamDO's state model except the dispatch flow.
**Authority**: This document supersedes the speculative TallyTeamDO descriptions in `tally/docs/specs/phase-1b-sub-pr-1-phase-0.md` §3.2 and §3.6 where the two conflict. §3.2/§3.6 specified TallyTeamDO at the storage-key-name level; this document pins implementation shape at the Rust-type and operational-pattern level.
**References**:
- Sub-PR 1 Phase 0 design notes: `tally/docs/specs/phase-1b-sub-pr-1-phase-0.md` (tally main `faa3073` or later)
- Stoa WakeRouter trait: `skytale/stoa/src/wake_router.rs` at master `94941a5` or later (post-Workstream-B' + WakeError docs cleanup)
- Phase 1B tracking issue: nicholasraimbault/skytale#444
## 1. What this PR builds
The TallyTeamDO state model implementation. A Cloudflare Durable Object that holds per-team state and implements Stoa's WakeRouter trait. Specifically:
- The `TallyTeamDO` struct with `#[durable_object]` macro annotation
- `impl WakeRouter for TallyTeamDO` covering register_handler, unregister_handler, and dispatch (the last as scope-boundary stub)
- The DO's `fetch` handler routing internal RPC paths to handler methods
- Read-only `read_inbox` returning empty list (no dispatch yet means no inbox writes; reads gracefully empty)
- `validate_api_key` returning true uniformly (auth genuinely deferred to Phase 2; not pretending via 401)
- Lazy `team:meta` initialization on first request
- Internal RPC types in `tally-worker/src/rpc.rs`
- Shared storage types (WakeRecord, TeamMeta) in `tally-core`
**Explicitly NOT in scope:**
- TallyTeamDO::dispatch implementation (stub via `unimplemented!()`)
- complete_wake (deferred with dispatch)
- inbox writes (deferred with dispatch)
- api_keys write-path (Phase 2 admin tooling)
- Audit trail writes (deferred entirely, not Phase 1B)
- HTTP API surface implementation in worker fetch handler (separate sub-PR)
- Integration tests against `wrangler dev` (separate sub-PR)
## 2. Architecture overview
### 2.1 Where things live
```
tally-core/src/
├── lib.rs                  # re-exports
├── wake_record.rs          # WakeRecord, WakeState (storage types for dispatch-era)
└── team_meta.rs            # TeamMeta struct
tally-worker/src/
├── lib.rs                  # Worker #[event(fetch)] entry; routes to DO
├── durable_object.rs       # TallyTeamDO struct + #[durable_object] + fetch handler
├── wake_router.rs          # impl WakeRouter for TallyTeamDO
└── rpc.rs                  # internal Worker↔DO request/response types
```
Justification for the split: `tally-core` stays Cloudflare-agnostic; types here will be referenced by the Tally CLI (Sub-PR 3) and any future non-Cloudflare consumers. `tally-worker` is the Cloudflare-specific implementation; RPC types live here because they describe internal Worker↔DO traffic shape, which is Cloudflare-specific.
### 2.2 What state lives in TallyTeamDO
TallyTeamDO holds only `state: State` (worker-rs's DO state handle). No other fields day-1.
Cloudflare Durable Objects have a single-writer guarantee: all method calls on one DO instance serialize through the runtime. No `Mutex<>` or `RwLock<>` around fields is needed; `&mut self` async methods are safe by construction.
Future addition during dispatch PR: `wake_resolvers: HashMap<WakeId, tokio::sync::oneshot::Sender<Result<WakeResponse, StoaError>>>`. Out of this PR's scope.
### 2.3 Storage schema
DO naming key per Sub-PR 1 Phase 0 design notes §3.2: `${tenancy_prefix}:${team_id_url_safe_b64}`. MVP tenancy_prefix is `"tally-cli-local"` (constant).
| Storage key | Rust type | Written day-1? | Read day-1? |
|---|---|---|---|
| `team:meta` | `TeamMeta` (typed struct) | Yes (lazy init on first request) | Yes (every request) |
| `agent:{identity_b64}:handlers` | `BTreeSet<String>` (context_ids) | Yes (register/unregister) | Yes (register/unregister/dispatch) |
| `agent:{identity_b64}:inbox` | `VecDeque<String>` (wake_ids) | No (dispatch writes; not yet) | Yes (read_inbox; gracefully empty) |
| `agent:{identity_b64}:api_keys` | `BTreeSet<String>` (api_key_hash) | No (Phase 2 admin tooling) | Yes (validate_api_key, returns true uniformly in MVP) |
| `wake:{wake_id}` | `WakeRecord` (typed struct) | No (dispatch writes; not yet) | No (dispatch reads; not yet) |
| `audit:{event_ulid}` | reserved | No (deferred entirely) | No |
`BTreeSet` chosen over `HashSet` for deterministic ordering (useful for debugging; small storage cost for the expected sizes). `VecDeque` chosen over `Vec` for O(1) front-drain matching FIFO semantics; serde roundtrip works cleanly via serde_json's standard `VecDeque` support.
### 2.4 Type definitions (`tally-core/src/wake_record.rs`)
```rust
use serde::{Deserialize, Serialize};
/// Persistent record of a wake's lifecycle. One row per wake_id under 
/// the `wake:{wake_id}` storage key. Written by dispatch (deferred to 
/// a subsequent PR); read by dispatch and complete_wake.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WakeRecord {
    pub caller_identity_b64: String,
    pub target_identity_b64: String,
    pub context_id: String,
    pub payload_b64: String,
    pub timeout_ms: u32,
    pub state: WakeState,
    pub response_b64: Option<String>,
    pub created_at: i64,  // unix milliseconds
    pub completed_at: Option<i64>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WakeState {
    Pending,
    Completed,
    TimedOut,
}
```
Both types derive `Serialize + Deserialize` (required for DO storage roundtrip via worker-rs's `state.storage().put<T>` / `get<T>`). `Clone` and `PartialEq + Eq` are cheap and useful for testing.
### 2.5 Type definitions (`tally-core/src/team_meta.rs`)
```rust
use serde::{Deserialize, Serialize};
/// Per-DO metadata. Lazy-initialized on first request; written once.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamMeta {
    pub tenancy_prefix: String,
    pub team_id_b64: String,
    pub created_at: i64,  // unix milliseconds
}
```
### 2.6 Type definitions (`tally-worker/src/rpc.rs`)
Internal Worker↔DO request/response types. Worker layer deserializes incoming HTTP requests, constructs these, forwards to DO; DO's fetch handler deserializes these from incoming Worker request body.
```rust
use serde::{Deserialize, Serialize};
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub identity_b64: String,
    pub context_id: String,
}
#[derive(Debug, Deserialize)]
pub struct UnregisterRequest {
    pub identity_b64: String,
    pub context_id: String,
}
#[derive(Debug, Deserialize)]
pub struct ReadInboxQuery {
    pub wait_seconds: Option<u32>,
    pub limit: Option<u32>,
}
#[derive(Debug, Deserialize)]
pub struct ValidateApiKeyRequest {
    pub identity_b64: String,
    pub api_key: String,
}
// DispatchRequest and CompleteRequest are deferred to dispatch PR.
#[derive(Debug, Serialize)]
pub struct OkResponse;  // unit serializes as {}
#[derive(Debug, Serialize)]
pub struct ValidateApiKeyResponse {
    pub valid: bool,
}
#[derive(Debug, Serialize)]
pub struct ReadInboxResponse {
    pub wakes: Vec<WakeSummary>,  // empty in MVP (no inbox writes yet)
}
#[derive(Debug, Serialize)]
pub struct WakeSummary {
    pub wake_id: String,
    pub caller_identity_b64: String,
    pub context_id: String,
    pub payload_b64: String,
}
```
## 3. The WakeRouter trait implementation
### 3.1 register_handler
Trait method takes `&mut self, identity: &Identity, context: &[u8]` and returns `Result<(), StoaError>`. Implementation:
```rust
async fn register_handler(
    &mut self,
    identity: &Identity,
    context: &[u8],
) -> Result<(), StoaError> {
    let context_str = std::str::from_utf8(context).map_err(|_| {
        StoaError::Wake(WakeError::Other(
            "context must be valid UTF-8".to_string(),
        ))
    })?;
    
    let key = format!("agent:{}:handlers", identity.to_url_safe_b64());
    let mut set: BTreeSet<String> = self.state
        .storage()
        .get(&key)
        .await
        .unwrap_or_default();
    set.insert(context_str.to_string());
    self.state.storage().put(&key, &set).await.map_err(|e| {
        StoaError::Wake(WakeError::Other(format!("storage write failed: {}", e)))
    })?;
    Ok(())
}
```
Read-modify-write pattern. The `unwrap_or_default()` handles the "no key yet" case (first registration produces empty BTreeSet which then gets the first context inserted).
### 3.2 unregister_handler (with delete-on-empty)
```rust
async fn unregister_handler(
    &mut self,
    identity: &Identity,
    context: &[u8],
) -> Result<(), StoaError> {
    let context_str = std::str::from_utf8(context).map_err(|_| {
        StoaError::Wake(WakeError::Other(
            "context must be valid UTF-8".to_string(),
        ))
    })?;
    
    let key = format!("agent:{}:handlers", identity.to_url_safe_b64());
    let Ok(mut set) = self.state.storage().get::<BTreeSet<String>>(&key).await else {
        // Key doesn't exist; nothing to unregister. No-op per trait contract.
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
```
Delete-on-empty discipline keeps storage clean; future operations don't have to filter for "key exists but set is empty."
### 3.3 dispatch (scope-boundary stub)
```rust
async fn dispatch(
    &self,
    _target: &Identity,
    _context: &[u8],
    _payload: WakePayload,
    _timeout: Duration,
) -> Result<WakeResponse, StoaError> {
    unimplemented!("TallyTeamDO::dispatch lands in the dispatch sub-PR")
}
```
`unimplemented!()` panics with explicit message. No partial input validation (no timeout-zero check, no UTF-8 validation on context). Partial work signals in-progress; scope-boundary should signal deferred.
Callers attempting to invoke dispatch in this PR's deployed state will see the DO crash with the panic message. Worker-layer routing will translate this to HTTP 500 with the panic message in the response body. This is intentional — it's a development-time guard that the dispatch sub-PR hasn't landed yet; production deployment should follow dispatch sub-PR landing.
### 3.4 The `&mut self` impedance — no Mutex needed
WakeRouter trait methods have mixed `&self` (dispatch) and `&mut self` (register_handler, unregister_handler) receivers. Cloudflare's DO single-writer guarantee plus async-trait's generated `Pin<Box<dyn Future + '_>>` futures with appropriate lifetime bounds mean the implementation compiles without `Mutex<TallyTeamDO>` or similar wrapping.
The fetch handler calls trait methods sequentially in match arms; each await releases the borrow before the next call. No nested-borrow issues by construction.
**If compilation reveals patterns I missed**: the implementation PR surfaces them as stop-and-surface findings rather than auto-adapting to `Mutex<>` or other patterns. Surfacing preserves the option for strategic-layer review before adopting structural workarounds.
If compilation reveals async-trait + reborrowing patterns that require structural workarounds (`Mutex<>`, `RefCell<>`, manual lifetime annotations beyond what async-trait generates), the implementation PR stops at that finding and surfaces it for strategic-layer review *before* adopting the workaround. Adopting interior-mutability patterns has implications for §5's concurrency model documentation; the adoption decision is architectural, not a routine implementation choice.
### 3.5 Validation at trait boundary
Each method with `context: &[u8]` validates UTF-8 inline at function entry, returning `WakeError::Other` for invalid input. The validation is inline (3 lines) rather than abstracted into a helper function — the pattern is small enough that the abstraction would add indirection without clarity benefit.
Validation runs at the trait boundary regardless of upstream guarantees: the RPC layer's JSON deserialization (which uses `context_id: String`) already requires valid UTF-8, so structurally redundant for the Worker↔DO path. But validation at the trait boundary ensures any non-Worker caller (future test code, alternative HTTP layers, direct DO test invocation) gets the same validation guarantee. Cost is small; benefit is structural correctness.
**Future-evolution note**: MVP uses `WakeError::Other("context must be valid UTF-8")` for invalid UTF-8. Future protocol evolution may add `WakeError::InvalidContext` for semantic precision (similar to `InvalidTimeout` added during Workstream B' PR γ). Current Other-string usage is a pragmatic choice that preserves trait surface stability during initial implementation; adding a new WakeError variant is a separate Stoa-repo coordination concern.
### 3.6 Inbox writes — committed policy for dispatch sub-PR
The inbox keyspace (`agent:{identity_b64}:inbox`) is read-only in this PR (gracefully empty per §4.5). Inbox writes happen during dispatch, which is deferred to a subsequent PR. This PR commits to the following policy for the dispatch sub-PR to implement:
**Inbox-drop is paired with wake-row-delete.** When the inbox's bounded-size enforcement (~1000 entries per agent per Sub-PR 1 Phase 0 design notes §3.2) drops the oldest entries on overflow, the corresponding `wake:{wake_id}` row gets deleted in the same operation. Specifically: when dispatch's push_back on the inbox VecDeque would exceed the bound, pop_front the oldest wake_id AND delete `wake:{wake_id}` for that wake_id in the same storage transaction.
This prevents wake-row leak — rows piling up in storage for overflow-dropped wakes with no cleanup path. The pairing matches the conceptual model: if a wake can't be retrieved (because its inbox entry was dropped), its storage row has no purpose.
Actual implementation lands with dispatch; this PR's state model implementation doesn't write to inbox, so the policy applies later. Stated here so future implementation has the locked commitment with strategic-layer-reviewed reasoning.
## 4. The Durable Object fetch handler
### 4.1 Path-based routing
worker-rs 0.5 DO RPC mechanism is the `fetch(&mut self, req: Request)` handler. The DO pattern-matches on request path and dispatches to appropriate handler methods. Internal path scheme:
```
POST /register             # register_handler trait method
POST /unregister           # unregister_handler trait method
POST /dispatch             # dispatch trait method (panics in this PR)
GET  /inbox?wait=N&limit=M # read_inbox
POST /complete             # complete_wake (deferred)
POST /validate_api_key     # validate_api_key
```
These paths are internal to Worker↔DO traffic. The public HTTP API (per Sub-PR 1 Phase 0 design notes §3.3) has different paths (e.g., `POST /v1/teams/{team_id}/agents/{identity}/register`) and the Worker layer translates between them.
### 4.2 The fetch handler skeleton
```rust
#[async_trait(?Send)]
impl DurableObject for TallyTeamDO {
    fn new(state: State, _env: Env) -> Self {
        Self { state }
    }
    
    async fn fetch(&mut self, mut req: Request) -> Result<Response> {
        // Lazy team:meta initialization on first request
        self.ensure_team_meta_initialized().await?;
        
        // Path-based routing
        let path = req.path();
        let method = req.method();
        
        match (method, path.as_str()) {
            (Method::Post, "/register") => self.handle_register(&mut req).await,
            (Method::Post, "/unregister") => self.handle_unregister(&mut req).await,
            (Method::Post, "/dispatch") => self.handle_dispatch(&mut req).await,
            (Method::Get, path) if path.starts_with("/inbox") => 
                self.handle_read_inbox(&req).await,
            (Method::Post, "/complete") => 
                Response::error("complete_wake deferred to dispatch sub-PR", 501),
            (Method::Post, "/validate_api_key") => 
                self.handle_validate_api_key(&mut req).await,
            _ => Response::error("not found", 404),
        }
    }
}
```
Each `handle_*` method deserializes the request body into the corresponding RPC type (RegisterRequest, etc.), calls the trait method or direct implementation, and serializes the response.
### 4.3 team:meta lazy initialization
```rust
impl TallyTeamDO {
    async fn ensure_team_meta_initialized(&mut self) -> Result<()> {
        let key = "team:meta";
        if self.state.storage().get::<TeamMeta>(&key).await.is_err() {
            let meta = TeamMeta {
                tenancy_prefix: "tally-cli-local".to_string(),
                team_id_b64: self.state.id().to_string(),  // worker-rs DurableObjectId
                created_at: current_unix_ms(),
            };
            self.state.storage().put(&key, &meta).await?;
        }
        Ok(())
    }
}
```
The exact `self.state.id()` API surface is worker-rs implementation detail; if the API differs from this pseudocode pattern, the implementation PR surfaces the actual API and adapts.
### 4.4 validate_api_key — uniform-true in MVP
```rust
impl TallyTeamDO {
    async fn handle_validate_api_key(&mut self, req: &mut Request) -> Result<Response> {
        let _request: ValidateApiKeyRequest = req.json().await?;
        // MVP: auth is genuinely deferred. validate_api_key returns true uniformly;
        // the api_keys storage exists but is read-empty (no admin tooling yet writes 
        // it). Phase 2 will add admin tooling for api_keys; until then, all 
        // authentication checks succeed.
        Response::from_json(&ValidateApiKeyResponse { valid: true })
    }
}
```
**Honest scope framing**: MVP doesn't have real auth. The plumbing exists for Phase 2 to fill in (validate_api_key reads from `agent:{identity_b64}:api_keys`; admin tooling will write keys there). For now, all calls return `valid: true`. Worker layer accepts this and forwards all requests; the dogfooding test runs without authentication friction.
Trade-off: this approach makes Tally trivially vulnerable to misuse if deployed publicly without Phase 2 auth in place. Phase 0 commits to this as the explicit MVP scope boundary — Tally MVP is for local dogfooding (single-tenant, trusted-development context); production deployment requires Phase 2 admin tooling to land first.
> **Deployment boundary**: Tally MVP must not be deployed to publicly-accessible Cloudflare environments without Phase 2 admin tooling in place. The dogfooding pattern assumes a local-development context (operator-controlled Cloudflare account, trusted-development network, single-tenant use). Deployment to environments accessible by untrusted parties is explicitly out of scope until Phase 2 lands real authentication.
### 4.5 read_inbox — gracefully empty in MVP
```rust
impl TallyTeamDO {
    async fn handle_read_inbox(&mut self, req: &Request) -> Result<Response> {
        let query: ReadInboxQuery = parse_query_params(req)?;
        let wait_seconds = query.wait_seconds.unwrap_or(0).min(60);  // bound at 60s
        let limit = query.limit.unwrap_or(100).min(1000);
        
        // For MVP: inbox is never written (dispatch deferred), so all reads 
        // return empty. The wait_seconds parameter is honored for compatibility 
        // with the eventual dispatch-era behavior but currently just sleeps.
        if wait_seconds > 0 {
            // Sleep for wait_seconds, then return empty. Real long-poll waiter 
            // logic lands with dispatch.
            Delay::from(Duration::from_secs(wait_seconds as u64)).await;
        }
        
        Response::from_json(&ReadInboxResponse { 
            wakes: vec![],  // always empty pre-dispatch
        })
    }
}
```
The wait_seconds sleep preserves the public API contract (long-poll behavior) even though there's nothing to wait for in MVP. When dispatch lands and inbox writes happen, the handler will be updated to actually check for waiting wakes during the wait period.
**Eviction interaction**: the wait_seconds sleep itself is in-memory state subject to DO eviction loss per §6.1. If the DO is evicted during the sleep, the HTTP request fails with 5xx and the client retries. This applies to both the MVP placeholder sleep and the real long-poll waiter that lands with dispatch — same eviction-recovery story.
## 5. Concurrency model
### 5.1 Single-writer guarantee per DO
Cloudflare Durable Objects serialize all method calls on one DO instance through the runtime. Within one TallyTeamDO (one team_id), register_handler / unregister_handler / read_inbox / validate_api_key calls don't race; the DO runtime serializes them. No application-level locking required.
Across TallyTeamDO instances (different team_ids), no ordering guarantees. Each team's DO is independent.
### 5.2 What this means for callers
- **Within a team**: callers can assume FIFO observation of operations by the DO. Sequential register / dispatch / unregister calls are processed in order.
- **Across teams**: no cross-team ordering; callers must serialize at application layer if cross-team ordering matters.
- **Tally-specific behavior, not protocol guarantee**: skytale issue #457 (concurrent dispatch ordering deferred) maintains MUST NOT framing at the protocol level. Tally's documented serialization is one implementation's behavior; cross-runtime callers should not depend on it as protocol-guaranteed.
### 5.3 Dispatch concurrency in this PR
Dispatch is `unimplemented!()` in this PR's scope. Calling dispatch panics; the DO crashes; the Worker layer translates to HTTP 500. Concurrency model documentation in 5.1 applies once dispatch lands.
## 6. Failure modes and recovery
### 6.1 Operation-by-operation recovery story
| Operation | Recovery on DO eviction |
|---|---|
| register_handler | Clean — storage roundtrip; survives eviction |
| unregister_handler | Clean — storage roundtrip |
| validate_api_key | Clean — storage roundtrip |
| read_inbox (no long-poll, wait_seconds=0) | Clean — storage roundtrip |
| read_inbox (with long-poll) | In-memory sleep is lost on eviction; HTTP request fails with 5xx; client retries |
| dispatch (deferred) | N/A (panics in this PR) |
| complete_wake (deferred) | N/A (501 in this PR) |
### 6.2 What "fail-with-retry" looks like for clients
For eviction-caused in-memory state loss (specifically the long-poll case in MVP), the Worker layer returns HTTP 503 Service Unavailable with `Retry-After: 1` header. Clients with idempotent request signatures can retry safely; non-idempotent retries are caller's responsibility.
### 6.3 Storage failure handling
Storage operations (`get`, `put`, `delete`) return `Result<T>` from worker-rs. Failures (network, capacity, etc.) map to `WakeError::Other(format!("storage X failed: {}", e))` in trait method returns. Worker layer translates `WakeError::Other` to HTTP 500.
## 7. Worker→DO RPC mapping
### 7.1 HTTP error code mapping
```rust
fn stoa_error_to_response(err: &StoaError) -> Response {
    match err {
        StoaError::Wake(WakeError::HandlerNotFound) => 
            Response::error("target identity has no eligibility registered for context", 404),
        StoaError::Wake(WakeError::DispatchRefused { reason }) => 
            Response::error(format!("dispatch refused: {}", reason), 400),
        StoaError::Wake(WakeError::TimeoutExpired { .. }) => 
            Response::error("wake timed out", 504),  // Gateway Timeout
        StoaError::Wake(WakeError::InvalidTimeout) => 
            Response::error("timeout must be > 0", 400),
        StoaError::Wake(WakeError::Other(msg)) => 
            Response::error(format!("internal error: {}", msg), 500),
        _ => Response::error("internal error", 500),
    }
}
```
504 Gateway Timeout chosen for TimeoutExpired over 408 Request Timeout because the Worker layer is acting as a gateway to the DO; 504 semantically matches "gateway didn't get timely upstream response" better than 408's "client took too long to send request."
## 8. Module structure (final)
```
tally-core/
├── Cargo.toml              # adds serde derive dep
└── src/
    ├── lib.rs              # `pub mod wake_record; pub mod team_meta;` + re-exports
    ├── wake_record.rs      # WakeRecord, WakeState
    └── team_meta.rs        # TeamMeta
tally-worker/
├── Cargo.toml              # adds tally-core, serde, async-trait deps
└── src/
    ├── lib.rs              # #[event(fetch)] Worker entry point
    ├── durable_object.rs   # TallyTeamDO + #[durable_object] + fetch handler
    ├── wake_router.rs      # impl WakeRouter for TallyTeamDO
    └── rpc.rs              # internal RPC types
```
### 8.1 Worker-level fetch handler (`tally-worker/src/lib.rs`)
The Worker's `#[event(fetch)]` extracts team_id from request path, gets DO stub via `env.durable_object("TALLY_TEAM_DO")?.id_from_name(&team_id_key)?.get_stub()?`, and forwards request to DO. Implementation details (auth check, path translation from public to internal scheme) are subsequent sub-PR scope — this Phase 0 specifies the DO state model implementation only.
### 8.2 wrangler.toml updates
The DO binding needs to be declared in `wrangler.toml`:
```toml
[[durable_objects.bindings]]
name = "TALLY_TEAM_DO"
class_name = "TallyTeamDO"
script_name = "tally"
```
Currently commented out in `tally/wrangler.toml` per Workstream A initial commit. This PR uncomments and activates it.
DO classes also need a migration entry on first deployment:
```toml
[[migrations]]
tag = "v1"
new_classes = ["TallyTeamDO"]
```
### 8.3 Deployment dependency
The migration entry must be successfully processed by Cloudflare during `wrangler deploy` before TallyTeamDO instances can be created. Deployment failures (auth, account quota, network) leave the system in a state where Worker requests reach the routing layer but DO method calls fail.
The integration-test sub-PR (per §9.3) will verify the deployment-and-migration sequence end-to-end; this PR's acceptance does not include verifying production deployment, only that `wrangler dev` correctly initializes the DO locally.
## 9. What's deferred and where it lands
### 9.1 Dispatch sub-PR
The dispatch trait method (currently `unimplemented!()`). Lands as a subsequent Workstream C PR. Includes:
- Real dispatch implementation per Sub-PR 1 Phase 0 design notes §3.6 wake routing flow
- Inbox writes (push_back on dispatch; bounded-size enforcement with wake-row-delete pairing)
- complete_wake handler (POST /complete)
- Real long-poll waiter logic in read_inbox (replacing MVP's sleep-then-empty)
- WakeRecord struct usage (the type defined in this PR's tally-core but not yet written)
- The wake_resolvers HashMap on TallyTeamDO struct (for in-memory promise resolution)
### 9.2 HTTP API surface implementation
The Worker fetch handler that translates public HTTP API (per Sub-PR 1 Phase 0 design notes §3.3) to internal DO paths. Lands as a subsequent Workstream C PR. Includes auth check enforcement (currently uniform-true in MVP — this PR's commitment), public-to-internal path translation, error code mapping.
### 9.3 Integration tests against `wrangler dev`
End-to-end tests that exercise the Worker + DO via `wrangler dev`. Lands as a subsequent Workstream C PR. Requires HTTP API surface (9.2) to land first.
### 9.4 Phase 2 admin tooling
API key management (write-path for `agent:{identity_b64}:api_keys`). Not Phase 1B scope per locked items.
## 10. Acceptance criteria
This PR is complete when:
1. TallyTeamDO struct exists with `#[durable_object]` and the DO trait impl
2. `impl WakeRouter for TallyTeamDO` covers all three methods (register_handler, unregister_handler, dispatch — last as `unimplemented!()`)
3. The DO's fetch handler routes the five internal paths (`/register`, `/unregister`, `/dispatch`, `/inbox`, `/validate_api_key`) to handler methods; `/complete` returns 501
4. `team:meta` lazy initialization works on first request
5. `validate_api_key` returns `Ok(true)` uniformly
6. `read_inbox` returns empty list (with optional wait_seconds sleep)
7. Storage operations use the agreed key schema and Rust types
8. Internal RPC types live in `tally-worker/src/rpc.rs`; shared storage types in `tally-core`
9. wrangler.toml declares the DO binding and migration
10. All CI jobs pass on tally main: Format, Clippy (with host and wasm32 steps), Tests (with host and wasm32 steps), Docs strict (with host and wasm32 steps), Unused dependencies, Worker build verification. CI workflow updated as part of this PR to split workspace-wide jobs by target internally.
11. Cargo.toml dependencies updated to consume stoa (the rev pin from #15 finally has a consumer)
12. This is the first tally-worker PR that adds stoa as a Cargo.toml dependency and actually consumes its trait surface in code. Prior rev-pin bumps were metadata-only; this PR makes Cargo.lock changes substantive for the first time, and future Workstream C PRs will follow the same substantive-rev-pin pattern.
## 11. Out of scope for Phase 0 deliberation
Items that may surface during implementation but are not Phase 0 commitments:
- Exact worker-rs API for `self.state.id()` and `DurableObjectId → String` conversion (implementation detail; pattern locked abstractly in §4.3)
- Helper functions for repeated patterns (e.g., abstracting UTF-8 validation into a helper). Inline pattern preferred per §3.5; if implementation surfaces clarity gain from helpers, surface for review.
- Test patterns and fixtures (this PR includes basic tests; comprehensive testing is in the integration-test sub-PR per 9.3)
- Specific log/observability instrumentation (defer to operational deliberation)
- Build profile optimizations (defer to perf deliberation)
These are implementation-PR concerns, not Phase 0 concerns.
