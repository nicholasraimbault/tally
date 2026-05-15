# Integration tests sub-PR — test plan

**Date:** 2026-05-15
**Scope:** tally Phase 0 §9.3 — end-to-end integration tests against the public HTTP API surface
**Status:** Pre-implementation. Scoping decisions locked through one deliberation session.
**Prerequisite:** PR #18 merged (public HTTP API surface; commits `884b074`–`cdf938c` on main)

## Summary

§9.3's Phase 0 spec is 3 lines — "End-to-end tests that exercise the Worker + DO via `wrangler dev`." This PR defines the actual §9.3 scope. Lands ~16 integration tests covering the 6 public routes' happy paths, §3.1 error code mapping, and 3 multi-target scenarios. Architectural decisions from §9.1/§9.2 are locked; this is test-design work, not architectural deliberation.

Different artifact shape than §9.1/§9.2 Phase 0 design notes: scoping decisions + scenario catalog + harness design, not "Architectural decisions" section.

## Scoping decisions (locked)

### Q1: HTTP-only via `wrangler dev`

In-process inherent-method testing would require refactoring `tally-worker`'s `cdylib` shape and TallyTeamDO's Cloudflare-runtime construction — architectural change not justified by test convenience. Lower-layer unit tests (42 existing) cover the inherent-method surface; integration tests cover what HTTP transport adds (routing, auth, JSON deserialization, error mapping, response shape).

### Q2: Real waits for alarm-fire scenarios

Mock-clock requires refactoring 7 `Date::now()` call sites behind an injectable trait — same anti-pattern as Q1. Real waits acceptable if scoped:
- Max 2-3 alarm-fire scenarios
- Minimum-timeout scenarios (1s + alarm slack ≈ 2s per test)
- Assertions on outcome (state transitioned; HTTP 408 returned) not on precise timing
- Total alarm-fire test budget: <10s wall-clock

If real-waits prove flakier than expected during implementation, that's a stop-and-surface event; resolution is reducing scenario count further, not adding mock-clock infrastructure.

### Q3: Shared `TestHarness` helper

Match skytale's e2e crate pattern (`tests/agent_teams_e2e/`): standalone test crate, manually-managed tokio runtime, `TestHarness::setup()` fixture, reqwest in `#[test]` (not `#[tokio::test]`). Per-test setup would duplicate ~30 lines of wrangler-dev/reqwest scaffolding per scenario; shared helper reduces duplication and isolates setup bugs.

### Q4: 4 multi-target scenarios (middle scope)

1. **Same-team multi-agent happy path** — A dispatches to B; B completes; A receives response
2. **Cross-team isolation** — A in T1 dispatches to B in T2; T1's DO has no record of B; returns `HandlerNotFound` → 422
3. **Long-poll wake-up** — B subscribes with `wait_seconds=30`; A dispatches mid-wait; B wakes immediately with the dispatched wake
4. **Inbox-overflow eviction** — dispatch N+1 wakes (N=INBOX_LIMIT=1000); verify oldest evicted, newest accepted

Skip parallel-dispatch race testing: race conditions over HTTP transport are structurally hard to test; the architectural decision (Cloudflare DO single-writer guarantee) is what makes it safe, not test coverage. Race-condition coverage at integration level would be aspirational rather than diagnostic.

### Q5: Middle coverage threshold

~16 integration tests total:
- 6 happy-path scenarios (one per public route)
- 7 error-code scenarios (400/401/403/404/408/410/422 from §3.1; 500 dropped — see below)
- 3 multi-target scenarios (Q4 minus inbox-overflow; see below)

Note: the happy-path "dispatch" and multi-target scenario 1 ("same-team multi-agent") have overlap; the multi-target scenario serves as the canonical happy path for dispatch + complete. Net distinct tests: ~16.

**Two scenarios moved to deferred during review pass:**

- **error_500_internal**: no clean way to inject internal errors over HTTP without compromising the test harness or adding production code that exists only to enable failure injection. The 500 path is exercised every time a panic or unhandled storage error occurs in production; synthetic integration coverage has low diagnostic value. Drop from §9.3 scope.
- **inbox_overflow_eviction**: `INBOX_LIMIT = 1000`; dispatching 1001 wakes is ~50s of HTTP round-trips per test (assuming ~50ms/dispatch). Test-time cost dominates diagnostic value. Unit-test coverage from §9.1 exists for the overflow path's correctness; integration-level coverage is desirable but not load-bearing. Defer to coverage-expansion sub-PR.

**Explicitly deferred to a future coverage-expansion sub-PR (not §9.3's responsibility):**
- error_500_internal (synthesis-path TBD; no clean injection)
- inbox_overflow_eviction (~50s test-time cost; unit-tested at §9.1)
- Parallel-dispatch race conditions
- Mock-clock-enabled timing assertions
- Concurrency scenarios beyond Q4's long-poll wake-up
- DO eviction recovery scenarios (in-flight wake when DO restarts)

If production bugs surface in untested paths, a §9.4 coverage-expansion sub-PR addresses them; not a §9.3 deliverable.

## Test harness design

**Standalone test crate** at `tally/integration-tests/` — outside the workspace per skytale's e2e pattern. Cargo.toml structure:

```toml
[package]
name = "tally-integration-tests"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]  # opt out of workspace

[dependencies]
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
ulid = "1"
base64 = "0.22"
```

**`TestHarness` fixture** (`integration-tests/src/lib.rs`):

```rust
pub struct TestHarness {
    pub base_url: String,
    pub client: reqwest::Client,
    wrangler_process: tokio::process::Child,
}

impl TestHarness {
    /// Starts `wrangler dev` in background; polls /v1/health until ready.
    pub async fn setup() -> Result<Self, HarnessError> { /* ... */ }

    /// Constructs an identity Bearer token (MVP: url-safe-b64 of random bytes).
    pub fn new_identity(&self) -> (Identity, String /* bearer */) { /* ... */ }

    /// Constructs a unique team_id for test isolation.
    pub fn new_team_id(&self) -> String { /* ulid */ }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        let _ = self.wrangler_process.start_kill();
    }
}
```

**Test pattern** (one test per scenario):

```rust
#[test]
fn dispatch_happy_path() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all().build().unwrap();
    let harness = runtime.block_on(TestHarness::setup()).unwrap();
    let team_id = harness.new_team_id();
    let (caller, caller_bearer) = harness.new_identity();
    let (target, target_bearer) = harness.new_identity();
    // ... register target, dispatch from caller, complete from target, verify response ...
}
```

## Scenario catalog (18 total)

### P0 — happy-path coverage (6 tests)

1. **`health_check`** — GET `/v1/health` returns 200 with `{"status":"ok","version":"..."}`
2. **`register_handler`** — POST `/v1/teams/{T}/agents/{A}/register` returns 201; subsequent dispatches to A succeed
3. **`unregister_handler`** — DELETE `/v1/teams/{T}/agents/{A}/handlers/{C}` returns 204; subsequent dispatches to (A, C) fail with HandlerNotFound
4. **`dispatch_and_complete`** — POST `/v1/teams/{T}/wakes` blocks; target POSTs `/wakes/{wake_id}/complete`; dispatcher receives response with `wake_id`, `response`, `completed_at`
5. **`read_inbox_immediate`** — GET `/v1/teams/{T}/agents/{A}/inbox?wait_seconds=0` returns immediately with pending wakes (or empty if none)
6. **`read_inbox_with_limit`** — `?limit=N` returns at most N wakes; `more_available: true` when more exist

### P1 — §3.1 error code coverage (7 tests)

1. **`error_400_malformed`** — POST `/v1/teams/{T}/wakes` with malformed JSON returns 400
2. **`error_401_missing_bearer`** — request without `Authorization` header returns 401 with `{"error":"..."}`
3. **`error_401_invalid_bearer`** — request with non-decodable Bearer returns 401
4. **`error_403_identity_mismatch`** — request to `/agents/{A}/inbox` with Bearer for B (≠A) returns 403
5. **`error_404_wake_not_found`** — POST `/wakes/{nonexistent_id}/complete` returns 404 with `wake_id` in body
6. **`error_408_timeout`** *(alarm-fire scenario — real wait)* — dispatch with `timeout_seconds=1`; nothing completes; after ~2s returns 408 with `wake_id` + `timeout_seconds`
7. **`error_410_already_terminal`** — complete a wake; attempt to complete it again returns 410 with `wake_id`
8. **`error_422_handler_not_found`** — dispatch to unregistered (target, context_id) returns 422 with `context_id` in body

`error_500_internal` deferred per Q5 — see "Two scenarios moved to deferred during review pass" above.

### P2 — multi-target scenarios (3 tests; Q4 minus inbox-overflow)

1. **`multi_agent_same_team`** *(canonical happy path for dispatch+complete)* — A in T1 dispatches to B in T1; B completes; A receives. Overlaps P0/#4 above; serves as the deduplicated canonical happy path.
2. **`cross_team_isolation`** — A in T1 dispatches to B in T2; T1's DO has no record of B; returns 422
3. **`long_poll_wake_up`** *(alarm-fire-adjacent — real wait)* — B GETs inbox with `wait_seconds=30`; A dispatches at T+1s; B's poll returns at ~T+1s with the dispatched wake (assertion loose: returns before T+5s with non-empty inbox)

`inbox_overflow_eviction` deferred per Q5 — see "Two scenarios moved to deferred during review pass" above.

**Alarm-fire scenarios summary:** `error_408_timeout` (P1#6) + `long_poll_wake_up` (P2#3). Both use real waits. Total alarm-fire wall-clock ≈ 5-7s within the <10s budget.

## What's explicitly deferred (not silently uncovered)

- Parallel-dispatch race conditions (Cloudflare DO single-writer guarantees atomicity at the architectural level; race testing over HTTP is structurally hard and would be aspirational)
- Mock-clock-enabled precise timing assertions (would require architectural refactor of `Date::now()` call sites)
- DO eviction recovery scenarios (in-flight wakes when DO restarts mid-dispatch)
- Stress / load tests
- Multi-region behavior (Tally is single-region per Phase 1B spec)
- Cross-protocol scenarios (Phase 2 surface)

A future §9.4 coverage-expansion sub-PR addresses gaps if production bugs surface in these paths.

## Implementation scope

**Files created:**

- `tally/integration-tests/Cargo.toml` (new; standalone, opts out of workspace)
- `tally/integration-tests/src/lib.rs` (new; `TestHarness` fixture + helpers)
- `tally/integration-tests/tests/happy_path.rs` (P0 scenarios)
- `tally/integration-tests/tests/error_codes.rs` (P1 scenarios)
- `tally/integration-tests/tests/multi_target.rs` (P2 scenarios)

**Files modified:**

- `.github/workflows/ci.yml` (or equivalent): add an `Integration tests` job that installs Node.js + Wrangler (`npm install -g wrangler`), builds the worker, runs `cargo test -p tally-integration-tests` against the started `wrangler dev`. Realistic budget ~1 minute (45-75s) cold-cache: Wrangler install ~10-15s; `worker-build` worker build ~10-20s; `wrangler dev` startup to /health-ready ~3-5s; 16 sequential HTTP tests at ~1-2s each ≈ 16-32s; alarm-fire real-wait scenarios ~5-7s (P1 #6 + P2 #3). Cached-runner builds compress this somewhat. Worth being explicit: integration tests add ~1 minute to the build pipeline; that's the cost of HTTP-end-to-end coverage.

**Dependencies (test crate only):** reqwest, serde, serde_json, tokio, ulid, base64. No production-crate impact.

Estimated diff: ~600-900 lines (lib.rs harness ~150 lines; 3 test files ~150-200 lines each; Cargo.toml + CI workflow update ~50 lines).

## Verification

Existing 7 CI checks must continue to pass:
- `cargo fmt --check`
- `cargo check --target wasm32-unknown-unknown -p tally-worker`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace` (unit tests; standalone integration crate not part of workspace)
- `cargo doc --workspace --exclude tally-worker --no-deps`
- `cargo doc --package tally-worker --target wasm32-unknown-unknown --no-deps`
- `cargo machete`

Plus new:
- **Integration tests** — `cargo test -p tally-integration-tests` against `wrangler dev` background process

Local verification before PR open: run the full set including the new integration-tests step.

## Mid-implementation correction note

Implementation of the test scenarios surfaced four PR #18 production-code gaps that no §9.2 test exercised. The first three came from static doc-review during implementation; the fourth surfaced empirically when CI tried to actually run `worker-build`:

- **DO binding name mismatch** (`tally-worker/src/lib.rs:108`): `DO_BINDING = "tally-team"` did not match `wrangler.toml`'s declared `name = "TALLY_TEAM_DO"`. `env.durable_object()` does case-sensitive property lookup; the mismatch would return `undefined` at runtime. Fixed: align to `"TALLY_TEAM_DO"`.
- **Self-referential `script_name` in `wrangler.toml`**: the `script_name = "tally"` field on the DO binding self-references the Worker's own name. Cloudflare's `script_name` is for cross-Worker DO references; self-reference confuses `wrangler dev`'s binding resolution. Fixed: removed.
- **`id_from_string` vs Phase 0 §3.2's URL-safe-b64 team_id semantic** (`tally-worker/src/lib.rs:185`): `id_from_string` requires a 64-hex DO ID from `State::id()`'s stringified form — incompatible with caller-provided URL-safe identifiers. Fixed: switched to `id_from_name`, which derives the DO ID via internal SHA-256 hash of any UTF-8 name. Test harness's `new_team_id()` simplified to return a ULID string (matches §3.2's URL-safe-b64 intent).
- **`worker-build` invoked from workspace root** (`wrangler.toml` + CI workflow): the workspace's root `Cargo.toml` has only `[workspace]` (no `[package]`); `worker-build` rejects it with `missing field 'package'`. The build must run from the `tally-worker/` package directory. Fixed: `wrangler.toml` `[build] command` and CI's `Build worker (release)` step now `cd tally-worker` (CI uses `working-directory:` step option); `wrangler.toml` `main` updated to `tally-worker/build/worker/shim.mjs` to point to the correct output location.

Corrections land as part of this PR with explicit acknowledgment, matching the §9.1 Decision 6.6.4 + §9.2 TallyError patterns (mid-implementation corrections to a prior sub-PR's locked decisions surfaced during the next sub-PR's work, folded into that next PR rather than carved into a separate retroactive fix-PR). The integration tests in this PR are the surface that catches and exercises these corrections; bundling them is more honest than separating provenance.

The fourth gap is also a Layer 4 data point: static doc-review (the agent's first-pass surfacing) caught 3 of the 4; CI's actual execution caught the 4th. Worth carrying forward to future Pattern C work — static review of "does this look right against docs" remains valuable but doesn't fully substitute for "actually run the thing on the target."

## Methodology note

Test plan is a different artifact than Phase 0 design notes. No "Architectural decisions" section; no Operational Notes subsections; no System Properties; no Layer-N verification framing. The work shape is scoping + cataloging + mechanical implementation against the catalog. Pattern C's lock-architectural-commitments cadence doesn't apply here — applying it would be over-elaboration.

The §9.1/§9.2 lessons that DO carry forward:
- Full CI surface verification (not just spec'd commands) — added explicitly above
- Stop-and-surface for genuine scoping gaps (e.g., if real-wait alarm tests prove flaky beyond budget) — not for small test-design choices
- Mid-implementation corrections to prior sub-PRs' decisions are folded into the surfacing sub-PR's branch with honest acknowledgment — see the "Mid-implementation correction note" above for the three PR #18 corrections that landed via this PR
