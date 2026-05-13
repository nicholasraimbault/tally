# Phase 1B Sub-PR 1 — Phase 0 Design Notes

**Status**: Phase 1 in progress. Design notes migrated from `skytale/docs/specs/phase-1b-sub-pr-1-phase-0.md` (skytale master at `86ab999`) to `tally/docs/specs/phase-1b-sub-pr-1-phase-0.md` as part of Sub-PR 1 Workstream A's force-reset commit on 2026-05-13. The skytale-side copy will be removed in a follow-up cleanup PR. This file is now the canonical Sub-PR 1 design reference.

**Precedent**: This document follows the Phase 0 pattern from issue #431's Option B sub-PR sequence — design notes surfaced for strategic-layer review before Phase 1 implementation begins, with explicit acceptance-criteria sign-off before code work. Same shape as Option B's individual sub-PR Phase 0 design notes.

**Skytale master HEAD at investigation time**: `8ac6c9f` (post-#450 merge; Phase 1B Phase 0 canonically complete).

**Phase 1B tracking issue**: [#444](https://github.com/nicholasraimbault/skytale/issues/444).

---

## Purpose of this document

The Phase 1B spec ([`docs/specs/phase-1b-spec.md`](./phase-1b-spec.md)) is the canonical Phase 1B framing. Sub-PR 1 is the largest sub-PR of the Phase 1B sequence: it creates the `tally` repo, migrates the Phase 1B spec into it, and lands the Tally Cloudflare runtime MVP (Worker + `TallyTeamDO` + the HTTP surface).

This Phase 0 document covers three workstreams that Sub-PR 1's Phase 1 implementation will run in parallel:

1. **Repo creation + spec migration** — mechanical scope; questions about license setup, Cargo workspace shape, CI scaffolding
2. **Stoa trait-surface gap survey** — strategic; concrete proposed revisions to `stoa::wake_router::WakeRouter` that the Cloudflare implementation needs. Concurrent Stoa-repo PRs land these in parallel.
3. **Tally Cloudflare runtime MVP architecture** — the bulk; Worker structure, HTTP routes, `TallyTeamDO` state model, auth, MLS-engine integration, wake routing flow

No code is written during Phase 0. The artifact is this document.

---

## What's already locked from the Phase 1B spec

These commitments are firm. Sub-PR 1's Phase 0 builds on them; doesn't relitigate:

1. **License**: BSL 1.1 for all Tally code. Apache 2.0 stays on stoa/stoa-rs/skytale-base/skytale-sdk.
2. **Runtime substrate**: Cloudflare Workers + Durable Objects. R2/Queues/KV deferred.
3. **Stoa trait implementation**: Tally implements `stoa::wake_router::WakeRouter`. Trait revisions surface via Workstream 2; concurrent Stoa-repo PRs land them.
4. **`TallyTeamDO` single-class shape** (vs split into router DO + inbox DO).
5. **Multi-tenancy prefix in DO naming from day one**, even if MVP keeps it constant.
6. **MVP auth**: long-lived API keys; no KV needed for MVP.
7. **HTTP surface** (5 routes from Phase 1B spec):
   - `POST /v1/teams/{team_id}/agents/{identity}/register` — WakeRouter::register_handler
   - `DELETE /v1/teams/{team_id}/agents/{identity}/handlers/{context_id}` — unregister
   - `POST /v1/teams/{team_id}/wakes` — WakeRouter::dispatch (synchronous; Worker holds connection)
   - `GET /v1/teams/{team_id}/agents/{identity}/inbox` — read pending wakes
   - `POST /v1/teams/{team_id}/wakes/{wake_id}/complete` — return response payload
8. **AuditTrail deferred**: not in MVP; data model supports adding later.
9. **WebSocket push optional for MVP**: HTTP polling sufficient.
10. **Cryptographic exclusion test (#395)** must keep passing across Sub-PR 1's work.

---

## Workstream 1: Repo creation + spec migration

### 1.1 Repo metadata

**Name**: `tally` (Phase 1B spec locked).

**License**: BSL 1.1 with the following parameters (per https://mariadb.com/bsl11/):

- **Licensor**: Pronoic Inc. (or the appropriate legal entity at commit time)
- **Licensed Work**: Tally — runtime, MCP plugin (TypeScript portion via separate LICENSE), CLI, docs
- **Change Date**: **2030-05-13** (four years after Phase 1B kickoff date 2026-05-13). 4 years is the maximum permitted under BSL 1.1; matches the precedent set by Bitwarden, Sentry, MariaDB, CockroachDB. Preserves the full commercial-exclusivity window before Tally automatically converts to Apache 2.0 — relevant for the optional managed-Tally-hosting commercial path even though the primary commercial moat lives in the Phase 2B destination product, not Tally itself.
- **Change License**: **Apache License, Version 2.0**. Matches the parent Stoa stack's license; consumers can rely on the eventual graduation to a recognized open-source license.
- **Additional Use Grant**: "You may make use of the Licensed Work, provided that you may not provide the Licensed Work to third parties as a hosted or managed service." This pattern (the Bitwarden BSL 1.1 grant) restricts the actual commercial concern — running Tally as a service for others — without excluding legitimate internal uses: consultancies coordinating their own work, open-source projects using Tally for contributor agents, researchers running Tally for academic work, or internal corporate teams. The grant doesn't preclude future commercial agreements for parties that want to offer Tally-as-a-service; it just doesn't grant that automatically.

The LICENSE file shall be the verbatim BSL 1.1 template with the four parameters filled in. A second file `LICENSES/Apache-2.0` shall include the Apache 2.0 text that's the change-license target — readers can verify the eventual graduation terms without external lookups.

**Description (GitHub About field)**:

> Cloudflare-hosted runtime for the Stoa agent-coordination protocol. Implements `WakeRouter` on Workers + Durable Objects. BSL 1.1.

**Topics (GitHub repo topics, for discoverability)**:

`agents`, `mls`, `cloudflare-workers`, `durable-objects`, `agent-coordination`, `claude-code`, `mcp`, `stoa`, `bsl`, `rust`

### 1.2 Workspace shape

**Multi-crate Cargo workspace from day one.** Single-crate is tempting for MVP simplicity but the Phase 1B spec already names two crates (`tally-core` shared logic + the Worker code). Establishing the workspace at the first commit avoids a refactor when Sub-PR 3 (Tally CLI) adds a third crate.

Initial workspace members:

- `tally-core/` — shared types and HTTP-client logic. Used by:
  - The Worker code (Sub-PR 1; calls into tally-core for type definitions)
  - The Tally CLI (Sub-PR 3; calls into tally-core for HTTP client and types)
- `tally-worker/` — the Cloudflare Worker entry point. `crate-type = ["cdylib"]` for wasm32-unknown-unknown target. Package name is `tally-worker` (originally specified as `worker` in §1.5; renamed during Phase 1 execution to avoid collision with the crates.io `worker = "0.5"` dep that shares the name — see §1.5's Phase 1 adaptation note).

The `tally-cli/` member gets added in Sub-PR 3 (not Sub-PR 1).

**Worker code language**: **Rust via `worker` crate** (Cloudflare's official Rust SDK for Workers, at https://crates.io/crates/worker). The crate has been at 0.x for several years with active maintenance; recent versions (~0.5+) support Durable Objects, fetch, KV, R2, and most Worker primitives in a stable enough surface for MVP work. Phase 1B spec implies Rust (Tally CLI is Rust; tally-core is shared); this confirms.

**Risk surface**: If Phase 1 implementation hits a worker-rs limitation that blocks the MVP HTTP surface, the fallback is TypeScript Worker code with tally-core staying Rust (compiled to WebAssembly and called from TypeScript via wasm-bindgen). That fallback is a Phase 1 stop-and-surface trigger, not preempted here.

**Cargo workspace integration with skytale**: tally is structurally independent of skytale but `tally-core` depends on `stoa-rs` (which lives in the skytale repo, unpublished). MVP uses git dependency pinned to a SHA:

```toml
# tally-core/Cargo.toml
[dependencies]
stoa-rs = { git = "https://github.com/nicholasraimbault/skytale", rev = "8ac6c9f" }
```

Cargo discovers the `stoa-rs` package within the skytale monorepo automatically; the rev pin avoids "upstream broke us" surprises. Updates happen explicitly per Sub-PR.

After issue #449 (publishing setup) and #448 (stoa name resolution) land in Sub-PR 4's Phase 0, the git dep flips to a crates.io version dep. Sub-PR 1 doesn't preempt this.

### 1.3 CI scaffolding

**Minimal MVP CI** (mirrors skytale's small standalone-crate pattern from `sdk` and `stoa-rs`):

- `Format` — `cargo fmt --check`
- `Clippy` — `cargo clippy --all-targets -- -D warnings`
- `Tests` — `cargo test`
- `Docs` — `cargo doc --no-deps` with `RUSTDOCFLAGS: -D warnings`
- `Unused deps` — `cargo machete`
- `Worker build verification` — `wrangler deploy --dry-run` to verify the Worker code compiles against Cloudflare's runtime environment

NOT in MVP CI:
- Cargo Audit (deferred; can borrow the skytale ignored-list approach when needed)
- Cargo Deny (deferred)
- Cargo Geiger (deferred)
- E2E integration tests against a live Worker (deferred to Sub-PR 4's dogfooding verification)

Adding the missing jobs is a Sub-PR 4 or later concern.

**Cross-repo CI verification**: Tally's CI builds against the latest stoa-rs pinned to a specific skytale SHA (per §1.2). If the upstream surface changes break Tally, CI catches it on the next dep bump. Concurrent Stoa-repo PRs landing trait revisions (Workstream 2) require an accompanying Tally PR to bump the rev pin — explicit coordination.

### 1.4 Spec migration mechanics

**Spec file path migration**:

- `skytale/docs/specs/phase-1b-spec.md` → `tally/docs/specs/phase-1b-spec.md` (via copy-and-delete since cross-repo)
- `skytale/docs/specs/phase-1b-sub-pr-1-phase-0.md` (this document) → `tally/docs/specs/phase-1b-sub-pr-1-phase-0.md`
- `skytale/docs/specs/phase-1b-tracking-issue-draft.md` — **stays in skytale as a historical artifact**. The issue (#444) is filed; the draft is no longer canonical. Adding a header note pointing at the filed issue is acceptable polish.

**Skytale-side cleanup PR** (concurrent or follow-up): after the migration commits land in tally, file a small skytale PR removing `skytale/docs/specs/phase-1b-spec.md` and `skytale/docs/specs/phase-1b-sub-pr-1-phase-0.md` from the skytale repo. The tracking-issue draft stays. This skytale-side cleanup is mentioned in Sub-PR 1 but is not blocking — can land as a follow-up small PR after Sub-PR 1's main work merges.

**Issue #444 body update**: when the skytale-side cleanup PR merges, the Phase 1B tracking issue #444's body currently references `skytale/docs/specs/phase-1b-spec.md` as the spec location. Update via `gh issue edit 444 --body-file <updated-body>` to reflect the new canonical location at `tally/docs/specs/phase-1b-spec.md`. Small operational task; no code changes. The issue stays OPEN until Sub-PR 4 closes Phase 1B.

**Spec opening section update**: the migrated spec's status header currently says (on master at `8ac6c9f`):

> **Status**: Phase 0 draft. Spec lives in `skytale/docs/specs/` during Phase 0 because the `tally` repo doesn't exist yet. On the first implementation sub-PR (Sub-PR 1 of the Phase 1B sequence), this file migrates to `tally/docs/specs/phase-1b-spec.md` and the `tally` repo gets created with that PR. Until then, this file's canonical location is `skytale/docs/specs/phase-1b-spec.md` to keep spec writing unblocked by repo-creation friction.

After migration, this becomes:

> **Status**: Phase 1 in progress. Spec migrated from `skytale/docs/specs/phase-1b-spec.md` (skytale master at `8ac6c9f`) to `tally/docs/specs/phase-1b-spec.md` as part of Sub-PR 1's first commit. The skytale-side copy was removed in a follow-up cleanup PR. This file is now the canonical Phase 1B implementation reference.

The phase-1b-sub-pr-1-phase-0.md migration mirrors the same opening-section update pattern.

### 1.5 Initial commit shape

The Sub-PR 1 Phase 1 work creates the repo with a first commit containing:

> **Phase 1 adaptation note (2026-05-13)**: §1.5 was originally specified with a workspace crate named `worker`. During Phase 1 execution, `cargo build --package worker` proved ambiguous because the load-bearing `worker = "0.5"` crates.io dep uses the same name (Cloudflare's official Rust SDK for Workers). The internal crate was renamed to `tally-worker` for namespace clarity (and symmetry with `tally-core`). The worker.rs SDK retains its canonical name in source code (`use worker::*;`). The file tree below reflects the post-rename state actually committed.

> **Phase 1 adaptation note (2026-05-13)** — placeholder Cargo.toml dependencies: the original Workstream A spec (§4j and §4l of the prompt) specified `serde`, `serde_json`, and `tally-core` as dependencies of the placeholder `tally-core/Cargo.toml` and `tally-worker/Cargo.toml`, anticipating Workstream C usage. cargo-machete CI flagged these as currently unused (the placeholder `lib.rs` files don't yet use them). Resolution: the placeholder Cargo.toml files in the post-resolution commit contain only deps that are actually used (`worker = "0.5"` in tally-worker; none in tally-core). Workstream C tasks will add deps atomically with their actual use. The design intent at Phase 0 close is preserved here as historical context; the fix commit aligns the Cargo.toml files with the placeholder-crate intent (do nothing yet).

```
tally/
├── README.md                   # elevator pitch + quickstart pointer
├── LICENSE                     # BSL 1.1 with parameters per §1.1
├── LICENSES/
│   └── Apache-2.0              # change-license target (full text)
├── .gitignore                  # standard Rust + Cloudflare patterns
├── Cargo.toml                  # workspace declaration (members: tally-core, tally-worker)
├── .github/
│   └── workflows/
│       └── ci.yml              # minimal MVP CI per §1.3
├── docs/
│   └── specs/
│       ├── phase-1b-spec.md                    # migrated from skytale
│       └── phase-1b-sub-pr-1-phase-0.md        # migrated from skytale (this document)
├── wrangler.toml               # Worker config; minimal stub
├── tally-core/
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs              # placeholder; types added in subsequent commits
└── tally-worker/
    ├── Cargo.toml              # crate-type = ["cdylib"]; package name "tally-worker"
    └── src/
        └── lib.rs              # placeholder; #[event(fetch)] handler stub
```

Subsequent commits in Sub-PR 1 add the Worker implementation, the DO implementation, integration tests, and any updates to the design notes that emerge from implementation.

---

## Workstream 2: Stoa trait-surface gap survey

The current `stoa::wake_router::WakeRouter` trait (on skytale master at `8ac6c9f`):

```rust
pub trait WakeRouter {
    fn register_handler(
        &mut self,
        identity: &Identity,
        context: &[u8],
        handler: Box<dyn Fn(WakePayload) -> Result<WakeResponse, StoaError> + Send + Sync>,
    ) -> Result<(), StoaError>;

    fn dispatch(
        &self,
        target: &Identity,
        context: &[u8],
        payload: WakePayload,
    ) -> Result<WakeResponse, StoaError>;

    fn unregister_handler(&mut self, identity: &Identity, context: &[u8]) -> Result<(), StoaError>;
}

pub struct WakePayload(pub Vec<u8>);
pub struct WakeResponse(pub Vec<u8>);
```

Methodology: write pseudo-code for the Cloudflare implementation; any expression that requires something the trait doesn't provide is a gap.

### 2.1 Gap surface — proposed revisions

#### REVISION 1 (required): Add `timeout: Duration` parameter to `WakeRouter::dispatch`

**Current**:
```rust
fn dispatch(
    &self,
    target: &Identity,
    context: &[u8],
    payload: WakePayload,
) -> Result<WakeResponse, StoaError>;
```

**Proposed**:
```rust
fn dispatch(
    &self,
    target: &Identity,
    context: &[u8],
    payload: WakePayload,
    timeout: Duration,
) -> Result<WakeResponse, StoaError>;
```

**Cloudflare implementation need**: The Worker holds the caller's HTTP connection while awaiting the target's `complete` call. Without an explicit timeout, the implementation has to pick an arbitrary default (e.g., 30s) — which may be too short for long-running coordination tasks (deliberation rounds, multi-step task chains) and too long for ad-hoc notifications. Per-call timeout lets the caller communicate its expected latency.

**Rationale**: Most async-coordination protocol contracts surface timeout in the dispatch signature (gRPC deadlines, HTTP request timeouts, etc.). The current trait omits it; the Cloudflare implementation makes the omission consequential.

**Compatibility impact**:
- `stoa-rs/src/wake_router.rs::NoopWakeRouter::dispatch`: needs the new parameter (trivial signature update; body still returns `Err(StoaError::Wake(...))`)
- `stoa-rs/examples/hypothetical_python_binding.rs` and `hypothetical_typescript_binding.rs`: no consumer code; type references only
- `skytale-sdk`: no consumer code (NoopWakeRouter is the stub Tally supersedes)
- The cryptographic exclusion test in `tests/agent_teams_e2e/`: doesn't touch WakeRouter; unaffected

#### REVISION 2 (recommended): Structured `WakeError` enum replacing `StoaError::Wake(String)`

**Current**:
```rust
pub enum StoaError {
    // ...
    Wake(String),  // free-form payload
}
```

**Proposed**:
```rust
pub enum WakeError {
    /// Wake dispatched but target didn't complete within the requested timeout.
    TimeoutExpired { timeout: Duration },
    /// Target identity has no registered handler for the given context.
    HandlerNotFound,
    /// Runtime refused dispatch (e.g., rate-limited, capacity exceeded, target not in team).
    DispatchRefused { reason: String },
    /// Runtime-internal error not covered by other variants. Free-form catch-all.
    Other(String),
}

pub enum StoaError {
    // ...
    Wake(WakeError),  // replaces Wake(String)
}
```

**MVP-bounded granularity**: The four variants (`TimeoutExpired`, `HandlerNotFound`, `DispatchRefused`, `Other`) are sized for MVP needs. Future Tally work may surface needs for finer granularity (e.g., separating "target not in team" from "target in team but not registered for this context" — both currently land in `HandlerNotFound`). Additive variants land as non-breaking changes; existing consumers continue to work since they're matching on the enum, not on string content. The MVP shape doesn't preclude growth.

**Cloudflare implementation need**: Distinguishing timeouts from "target not registered" from "dispatch refused" matters for caller behavior. A timeout might warrant retry with a longer timeout; "target not registered" warrants surfacing the team-state mismatch; "dispatch refused" warrants surfacing whatever the runtime is complaining about. With `Wake(String)`, callers can't pattern-match these — they'd parse strings, which is brittle.

**Rationale**: Phase 1B's `@skytalesh/tally-mcp` Sub-PR 2 translates `StoaError` into Python exception types (per the Orchestration MCP server's pattern: `AuthError`, `ChannelError`, `MlsError`, etc.). Structured wake errors let the MCP plugin distinguish "raise TimeoutError" vs "raise NotFoundError" cleanly.

**Pre-revision codebase verification** (run before drafting concrete Stoa-repo PRs):

```bash
cd ~/Projects/pronoic/skytale
# Find all pattern matches on StoaError::Wake
grep -rn "StoaError::Wake" --include="*.rs" .
# Find all string-formatting of Wake variant
grep -rn "Wake(" --include="*.rs" . | grep -v "WakeRouter\|WakePayload\|WakeResponse"
```

The Compatibility impact section below presumes "none exist outside the bridge." If verification surfaces additional pattern-match sites (e.g., in test code, in the existing examples, in skytale-sdk integration code), the compatibility impact updates to enumerate them. This verification runs as the first step of Phase 1's Workstream 2 (Stoa-repo PR drafting), not during Phase 0.

**Compatibility impact** (presuming the verification above confirms no additional sites; update inline if it surfaces any):
- `From<stoa::StoaError> for sdk::SdkError` bridge in `skytale-sdk/src/lib.rs`: change one arm from `Wake(s) => SdkError::ProtocolError(format!("wake error: {s}"))` to `Wake(e) => SdkError::ProtocolError(format!("wake error: {e}"))` — `WakeError` derives `Display` so the format string works identically
- `NoopWakeRouter`: currently returns `StoaError::Wake("...".to_string())`; needs update to `StoaError::Wake(WakeError::Other("...".to_string()))`
- Pattern matches on `Wake(s)` anywhere: none currently exist in the codebase outside the bridge (pending Phase-1 verification per the grep above)

#### REVISION 3 (recommended): Doc-comment clarification — sync trait commitment + Cloudflare async-bridging pattern

**Current**: The trait's module-level doc-comment doesn't address the sync-vs-async question explicitly. The trait happens to be sync; consumers infer.

**Proposed**: Add a doc-comment section to `stoa/src/wake_router.rs`:

```rust
//! ## Sync trait surface; async-implementing runtimes bridge internally
//!
//! The `WakeRouter` trait surface is synchronous. Implementations that
//! are naturally async (e.g., Tally's Cloudflare-Workers-hosted runtime)
//! bridge to sync via internal `block_on` or equivalent.
//!
//! Rationale: Stoa's planning doc committed to sync APIs for codebase
//! consistency with the existing Skytale stack. The Cloudflare
//! implementation hides its async machinery behind a sync facade matching
//! the pattern `SkytaleTeam` uses (`SkytaleTeam` holds a `tokio::Runtime`
//! and calls `runtime.block_on(...)` internally to drive
//! `SkytaleApiClient`'s async methods).
//!
//! An async cousin trait `AsyncWakeRouter` is not part of MVP and is
//! tracked as a Phase 2 question in the Phase 1B spec's "Open
//! commercial/operational questions" section.
```

**Rationale**: Future readers tracing why a sync trait runs on async Cloudflare need an in-source explanation. The Phase 1B spec captures this question but the source is the canonical place.

**Compatibility impact**: doc-only; no signature changes.

### 2.2 Considered but not proposed

The following gaps surfaced during the survey but are NOT proposed for Sub-PR 1's revisions. Each is captured here so strategic-layer review knows what was considered and rejected.

#### Fire-and-forget dispatch (NOT MVP)

The current trait requires every dispatch to await a `WakeResponse`. A fire-and-forget variant would suit broadcast notifications ("task assigned to X" sent to interested observers) and one-way work delegation.

**Why not now**: The Phase 1B spec's HTTP surface treats every wake as response-required (the caller's HTTP `POST /wakes` blocks). Adding fire-and-forget would require a second HTTP surface and second trait method. Out of scope for the MVP dogfooding pattern (which is point-to-point coordination, not broadcast).

**Future**: Surface as a Phase 2 question if the deliberation board pattern (Phase 2B) needs broadcast semantics.

#### Wake correlation IDs at the trait surface (NOT MVP)

The Cloudflare runtime needs to correlate `complete` calls to original `dispatch` calls. Two options:
- (a) Runtime generates IDs internally; trait doesn't expose
- (b) Trait exposes `wake_id` as part of `WakeResponse` or dispatch metadata

**Why (a)**: The HTTP API exposes wake_id as the spec's `POST /wakes/{wake_id}/complete` path component. Internal runtime can generate ULIDs (sortable, sufficient entropy). The trait stays clean.

**Future**: If a use case emerges where callers need the wake_id BEFORE dispatch completes (e.g., cancellation, status queries), surface as a trait revision.

#### Idempotency keys (NOT MVP)

Network retries could duplicate dispatches. Without idempotency, the same wake gets delivered twice.

**Why not now**: HTTP-level idempotency (e.g., `Idempotency-Key` header) is a runtime-level concern, not a trait-level concern. MVP scope doesn't require it; agents using Tally are expected to handle their own dedup.

**Future**: Add as HTTP-API affordance (not a trait change) if duplicate-delivery problems emerge.

#### Cancellation (NOT MVP)

A sync `dispatch` can't be cancelled from caller-side. The HTTP API could expose `DELETE /v1/teams/{team_id}/wakes/{wake_id}` to cancel in-flight wakes; the DO would mark the wake cancelled and resolve the waiting `dispatch` with an error.

**Why not now**: Marginal value for MVP. Cancellation matters when wakes are long-running or expensive; the dogfooding pattern's wakes are short (point-to-point task hand-off). Phase 2 question if needed.

#### Async cousin trait `AsyncWakeRouter` (NOT MVP)

Conceptually parallel to `WakeRouter` but with `async fn` signatures.

**Why not now**: Stoa's sync-trait commitment from Option B sub-PR 3 stands. The Cloudflare implementation bridges sync-to-async via internal block_on. Adding `AsyncWakeRouter` is a feature, not a fix.

**Future**: Phase 2 question. If a consumer of Stoa wants async-native dispatch from idiomatic Rust async code (vs the HTTP API), `AsyncWakeRouter` lands then. The contracts stay parallel; consumers pick the variant matching their runtime model.

### 2.3 Survey output summary

| # | Revision | Required? | Affects |
|---|---|---|---|
| 1 | `dispatch` gains `timeout: Duration` | **Required** for MVP | stoa, stoa-rs (NoopWakeRouter) |
| 2 | `StoaError::Wake(String)` → `StoaError::Wake(WakeError)` enum | **Recommended** for MVP | stoa, stoa-rs (NoopWakeRouter), skytale-sdk (From bridge) |
| 3 | Doc-comment clarifying sync-trait commitment + Cloudflare async bridging | **Recommended** for MVP | stoa (doc only) |

Items considered but not proposed for Sub-PR 1: fire-and-forget, wake correlation IDs at trait, idempotency keys, cancellation, async cousin trait. All deferred to Phase 2 or beyond.

### 2.4 Concurrent Stoa-repo PRs

Revisions 1 and 2 require code changes to the skytale repo's `stoa/` and `stoa-rs/` crates. They are filed as Stoa-repo PRs landing in parallel with Sub-PR 1's Phase 1 implementation work in the tally repo.

Suggested Stoa-repo PR sequence:

- **stoa-trait PR A**: Add `timeout: Duration` to `WakeRouter::dispatch`. Updates `stoa/src/wake_router.rs`, `stoa-rs/src/wake_router.rs::NoopWakeRouter::dispatch` signature, and the two example bindings (`stoa-rs/examples/hypothetical_python_binding.rs` + `hypothetical_typescript_binding.rs`) to thread the new parameter.
- **stoa-trait PR B**: Add `WakeError` enum and refactor `StoaError::Wake(String)` to `StoaError::Wake(WakeError)`. Updates `stoa/src/error.rs`, `stoa-rs/src/wake_router.rs::NoopWakeRouter::dispatch` to use `WakeError::Other(...)`, and `skytale-sdk/src/lib.rs`'s `From<StoaError> for SdkError` bridge.
- **stoa-trait PR C**: Doc-comment clarification. Updates `stoa/src/wake_router.rs` module-level docs.

Each Stoa-repo PR follows the same Phase 0 → Phase 1 → review → merge discipline as the Option B sub-PRs. PR A and B are independent and can land in either order; PR C is pure docs and can land alongside either.

Tally Sub-PR 1's Phase 1 work CAN begin against the current `stoa-rs` SHA pin and bump the pin after each stoa-trait PR merges. The Stoa-repo PRs are not blockers for Sub-PR 1's Phase 1 to start; they're concurrent work.

---

## Workstream 3: Tally Cloudflare runtime MVP architecture

### 3.1 Worker structure

**Single Worker** fronting the HTTP API. No WebSocket in MVP (deferred per Phase 1B spec).

**Entry point**: `tally-worker/src/lib.rs` with the `worker` crate's `#[event(fetch)]` macro (the crates.io `worker` dep, not our internal `tally-worker` package):

```rust
#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    Router::new()
        .post_async("/v1/teams/:team_id/agents/:identity/register", register_agent)
        .delete_async("/v1/teams/:team_id/agents/:identity/handlers/:context_id", unregister_handler)
        .post_async("/v1/teams/:team_id/wakes", dispatch_wake)
        .get_async("/v1/teams/:team_id/agents/:identity/inbox", read_inbox)
        .post_async("/v1/teams/:team_id/wakes/:wake_id/complete", complete_wake)
        .get_async("/v1/health", health_check)
        .run(req, env)
        .await
}
```

Worker code is stateless. All state lives in `TallyTeamDO`. The Worker's role is:
1. Parse the URL path + match to a handler
2. Authenticate (extract API key from Authorization Bearer; validate against the DO's stored key list)
3. Forward authenticated requests to the appropriate `TallyTeamDO` instance via DO RPC
4. Map DO results to HTTP responses

**Error → HTTP status mapping**:

| Internal error | HTTP status |
|---|---|
| Malformed request (bad JSON, missing required fields) | 400 |
| Missing API key | 401 |
| Invalid API key | 401 |
| API key doesn't authorize the (identity, team) tuple | 403 |
| Team not found | 404 |
| Wake not found | 404 |
| Wake timed out (in-flight; target didn't complete) | 408 |
| Wake already in terminal state (caller tried to complete a completed wake) | 410 |
| Dispatch refused (target not registered for context, target not in team, etc.) | 422 |
| Internal runtime error (DO error, serialization failure) | 500 |

Health check `GET /v1/health` returns 200 with `{"status": "ok", "version": "..."}` for liveness probes.

### 3.2 `TallyTeamDO` state model

**DO naming key**: `${tenancy_prefix}:${team_id_url_safe_b64}`. For MVP, `tenancy_prefix = "tally-cli-local"` (constant). The two-segment naming establishes the multi-tenancy hook from day one without preempting Phase 2's multi-tenant decision.

**Storage keys** (using Cloudflare DO's `state.storage` key-value API):

```
team:meta
  Value: { tenancy_prefix: String, team_id_b64: String, created_at: Timestamp }
  Written: on first request to this DO (lazy initialization)
  Read: on every request (for tenancy verification)

agent:{identity_b64}:handlers
  Value: Set<context_id: String>
  Written: register_handler / unregister_handler
  Read: dispatch (to verify target has a registered handler for the context)

agent:{identity_b64}:inbox
  Value: List<wake_id: String> (FIFO; bounded to e.g. 1000 entries)
  Written: dispatch (append wake_id) / complete_wake (no-op on inbox; wake state changes separately)
  Read: read_inbox (drain or peek)

agent:{identity_b64}:api_keys
  Value: Set<api_key_hash: String> (SHA-256 hex of the API key)
  Written: admin endpoint (issued via tally CLI in Sub-PR 3)
  Read: every request that authenticates as this agent

wake:{wake_id}
  Value: {
    caller_identity_b64: String,
    target_identity_b64: String,
    context_id: String,
    payload_b64: String,
    timeout_ms: u64,
    state: "pending" | "completed" | "timed_out" | "cancelled",
    response_b64: Option<String>,
    created_at: Timestamp,
    completed_at: Option<Timestamp>,
  }
  Written: dispatch (state=pending) / complete_wake (state=completed) / alarm handler (state=timed_out)
  Read: complete_wake (validation) / inbox listing (cross-ref)

[reserved for future Phase 2A persistence; not written in MVP:]
agent:{identity_b64}:memory
agent:{identity_b64}:history
agent:{identity_b64}:affordances

[reserved for future audit-trail:]
audit:{event_ulid}
```

**Tenancy prefix**: stored in `team:meta`, constant for MVP. Multi-tenant Phase 2 will key DO instances by tenant + team_id; the prefix already in the naming key supports the transition.

**Audit-trail buffer placeholder**: `audit:{event_ulid}` keyspace reserved. MVP doesn't write to it; the data model doesn't preclude adding it. When `AuditTrail` impl lands (later Phase 1B sub-PR or Phase 2), audit events accumulate here. Once volume warrants, cold events migrate to R2.

**Persistent memory placeholders**: `agent:{identity_b64}:memory|history|affordances` reserved. MVP doesn't write to them. Phase 2A's spec defines their shape.

**Storage bounds**: Cloudflare DO has a ~128KB-per-key practical limit. Wake payloads and response payloads are opaque bytes; large payloads need either:
- Application-level chunking
- R2-backed storage for the payload, key holds pointer

MVP enforces a 32KB-per-wake-payload limit (returns HTTP 400 if exceeded). Rationale: Cloudflare DO storage values have a ~128KB practical limit per key. Wake state includes the payload plus metadata (caller identity, target identity, context, timestamps, state, response, etc.); 32KB leaves ~96KB headroom for the rest of the wake state including the eventual response. 32KB is sufficient for typical encrypted task descriptions with context (compare: a typical MLS-encrypted message payload is a few KB; tool-call payloads are similarly modest). Larger payloads can use application-level chunking (caller splits across multiple wakes) or wait for R2-backed payload storage when AuditTrail lands and similar storage patterns enter the runtime.

### 3.3 HTTP route specifications

**POST `/v1/teams/{team_id}/agents/{identity}/register`**

Request:
```json
{
  "context_id": "task-routing",
  "metadata": { "role": "engineer" }
}
```

`context_id` is a string (caller-meaningful routing key; the underlying Stoa trait expects `&[u8]` but MVP uses UTF-8 strings; trait surface accepts via `.as_bytes()`). `metadata` is opaque JSON the DO stores but doesn't interpret.

Response (201):
```json
{ "registered": true, "context_id": "task-routing" }
```

Auth: Authorization Bearer must validate against `agent:{identity_b64}:api_keys`.

DO RPC: `register_agent_handler(identity, context_id) -> Result<(), StoaError>`.

**DELETE `/v1/teams/{team_id}/agents/{identity}/handlers/{context_id}`**

Response (204, no body).

Auth: same as register.

DO RPC: `unregister_agent_handler(identity, context_id) -> Result<(), StoaError>`.

**POST `/v1/teams/{team_id}/wakes`**

Request:
```json
{
  "target_identity": "base64(target_bytes)",
  "context_id": "task-routing",
  "payload": "base64(opaque ciphertext)",
  "timeout_seconds": 30
}
```

`timeout_seconds`: clamped to [1, 300]. Default if omitted: 30.

Response (200, on success):
```json
{
  "wake_id": "01J5...",
  "response": "base64(opaque ciphertext)",
  "completed_at": "2026-05-13T20:00:00Z"
}
```

Response (408, timeout):
```json
{ "error": "wake timed out", "wake_id": "01J5...", "timeout_seconds": 30 }
```

Response (422, target not registered):
```json
{ "error": "target has no registered handler for context_id", "context_id": "task-routing" }
```

Auth: Authorization Bearer validates `agent:{caller_identity_b64}:api_keys`. Caller identity from the validated API key; not a request parameter.

DO RPC: `dispatch(caller, target, context_id, payload, timeout) -> Result<WakeResponse, StoaError>`. Blocks until completion or timeout.

**GET `/v1/teams/{team_id}/agents/{identity}/inbox`**

Query params:
- `wait_seconds` (default 0; max 30): long-poll up to this many seconds awaiting new wakes if inbox is empty
- `limit` (default 10; max 100): max wakes to return per call

Response (200):
```json
{
  "wakes": [
    {
      "wake_id": "01J5...",
      "caller_identity": "base64(...)",
      "context_id": "task-routing",
      "payload": "base64(...)",
      "expires_at": "2026-05-13T20:01:00Z"
    }
  ],
  "more_available": false
}
```

Auth: Authorization Bearer validates `agent:{identity_b64}:api_keys`.

DO RPC: `read_inbox(identity, wait_seconds, limit) -> Vec<WakeSummary>`. Long-polls via in-memory wait-list keyed by identity, **NOT** via `state.blockConcurrencyWhile` (see §3.6 pattern verification — `blockConcurrencyWhile` has a 30-second timeout and would reset the DO if exceeded). The DO maintains a per-identity list of pending `read_inbox` resolvers; when step 7's `agent:{target_b64}:inbox` append happens, the DO drains those resolvers.

**POST `/v1/teams/{team_id}/wakes/{wake_id}/complete`**

Request:
```json
{ "response": "base64(opaque ciphertext)" }
```

Response (200):
```json
{ "completed": true, "wake_id": "01J5..." }
```

Response (410, already terminal):
```json
{ "error": "wake already in terminal state", "wake_id": "...", "current_state": "timed_out" }
```

Auth: Authorization Bearer must authorize the wake's target_identity. Caller can only complete wakes targeting them.

DO RPC: `complete_wake(wake_id, response, by_identity) -> Result<(), StoaError>`.

### 3.4 Auth implementation

**API key format**: `tk_${issuer}_${random_32_b64}`

Examples:
- `tk_local_aB3xK9mNpQ7zR2vF6sH8wE5jY1cT4dL0` (MVP, issuer = "local")
- `tk_subscription_...` (Phase 2)

The `tk_` prefix is the namespace marker. `issuer` distinguishes key sources. `random_32_b64` is 24 bytes (32 b64 chars) of crypto-random data.

**Phase 2 evolvability**: This MVP format is opaque-random tokens. Phase 2 commercial work may want signed tokens (JWT-style) for offline validation and embedded subscription metadata. The runtime can support both formats during a transition by detecting format from the prefix: `tk_` for opaque random, a different prefix (e.g., `eyJ` for JWT, or a deliberate Pronoic prefix) for signed. Migration doesn't require breaking existing keys; new issuer types extend the validation strategy. The MVP format doesn't preclude this growth.

**Issuer field**: per Phase 1B spec, the issuer field is the Phase 2 evolvability hook. MVP issuer is `local` (issued by Tally CLI on operator machine). Phase 2 introduces additional issuers (e.g., `subscription` issued by Pronoic's subscription service).

**Storage**: Worker hashes the API key (SHA-256, hex-encoded) and looks up the hash under `agent:{identity_b64}:api_keys`. Plain SHA-256 is sufficient because keys are random and not user-chosen; bcrypt/argon2 protect against rainbow-table attacks on user passwords, irrelevant here.

**Validation flow**:

1. Worker extracts `Authorization: Bearer <token>` from request
2. Worker computes `sha256(token)` → key_hash
3. Worker calls `TallyTeamDO::validate_api_key(identity, key_hash)`
4. DO checks `agent:{identity_b64}:api_keys` contains key_hash
5. Returns 401 if no match

**API key issuance (MVP path)**:

The DO exposes an internal admin RPC method `add_api_key(identity, key_hash)`. The Tally CLI (Sub-PR 3) invokes this via an admin endpoint on the Worker:

```
POST /admin/agent-keys
Authorization: Admin <admin_secret>
Body: { "identity": "...", "key_hash": "..." }
```

The admin secret is a deploy-time env var configured in the Worker. Sub-PR 1 exposes the admin endpoint; Sub-PR 3's `tally agents key issue` consumes it.

**Admin endpoint security**: This endpoint creates API keys with arbitrary (identity, team) authorization. Compromise of the admin secret = full system compromise. MVP guardrails:

- **High-entropy admin secret**: 32+ bytes crypto-random, generated during Worker deployment via `wrangler secret put ADMIN_SECRET` (Cloudflare stores secrets encrypted at rest).
- **Cloudflare-level rate limiting**: per-IP rate limit (e.g., 10 requests/minute) on the `/admin/*` route via Cloudflare Workers' built-in rate-limiting binding. Prevents brute-force attempts.
- **Logging via Cloudflare Workers Logs**: every admin endpoint invocation logged with timestamp, IP, success/failure. Audit-trail concern; cleanup happens when AuditTrail trait implementation lands.
- **No request body persistence beyond the API key hash**: the admin endpoint receives plaintext API keys, hashes them, stores only the hash. Plaintext is not logged or persisted.

**Phase 2 hardening list** (NOT MVP; surfaced for future work):
- mTLS for admin endpoint access (requires client certificate management)
- IP allowlisting (operator's CLI machine IPs only)
- Audit log integration with Cloudflare R2 for long-term retention
- Separate admin endpoint deployment topology (e.g., admin endpoint on a separate Worker with stricter access controls)

For Sub-PR 1's MVP scope: the admin endpoint is included so operators can manually pre-populate keys during the dogfooding setup. Sub-PR 3's CLI wraps this with a friendlier UX.

### 3.5 MLS-engine integration

**The Worker does not touch MLS state.** Per Phase 1B spec's Tier 2 commitment, the Worker stays skytale-sdk-free. Wake payloads are opaque bytes (already encrypted by callers via `SkytaleTeam::encrypt` on the agent side); the Worker routes ciphertext.

**Team membership verification**: MVP uses **operator-trust**:

- The skytale CLI (existing tooling) creates and manages Stoa teams; team membership is the cryptographic MLS state.
- The Tally CLI (Sub-PR 3) issues API keys; the operator (running Tally CLI) verifies the target identity is a team member before issuing the key. This is the trust point.
- The Worker trusts API keys at runtime; if a key authorizes (identity, team), the Worker accepts the agent's actions on that team.

This is option (a) from the Phase 1B spec's MLS integration section. The Worker doesn't call Skytale's HTTP API to re-verify membership on each request; that would add latency and require the Worker to depend on skytale-base (or call Skytale's API directly), partially defeating the Tier 2 commitment.

**Phase 2 consideration**: commercial paths may want stricter validation (e.g., "expire API keys when MLS membership lapses"). That's option (b) and requires the Worker to call out to Skytale's API per request. NOT MVP; Phase 2 question if it surfaces.

### 3.6 Wake routing flow (detailed sequence)

**Pattern verification** (run during Phase 0): Web-searched Cloudflare's official documentation for `blockConcurrencyWhile` behavior with long external waits. **Finding: gap confirmed.**

- Cloudflare's `blockConcurrencyWhile` has a documented **30-second timeout**; if exceeded, the Durable Object is **reset** (in-memory state lost). Source: https://developers.cloudflare.com/durable-objects/api/state/.
- Cloudflare's documentation explicitly recommends against `blockConcurrencyWhile` for regular request handling ("For regular request handling, you rarely need `blockConcurrencyWhile`").
- Tally's MVP timeout range is 1–300 seconds (5 minutes max), which exceeds the 30-second `blockConcurrencyWhile` limit on the upper end.
- `blockConcurrencyWhile` is intended for blocking concurrent requests during a critical section (e.g., schema migration, state initialization), not for awaiting external events that another request will resolve.

**Resolution**: §3.6 step 9 below uses the **in-memory promises map** pattern, NOT `blockConcurrencyWhile`. The DO maintains a `Map<wake_id, OneshotResolver<WakeResponse>>` in memory (in JS, a Promise with an externally-callable `resolve`; in Rust-via-worker-crate, a `tokio::sync::oneshot::Sender`). The dispatching request awaits the resolver directly; other requests interleave with the awaiting dispatch. Storage state (the `wake:{wake_id}` row) is the source of truth; the in-memory resolver is convenience for the waiting request. Cloudflare's alarm subsystem handles the timeout independently of any in-memory state.

**Failure mode considered**: if the DO is reset between dispatch and complete (e.g., due to an unrelated error, deploy, or eviction), the in-memory resolver is lost. The dispatching Worker request fails with a 5xx error; the caller's MCP plugin can retry. The wake row in storage stays state=pending and the alarm eventually marks state=timed_out. Late `complete_wake` calls after timeout return 410 Gone per §3.7. Acceptable degradation for MVP.

**Alternatives considered**: (1) Storage polling — the caller polls `GET /v1/teams/{team_id}/wakes/{wake_id}` for completion; less elegant but unambiguously works and is the Phase 1 stop-and-surface fallback if the in-memory-resolver pattern surfaces problems during implementation. (2) WebSocket from caller — requires changes to the spec's HTTP surface; deferred. The in-memory-promises-map pattern is the cleanest MVP fit.

---

A complete `tally_assign_task` from Caller to Target:

1. **Caller's MCP server** (Sub-PR 2) calls `tally_assign_task(target, context_id, payload)`.
2. **MCP server → Worker**: `POST /v1/teams/{team_id}/wakes` with `Authorization: Bearer <caller_api_key>`.
3. **Worker authenticates Caller**: hashes the API key, RPCs into `TallyTeamDO::validate_api_key(caller_identity, key_hash)`. Returns 401 on failure.
4. **Worker → DO**: `dispatch(caller, target, context_id, payload, timeout)`.
5. **DO** checks `agent:{target_b64}:handlers` for `context_id`. If not registered, returns `Err(StoaError::Wake(WakeError::HandlerNotFound))`; Worker maps to 422.
6. **DO** generates `wake_id = ulid::Ulid::new()` (sortable, 26 chars).
7. **DO** writes (in a single DO transaction):
   - `wake:{wake_id}` with state=pending, fields populated
   - `agent:{target_b64}:inbox` append wake_id
8. **DO** sets a storage alarm for `now + timeout_ms` (Cloudflare DO's `state.storage.setAlarm` API).
9. **DO** registers an in-memory resolver keyed by wake_id (Rust: `tokio::sync::oneshot::Sender<Result<WakeResponse, StoaError>>`). The dispatching request `await`s this resolver directly per the in-memory-promises-map pattern resolved in §3.6's pattern verification above. Other requests interleave on the DO; storage state from step 7 remains the durable source of truth.
10. **Target's MCP server** (Sub-PR 2) polls `GET /v1/teams/{team_id}/agents/{target_identity}/inbox?wait=30&limit=10`.
11. **Worker → DO**: `read_inbox(target, 30s, 10)`. DO drains wakes from the target's inbox keyspace. If empty, blocks up to 30s on its own internal arrival notification (signalled by step 7's inbox-append).
12. **DO returns** pending wake summaries; Worker returns 200 with the wake list.
13. **Target processes the wake** (out of Tally's scope; happens in Target's Claude Code session).
14. **Target's MCP server** calls `tally_complete_task(wake_id, response)`.
15. **MCP server → Worker**: `POST /v1/teams/{team_id}/wakes/{wake_id}/complete` with `Authorization: Bearer <target_api_key>` and response payload.
16. **Worker authenticates Target**: same flow as step 3.
17. **Worker → DO**: `complete_wake(wake_id, response, by=target_identity)`.
18. **DO**:
    - Validates `wake:{wake_id}` exists and `state == pending`
    - Validates `by_identity == wake.target_identity`
    - Writes `wake:{wake_id}` with state=completed, response, completed_at
    - Resolves the in-memory promise from step 9 with `Ok(WakeResponse(response))`
    - Cancels the storage alarm from step 8
19. **DO's `dispatch` RPC** (waiting since step 9) unblocks; returns `Ok(WakeResponse(response))`.
20. **Worker** maps DO response to HTTP 200 with wake_id, response, completed_at.
21. **Caller's MCP server** receives the response; `tally_assign_task` returns the result.

**Timeout path** (alarm fires before step 18):

19'. **DO alarm handler** (Cloudflare DO's `alarm()` method):
    - Reads `wake:{wake_id}`
    - If state == pending: writes state=timed_out, completed_at=now
    - Resolves the in-memory promise with `Err(StoaError::Wake(WakeError::TimeoutExpired { timeout: ... }))`
20'. **DO's `dispatch` RPC** unblocks with `Err`.
21'. **Worker** maps to HTTP 408 with `{ "error": "wake timed out", "wake_id": "...", "timeout_seconds": ... }`.

**Late completion** (target tries to complete after timeout):

- DO `complete_wake` finds `wake.state == timed_out`
- Returns `Err(StoaError::Wake(WakeError::Other("wake already in terminal state".to_string())))`
- Worker maps to HTTP 410 Gone with current state in body

### 3.7 Timeout behavior (specific)

**Default**: 30 seconds (matches Cloudflare Worker's default request lifetime on the free plan).

**Range**: 1 to 300 seconds (5 minutes max; aligned with Cloudflare Workers Paid plan's max request duration via `compatibility_flags = ["http_request_timeout"]`).

**Configuration**: per-wake via the request body's `timeout_seconds` field.

**Alarm-based enforcement**: Cloudflare DO's storage alarms fire even if the Worker request that initiated dispatch has been terminated. This guarantees timeout cleanup is durable regardless of Worker lifecycle.

**Late completion**: rejected with 410 Gone. The wake state stays terminal (timed_out); never re-resolvable. The target's MCP plugin can interpret the 410 as "your work was wasted; the caller already gave up."

### 3.8 Inbox polling pattern

**Long-polling**: `GET /inbox?wait_seconds=30` blocks up to 30 seconds awaiting new wakes.

Implementation: the DO maintains an in-memory wait list keyed by identity. When step 7's `agent:{target_b64}:inbox` append happens, the DO resolves all waiting inbox calls for that identity. If no new wakes arrive within `wait_seconds`, the DO returns the empty wake list.

**Recommended client-side pattern** (for the MCP plugin in Sub-PR 2):
- Default to `wait_seconds=30`
- On connection failure, retry with exponential backoff (1s, 2s, 4s, up to 30s)
- On 401 (key revoked), surface fatal error

**Pagination**:
- `limit` default 10, max 100
- If inbox has more than `limit` pending, response includes `"more_available": true`
- Client repolls immediately after a non-empty response to drain
- No cursor needed for MVP — wakes are returned in ULID-sortable order; the client tracks "highest wake_id seen" if needed

### 3.9 Cross-workstream considerations (within Workstream 3)

The runtime depends on Workstream 2's trait revisions:

- **Revision 1 (timeout parameter)**: directly used in the `dispatch` DO RPC signature (§3.3, §3.6, §3.7). Cannot ship runtime without this revision landing first.
- **Revision 2 (`WakeError` enum)**: used in the error mapping (§3.1) and in the wake routing flow (§3.6). The DO returns `StoaError::Wake(WakeError::TimeoutExpired { timeout })` for timeouts and `StoaError::Wake(WakeError::HandlerNotFound)` for missing-handler errors; the Worker maps these to specific HTTP status codes.
- **Revision 3 (doc-comment)**: not consumed by code; pure doc.

Sub-PR 1's Phase 1 implementation can begin against the current trait surface (pinning to `8ac6c9f`) and bump the rev pin as Stoa-repo PRs land. The runtime's error-to-HTTP mapping is the most affected; the timeout parameter has cleaner integration (just thread the value through).

---

## Cross-workstream considerations

### Phase 2A persistence-compatibility

The runtime's `TallyTeamDO` state model (Workstream 3.2) reserves three key prefixes for future per-agent persistence:

- `agent:{identity_b64}:memory`
- `agent:{identity_b64}:history`
- `agent:{identity_b64}:affordances`

MVP doesn't write to these. Phase 2A's spec defines their shape and the trait surface that lets them work cross-runtime. No constraint in the DO storage model prevents adding them — the `agent:{identity_b64}:*` namespace is open-ended.

**Specifically NOT preempted**: the placement decision for persistence (Stoa trait vs higher-level standard vs Tally-specific feature). The reserved key prefixes are an implementation detail of one possible placement; they don't commit to it.

### Phase 2B deliberation-pattern compatibility

The wake-routing flow (Workstream 3.6) treats wake payloads as opaque bytes. The Worker doesn't interpret them; the DO doesn't interpret them. This means:

- A future deliberation board could encode its state in wake payloads
- Tally would route the deliberation-board wakes without any runtime changes
- The deliberation pattern's placement (Stoa primitive vs higher-level standard vs Tally-specific) is unaffected by Sub-PR 1's choices

**Specifically NOT preempted**: the placement decision for the deliberation pattern. The opaque-bytes design is compatible with all three placement options.

### Phase 2B orchestrator-as-agent compatibility

Per Phase 1B spec, the orchestrator (Phase 2B's destination product) is an agent in the team, not a separate router. Sub-PR 1's runtime treats all registered identities as peers; no special "orchestrator" role. When Phase 2B's orchestrator gets implemented, it's an agent with its own identity, API key, and inbox — same flow as any other agent.

**Specifically NOT preempted**: any architectural special-casing for orchestrators in the runtime. The runtime's agent model is uniform.

---

## Sub-PR 1 Phase 1 work plan (high-level)

After Phase 0 close (this document approved and merged to skytale's master), Sub-PR 1's Phase 1 implementation has the following parallel workstreams:

### Workstream A: Tally repo creation (one PR)

Single commit on a fresh `tally` repo establishing the file tree from §1.5. Opens against tally repo's master (which doesn't exist yet — the first commit IS master).

**Repo visibility: public from day one.** `gh repo create nicholasraimbault/tally --public`. Matches Pronoic's existing operational pattern (Skytale and Outset Maps are public). Supports the platform-play strategic framing (Stoa as protocol with multiple implementations; Tally as the first reference one). The Phase 1B spec and tracking issue #444 already publicly reference the eventual tally repo; making it public from creation is consistent with what's already disclosed. Branch protection configured to match skytale's pattern: any repo-file change goes through PR; only `gh issue create` bypasses (per the operational discipline that surfaced during Phase 1B Phase 0 work, specifically PR #445's branch-protection finding).

Includes:
- README, LICENSE (BSL 1.1 with parameters from §1.1), LICENSES/Apache-2.0, .gitignore
- Cargo workspace declaration (members: tally-core, tally-worker)
- Migrated docs (phase-1b-spec.md, phase-1b-sub-pr-1-phase-0.md)
- Initial CI workflow
- wrangler.toml stub
- Empty tally-core and tally-worker crates with placeholder lib.rs files

### Workstream B: Concurrent Stoa-repo PRs (~3 PRs)

In skytale repo, per Workstream 2:

- stoa-trait PR A: timeout parameter on `dispatch`
- stoa-trait PR B: `WakeError` enum
- stoa-trait PR C: doc-comment clarification

Each follows Option B sub-PR discipline. PRs A and B independently land; PR C lands alongside.

Tally Sub-PR 1's Phase 1 code work CAN begin against the pre-revision Stoa surface and bump the rev pin as each Stoa PR merges. Coordination cost: explicit rev-pin bump per Stoa PR.

### Workstream C: Runtime implementation (tally repo PRs)

After Workstream A's repo is created, additional tally repo PRs build out:

1. `tally-core/` types: WakeId, AgentIdentity, TallyError, HTTP request/response DTOs
2. `tally-worker/`: Router setup, Authorization Bearer parsing, DO binding, error→HTTP mapping
3. `TallyTeamDO`: state model (per §3.2), DO RPC methods, alarm handler
4. Integration tests against `wrangler dev` (Cloudflare's local Worker runtime emulator)

### Workstream D: Final Sub-PR 1 PR

The aggregate Sub-PR 1 PR opened against tally repo's master at the end of Workstreams A-C. Includes:
- All commits from Workstreams A and C
- Verification that Workstream B's Stoa-repo PRs have all merged and the rev pin is current
- A working `wrangler dev` setup that runs the runtime locally
- Integration tests passing
- A deployment script (`scripts/deploy.sh`) that the operator runs to push to a Cloudflare account

Sub-PR 1 deployment to Cloudflare is operator-side (Nick deploys to his own account); not part of the PR but verified in Sub-PR 4's dogfooding handoff.

### Dependency graph: Workstream C tasks ↔ Workstream B prerequisites

Tally repo runtime tasks (Workstream C) can begin in parallel with Stoa-repo PRs (Workstream B), but specific merge gates exist:

| Workstream C task | Workstream B prerequisite | Merge gate |
|---|---|---|
| tally-core types: WakeId, AgentIdentity, error DTOs | None | Can begin and merge independently |
| tally-worker/: Router setup, Authorization Bearer parsing, basic DO binding | None | Can begin and merge independently |
| tally-worker/: error→HTTP status mapping (timeout, handler-not-found cases) | Workstream B PR B (WakeError enum) | Cannot merge until B PR B lands |
| TallyTeamDO: dispatch with timeout parameter | Workstream B PR A (timeout param on dispatch) | Cannot merge until B PR A lands |
| TallyTeamDO: state model, RPC methods (other than dispatch) | None | Can begin and merge independently |
| Integration tests against `wrangler dev` | All of Workstream B and prior C tasks | Cannot run until all upstream merges |
| Final Sub-PR 1 PR (Workstream D) | All of Workstream B | Cannot merge until full B sequence lands |

Coordination cost: explicit rev-pin bumps in tally-core/Cargo.toml after each Workstream B PR merges. The bumps are mechanical commits (3 across Workstream B); each unblocks the dependent Workstream C tasks.

### Phase 1B execution log discipline (during Phase 1)

Per the Phase 1B spec's execution-log section, Phase 1 work captures running signals into the migrated `phase-1b-spec.md`:

- **MCP plugin UX pain points**: not yet relevant (Sub-PR 2's work).
- **Stoa trait-surface questions**: Workstream 2 captures the up-front gaps. Phase 1 adds any newly-surfaced gaps as additional Stoa-repo PRs.
- **Persistence-need signals**: not expected to surface during Sub-PR 1 (runtime doesn't touch persistence).
- **Naming candidates**: not expected.

---

## Acceptance criteria for Sub-PR 1 Phase 0 close (this document)

For this document to be considered Phase 0-complete and ready for strategic-layer review:

1. ✅ Three workstreams covered with concrete design notes (§Workstream 1, §Workstream 2, §Workstream 3)
2. ✅ Stoa trait-surface gap survey produces a concrete list of proposed revisions with rationale and compatibility impact (§2.1, §2.3, §2.4)
3. ✅ Tally Cloudflare runtime architecture specifies the 5 HTTP routes (§3.3), the DO state model (§3.2), the auth flow (§3.4), and the wake routing flow (§3.6) at implementation-ready detail
4. ✅ Repo creation Workstream specifies the first-commit contents (§1.5) and CI scaffolding (§1.3)
5. ✅ Cross-workstream considerations confirm extensibility for Phase 2A persistence, Phase 2B deliberation, and Phase 2B orchestrator-as-agent without preempting any of them (§Cross-workstream considerations)
6. ✅ No Phase 1 implementation work has begun
7. ✅ No scope additions beyond what the Phase 1B spec authorizes (license, runtime substrate, WakeRouter implementation, four sub-products, dogfooding pattern, repo creation timing)

**Next step**: strategic-layer review of this document. After approval, a follow-up small PR commits this document to `skytale/docs/specs/phase-1b-sub-pr-1-phase-0.md` (same shape as PRs #445, #447, #450 — small markdown amendment PRs). Sub-PR 1's Phase 1 work begins after the design notes are merged to master.

---

## References

- **Phase 1B spec**: [`docs/specs/phase-1b-spec.md`](./phase-1b-spec.md) on skytale master at `8ac6c9f`
- **Phase 1B tracking issue**: [#444](https://github.com/nicholasraimbault/skytale/issues/444)
- **Option B planning doc**: [`docs/specs/option-b-extraction.md`](./option-b-extraction.md) — precedent for sub-PR Phase 0 discipline
- **Stoa WakeRouter trait**: `stoa/src/wake_router.rs` on skytale master — the trait Tally implements
- **NoopWakeRouter** (deferred-stub reference impl): `stoa-rs/src/wake_router.rs`
- **Tracking issues referenced (not in scope for Sub-PR 1)**: #448 (stoa name resolution), #449 (publishing setup) — both decision-triggered at Sub-PR 4 Phase 0
- **Worker SDK**: https://crates.io/crates/worker — Cloudflare's official Rust SDK for Workers
- **BSL 1.1 template**: https://mariadb.com/bsl11/
