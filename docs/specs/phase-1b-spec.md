# Phase 1B — Tally: First Product on Stoa

**Status**: Phase 1 in progress. Spec migrated from `skytale/docs/specs/phase-1b-spec.md` (skytale master at `86ab999`) to `tally/docs/specs/phase-1b-spec.md` as part of Sub-PR 1 Workstream A's force-reset commit on 2026-05-13. The skytale-side copy will be removed in a follow-up cleanup PR. This file is now the canonical Phase 1B implementation reference.

**Precedent**: This spec follows the structural template of `docs/specs/option-b-extraction.md` (issue #431, completed 2026-05-13). Option B extracted the Stoa protocol layer that Tally implements; Phase 1B is the first concrete product built on that layer. The Option B spec's post-graduation shape (status header, sub-PR sequence, acceptance criteria, completion log, Phase 0 corrections summary) is the canonical reference for how this spec should read once Phase 1B completes.

**Skytale master HEAD at investigation time**: `962c92a` (post-#442 merge, post issue #431 close).

---

## Purpose of this document

The Phase 1B tracking issue (to be filed; draft at `phase-1b-tracking-issue-draft.md`) is the public commitment and strategic reasoning. This document is the code-grounded implementation reference: where Tally fits in the Skytale + Stoa architecture, what the four sub-products look like at the file level, which Stoa trait surface each sub-product touches, what the dogfooding pattern is, and what commercial/operational questions Phase 2 needs to answer that Phase 1B's architecture has to remain compatible with.

---

## Strategic context

Skytale's Option B extraction (issue #431) decomposed the agent-coordination stack into four crates:

- **skytale-base** — infrastructure (HTTP API client, MLS engine, BaseError)
- **stoa** — protocol interface (TeamPrimitive, RolePackHandler, AuditTrail, WakeRouter traits + StoaError)
- **stoa-rs** — Rust reference implementation of the Stoa traits
- **skytale-sdk** — SDK orchestrator (transport, channel, trust layer, PyO3 bindings)

The structural extraction's Tier 2 acceptance criterion (`cargo tree --invert skytale-sdk` returns "did not match any packages" from inside `stoa-rs/`) confirms that third-party implementations of the Stoa protocol can be built against `stoa-rs` alone, without reaching into `skytale-sdk`. Tally is the first concrete consumer that exercises this pathway.

Specifically, Tally is a **runtime that implements Stoa's `WakeRouter` trait on Cloudflare Workers + Durable Objects**, plus the surrounding product surface — a Claude Code MCP plugin, a CLI for operators, and documentation. The `WakeRouter` trait was defined as provisional in sub-PR 3 of issue #431 with the explicit framing that the first concrete consumer would surface refinements to the trait shape. Phase 1B is that first consumer.

The minimum-viable end-to-end test the architecture has to pass: **two Claude Code sessions running on the same machine, in the same Stoa team, coordinating via Tally's runtime — driven by either human users or LLM agents** (Phase 1B closes on human-driven verification; LLM-driven coordination is Phase 2 product validation). If that dogfooding pattern works, the structural commitments hold. If it doesn't, Phase 1B has surfaced an architectural gap that requires either a Stoa trait revision (would feed back into a Stoa sub-PR) or a Tally-side workaround.

### Why Tally follows Option B

Option B finished 2026-05-13; Tally builds on the Stoa protocol that Option B locked. The two specs share a structural template intentionally: future readers tracing how Skytale's products compose should see the same shape twice — Option B for the platform extraction, Phase 1B for the first product on the platform. Sub-PR sequencing discipline (Phase 0 design → strategic-layer approval → Phase 1 implementation → fidelity-check review → merge), Refs/Closes trailer hygiene, planning-doc-graduation patterns — all of these are inherited from Option B and applied to Phase 1B.

### What Tally is NOT (Phase 1B scope discipline)

Tally Phase 1B is the runtime + MCP plugin + CLI + docs. It is **not**:

- The Tally Code / Tally Repos product-family framing (strategic positioning for a later phase; not load-bearing for Phase 1B)
- A native Tauri app (Phase 2+ if at all)
- Cloud-managed executors beyond the MVP coordination pattern
- Subscription billing infrastructure (Phase 2)
- A migration path from the deprecated `skytale_sdk[orchestration]` MCP server (separate effort)

Sticking to the four sub-products keeps Phase 1B bounded and surfacing-driven. Strategic ambition for Tally Code / Tally Repos belongs in their own future specs.

---

## What's locked (do not re-deliberate)

These commitments are firm. Phase 0 doesn't re-litigate them; later phases revise only with explicit strategic-layer approval.

1. **License**: BSL 1.1 across all Tally sub-products. Same pattern as Skytale's "open source + optional paid managed hosting" approach (Bitwarden-style). Apache 2.0 for stoa/stoa-rs/skytale-base/skytale-sdk; BSL 1.1 for Tally.

2. **Runtime substrate**: Cloudflare Workers + Durable Objects. Phase 0 settles whether R2/Queues/KV are additionally needed; the Workers+DO base is non-negotiable.

3. **Stoa trait implementation**: Tally implements `stoa::wake_router::WakeRouter`. `AuditTrail` follows in a later sub-PR if scope permits; `TeamPrimitive` and `RolePackHandler` are NOT implemented by Tally (those are Stoa's responsibility — Tally's TallyTeamDO consumes them, doesn't reimplement them). Tally is permitted to surface trait-shape refinements to Stoa as a feedback loop, per the provisional marking sub-PR 3 placed on WakeRouter.

4. **Four sub-products**:
   - **Tally Cloudflare runtime** — Worker + Durable Object code in the tally repo
   - **`@skytalesh/tally-mcp`** — npm-distributed Claude Code MCP server plugin
   - **Tally CLI** — Rust binary for operators (deployment, team provisioning, status inspection)
   - **Tally documentation** — getting-started + reference docs in the tally repo

5. **MVP dogfooding pattern**: Two Claude Code instances running on the same machine, in the same Stoa team, coordinating via the deployed Tally runtime. Each instance hosts its own `@skytalesh/tally-mcp` server; the MCP tools route through Tally's `WakeRouter` implementation; the coordination round-trips through Cloudflare. This is the smallest end-to-end test that exercises all four sub-products simultaneously.

6. **Out of scope for Phase 1B** (deferred to Phase 2 or later):
   - Native Tauri app
   - Cloud-managed executors beyond the MVP coordination pattern
   - API key management for non-subscription auth
   - Billing infrastructure
   - Multi-tenant Cloudflare deployment (single operator account for MVP)

7. **Repo location**: Tally lives in a new `tally` repo (not yet created). This Phase 0 spec lives in `skytale/docs/specs/`; on Sub-PR 1 of Phase 1B, the spec migrates to `tally/docs/specs/` and the `tally` repo is created with the same commit.

8. **Naming**: Product name for Phase 1B is "Tally" (singular). The "Tally Code / Tally Repos" product-family framing is reserved for a later strategic spec; Phase 1B doesn't use those names.

---

## Architecture

### Sub-product 1: Tally Cloudflare runtime

**Deployment shape**: Single Worker fronting HTTP API + WebSocket connections from registered agents. The Worker itself is stateless dispatch; all state lives in Durable Objects.

**Durable Object class**: `TallyTeamDO`. One DO instance per Stoa team. Holds:

- **Registered agent state**: which `Identity` bytes have registered handlers for which `context` byte sequences (per Stoa `WakeRouter::register_handler`)
- **Routing state**: the (identity, context) → handler mapping that `dispatch` consults
- **Pending wake inbox**: wakes that haven't been processed yet (per-agent FIFO queue)
- **Wake response futures**: in-flight wakes waiting for response payloads
- **Audit-trail buffer**: per-team audit events (deferred to Sub-PR 2 of Phase 1B sequence if Tally implements AuditTrail; not in MVP)

Single DO class instead of split (e.g., TeamRouterDO + AgentInboxDO) because:
- Per-team state is small; one DO handles it
- Routing requires consulting registered handlers AND consulting inbox state — keeping both in one DO avoids cross-DO RPC
- If contention becomes a problem post-MVP, splitting is a straightforward refactor

**Additional Cloudflare primitives**:
- **R2**: not needed for MVP. Add later when AuditTrail implementation lands and audit-chain blobs need bulk storage. Hot audit data lives in DO storage; cold data migrates to R2.
- **Queues**: not needed for MVP. Durable Objects hold their own queue semantics natively.
- **KV**: not needed for MVP. All MVP state (including the API key list) lives in Workers + Durable Objects. KV surfaces in the Phase 2 questions section if short-lived auth tokens land later — the runtime's validation primitive supports both long-lived (MVP) and short-lived (Phase 2) modes without architectural rewrite.
- **Workers AI / Vectorize**: not needed; Tally doesn't run models.

**HTTP surface (Worker → DO)**:
- `POST /v1/teams/{team_id}/agents/{identity}/register` — WakeRouter::register_handler
- `DELETE /v1/teams/{team_id}/agents/{identity}/handlers/{context_id}` — WakeRouter::unregister_handler
- `POST /v1/teams/{team_id}/wakes` — WakeRouter::dispatch (synchronous; Worker holds the connection until DO completes the dispatch)
- `GET /v1/teams/{team_id}/agents/{identity}/inbox` — read pending wakes for this agent (read-only)
- `POST /v1/teams/{team_id}/wakes/{wake_id}/complete` — agent completes a wake and returns the response payload

**WebSocket surface (Worker ↔ Agent's MCP server)**: optional for MVP. Initial implementation can poll the inbox via HTTP; WebSocket push notifications are an optimization for sub-PR follow-ups.

**State model**: WakeRouter trait is sync from the caller's perspective. Tally's implementation routes the synchronous `dispatch` call through:

1. Caller's Tally client makes an HTTP `POST /v1/teams/{team_id}/wakes` and blocks on the response
2. Worker forwards to TallyTeamDO
3. DO checks the routing state for a registered handler matching (target, context)
4. DO writes the wake to the target agent's inbox slot in DO storage
5. DO awaits the target agent's `complete` HTTP call (with a timeout; if the agent doesn't respond, DO returns an error)
6. Target agent's MCP server polls (or receives WebSocket push) the inbox, processes the wake, calls `complete`
7. DO returns the response payload to the original caller's HTTP request
8. Worker returns to caller; `dispatch` returns

The DO sits between the caller's "synchronous" dispatch and the target's "asynchronous" inbox polling, providing the routing primitive. The synchrony commitment from Stoa's trait holds at the API boundary; the implementation is event-loop asynchronous inside Cloudflare.

**Trait-surface refinement questions for Phase 1B Sub-PR 1**:

- Does WakeRouter::dispatch need a `timeout: Duration` parameter? Without one, the Cloudflare implementation has to pick an arbitrary upper bound for waiting; with one, the trait surface gets richer. **Recommended**: surface this as a Stoa trait revision in a sub-PR that runs concurrent with Phase 1B Sub-PR 1's Cloudflare runtime work.
- Does `WakeRouter` need an async variant? Stoa's planning doc anchored on sync APIs for codebase consistency; Cloudflare's natural model is async. Tally's sync surface bridges via blocking HTTP, but it's worth asking if Stoa should grow an async cousin trait. **Recommended**: surface as a question in Sub-PR 1 Phase 0; don't preemptively change.

### Sub-product 2: `@skytalesh/tally-mcp` Claude Code MCP plugin

**Distribution**: npm package `@skytalesh/tally-mcp`. Same naming convention as Skytale's existing `@skytalesh/sdk` for TypeScript.

**Implementation language**: TypeScript. (The Orchestration MCP server it supersedes is Python-based via `skytale_sdk.integrations._orchestration`; Tally's MCP server is fresh implementation, not a port.)

**Why TypeScript not Rust/Python**: Claude Code's MCP ecosystem is dominantly Node.js-native; npm distribution is the lowest-friction path for users. Rust via napi would work but adds binary-distribution complexity for marginal user benefit. Python via PyPI is another option but cross-language consistency with Tally CLI (Rust) is better preserved by separating "agent-facing tooling" (TypeScript/npm) from "operator-facing tooling" (Rust/CLI).

**MCP tool surface**: Three core tools + one read-only inspection tool. Intentionally bounded — the agent's MCP server handles in-session coordination; out-of-session registration/teardown is the CLI's job.

| Tool | Purpose | I/O | Stoa surface |
|---|---|---|---|
| `tally_assign_task` | Send a task to a teammate (or any agent matching role+context) | in: target identity/role, context, payload (text); out: wake_id | WakeRouter::dispatch |
| `tally_inbox` | List pending wakes received by this agent | in: (optional) filter; out: list of pending wakes | Tally HTTP `GET /inbox` (wraps WakeRouter state) |
| `tally_complete_task` | Mark a wake complete and return response | in: wake_id, response payload; out: ack | Tally HTTP `POST /complete` (wraps WakeRouter response) |
| `tally_team_status` | Read team's current state (members, active tasks). Read-only operation mirroring `tally teams status` at the CLI. Gives the agent visibility into team state during task assignment without leaving the MCP context — the CLI version is for operators, the MCP version is for agents acting on team state during a session. | in: team_id; out: status snapshot | Stoa TeamPrimitive::members + Tally inbox metadata |

Four tools. Subscribe / unsubscribe are explicitly NOT in the MCP surface — they're handled by `tally agents register` / `tally agents unregister` at the CLI level. The agent's MCP server doesn't function without an API key issued during CLI registration; subscribing again via MCP after registration is a no-op surface. The CLI handles registration; the MCP plugin handles in-session coordination.

**Tools NOT in MVP**:
- Role-pack management (Tally CLI handles this)
- Audit-trail querying (deferred to Phase 1B Sub-PR with AuditTrail impl)
- Team-administrative operations (member add/remove is via skytale CLI, not Tally MCP)

**Auth model**: Each `@skytalesh/tally-mcp` instance carries a **long-lived** Tally API key (env var configuration; same pattern as the deprecated Orchestration MCP server). The API key authorizes (identity, team) tuples; the Worker validates on each request via a stored key list in the TallyTeamDO. Keys are issued by `tally agents key issue` during agent registration; revocation is `tally agents key revoke`.

Why long-lived for MVP: simpler than short-lived tokens. Sufficient for dogfooding (single operator, trusted agents). Consistent with Orchestration's predecessor pattern. Phase 2 commercial work (subscription tying, automated revocation) can layer short-lived tokens on top without changing the runtime's validation primitive — API keys carry an issuer field (MVP issuer is `"tally-cli-local"`; Phase 2 issuers extend the validation strategy without breaking existing keys).

### Sub-product 3: Tally CLI

**Distribution**: Rust binary, `tally`, distributed via cargo install initially and via packaged installers (homebrew, apt) post-MVP.

**Relationship to skytale CLI**: Complementary, not redundant. Skytale CLI manages Stoa teams (cryptographic primitives, member management, role packs). Tally CLI manages Tally state on top of existing Stoa teams.

Workflow:
1. User creates a Stoa team via `skytale teams create ...`
2. User adds members via `skytale teams invite ...`
3. User then runs `tally teams init <team_id>` to provision Tally state for the team
4. Agents register with `tally agents register --team <team_id> --identity <bytes>`
5. Agents run their Claude Code instances with `@skytalesh/tally-mcp` configured against the Tally API key

**Structural dependency model**: Tally CLI depends on `skytale-sdk` for team-management concerns (creating Stoa teams, inviting members, reading roster state). Reusing the existing SDK orchestration avoids reimplementing sdk-level patterns at the binary level — operators already have skytale-sdk installed (it's the same tooling that creates teams in the first place).

The Tier 2 architectural intent is **third-party language bindings against the runtime**. The Worker code in Sub-product 1 is what those bindings must build against without skytale-sdk — that's where Tier 2 is architecturally meaningful, and the Tier 2 verification (`cargo tree --invert skytale-sdk` from the runtime's stoa-rs-consuming layer) runs against the runtime's Worker code specifically, NOT against the CLI. Operator tooling talking to Skytale's HTTP API doesn't have the same constraint; forcing the CLI to reimplement sdk-level patterns would extend Tier 2 into a domain where it costs without benefit.

The Tally CLI's Cargo.toml will declare:
- `stoa-rs = { path = "..." }` or `{ git = ... }` — CLI uses team primitive types
- `skytale-sdk = { path = "..." }` — CLI uses SDK orchestration for team management (the addition that distinguishes Tally CLI from the Worker code's dep shape)
- `stoa = { path = "..." }` — transitive through both above, but explicit for clarity
- `skytale-base = { path = "..." }` — transitive through skytale-sdk, but explicit for clarity
- `tally-core` — new local crate in the tally repo for Tally-specific Cloudflare-API calls (the part the CLI shares with the runtime client logic)

**Commands**:

| Command | Level | Purpose |
|---|---|---|
| `tally init` | Operator | Configure Cloudflare account + deployment target |
| `tally deploy` | Operator | Deploy the Worker code to Cloudflare |
| `tally destroy` | Operator | Tear down a deployment |
| `tally teams init <team_id>` | Team | Provision TallyTeamDO for an existing Stoa team |
| `tally teams status <team_id>` | Team | Inspect routing state, inbox depth, recent wakes |
| `tally teams delete <team_id>` | Team | Tear down a team's Tally state (keeps the Stoa team) |
| `tally agents register --team <id> --identity <bytes>` | Agent | Register this agent's identity with Tally's WakeRouter |
| `tally agents unregister --team <id> --identity <bytes>` | Agent | Clean shutdown |
| `tally agents key issue --team <id> --identity <bytes>` | Agent | Issue an API key for this agent's MCP server |
| `tally version` | Diagnostic | Print Tally version, Stoa trait surface version, runtime endpoint |

Approximately 10 commands. Slightly larger surface than skytale CLI's team commands; appropriate for an operator-tool binary.

### Sub-product 4: Tally documentation

**Lives in**: `tally/docs/` after Sub-PR 1 of the Phase 1B sequence creates the repo. During Phase 0, this spec doc itself is the only documentation artifact; later sub-PRs add user-facing docs.

**Documentation surface**:
- `README.md` — Tally's elevator pitch, install, quickstart (the two-Claude-Code-instances dogfooding pattern as a worked example)
- `docs/getting-started.md` — Step-by-step from skytale account setup → Tally deployment → MCP plugin install → first wake
- `docs/architecture.md` — How Tally implements Stoa's WakeRouter; links to the Stoa trait docs upstream
- `docs/cli-reference.md` — Per-command documentation for Tally CLI
- `docs/mcp-tools.md` — Per-tool documentation for `@skytalesh/tally-mcp`
- `docs/deployment.md` — Cloudflare account setup, Worker deployment, custom domains
- `docs/troubleshooting.md` — Common failure modes (auth errors, DO contention, MCP install issues)

**Lives in tally repo**: not on docs.skytale.sh. Phase 2 may merge Tally docs into docs.skytale.sh as a separate section; Phase 1B keeps them in-repo for simplicity.

---

## Sub-PR sequence

Modeled on Option B's 7-sub-PR sequence (#433 → #442, completed 2026-05-13). Phase 0 for each sub-PR is its own design-and-surface step; Phase 1 implements; strategic-layer review before each merge.

| Sub-PR | Scope | Stoa trait touched | Notes |
|---|---|---|---|
| 1 | **Tally Cloudflare runtime — MVP** — Worker + TallyTeamDO + the HTTP surface enumerated above. Creates the `tally` repo. Migrates this spec from skytale to tally. Implements `WakeRouter` for the synchronous dispatch pattern. Phase 0 includes a dedicated **"Stoa trait-surface gap survey"** step that produces concrete proposed trait revisions before implementation begins. | WakeRouter | Largest sub-PR. Repo creation + foundational runtime. |
| 2 | **`@skytalesh/tally-mcp` plugin** — TypeScript Node.js package implementing the 4 MCP tools enumerated above. npm-packaged under the canonical `@skytalesh` org (matches the existing `@skytalesh/sdk` package). Distinct sub-PR because the MCP plugin can be developed and tested independently of the runtime (using a mock Tally HTTP server). | WakeRouter (consumer) | npm publish in this sub-PR's release flow. |
| 3 | **Tally CLI** — Rust binary with the ~10 commands enumerated above. Depends on stoa-rs + skytale-sdk + the new tally-core crate. | WakeRouter (consumer); reads TeamPrimitive | Tier 2 verification runs against the runtime's Worker code (Sub-product 1), not the CLI — operator tooling reusing skytale-sdk doesn't violate the architectural intent. |
| 4 | **Tally documentation** — README, quickstart, architecture, CLI reference, MCP tools reference, deployment guide, troubleshooting. Verifies the dogfooding pattern end-to-end as the closing test. | (none — docs only) | Closes the Phase 1B tracking issue. |

Approximate ordering: sub-PRs 1, 2, 3 can substantially run in parallel after Sub-PR 1's foundation lands. Sub-PR 4 (docs) closes the issue with the dogfooding verification.

### Stoa trait-surface gap survey (Sub-PR 1 Phase 0 step)

Sub-PR 1's Phase 0 includes a **dedicated trait-surface gap survey** as an explicit step. Before implementation begins, Sub-PR 1's Phase 0 enumerates every `WakeRouter` trait change the Cloudflare implementation needs. Each gap surfaces as a concrete proposed revision (signature change, new method, new associated type, etc.) with rationale — for example:

- Does `WakeRouter::dispatch` need a `timeout: Duration` parameter? Without one, the Cloudflare implementation has to pick an arbitrary upper bound for waiting.
- Does `WakeRouter` need an async variant? Stoa anchored on sync APIs; Cloudflare's natural model is async.
- Are there error variants `StoaError::Wake` needs that don't exist yet (e.g., `TimeoutExpired`, `HandlerNotFound`)?
- Does the trait need a way to express "fire and forget" dispatch alongside the existing blocking dispatch?

Stoa-side PRs implementing the revisions land **in parallel** with Tally Sub-PR 1's Phase 1 code work. The two repos coordinate via the trait-surface specification produced in Sub-PR 1's Phase 0 — Tally writes code against the agreed revised surface; Stoa PRs land that surface in the upstream repo on the same coordination boundary.

This eliminates the sequential dependency that would otherwise force Tally Sub-PR 1 to wait on Stoa PR merges. The dual-PR discipline stays; coordination cost drops because the trait revisions are surfaced upfront, not discovered during implementation. The provisional marking sub-PR 3 of #431 placed on WakeRouter is the contract that permits this feedback loop.

### Estimated diff scale per sub-PR

- Sub-PR 1: ~1500-2500 LOC. New repo + Worker + DO + HTTP routes + initial tests + CI.
- Sub-PR 2: ~500-800 LOC. TypeScript MCP server + package manifest + tool definitions.
- Sub-PR 3: ~800-1200 LOC. Rust CLI + tally-core crate.
- Sub-PR 4: ~400-700 LOC of markdown.

Total estimate: ~3000-5000 LOC across Phase 1B. Comparable to Option B's ~5000 LOC across 7 sub-PRs.

---

## Acceptance criteria

### Structural (per-sub-PR + final)

Per-sub-PR (matching the Option B pattern):
1. Sub-PR Phase 0 design notes surfaced and strategic-layer-approved
2. Sub-PR Phase 1 implementation built against the approved design
3. CI passes for both the tally repo's checks AND any cross-repo verification (e.g., stoa trait-surface compat)
4. Cryptographic exclusion test in Skytale's tests/agent_teams_e2e still passes (canonical no-regression — Tally must not break Stoa's existing invariants)
5. PR body includes `Refs #444` (NOT `Closes`) for sub-PRs 1-3; `Closes #444` for sub-PR 4

Final (at Phase 1B close):
6. All four sub-products deployed/published:
   - Tally runtime live on a Cloudflare account
   - `@skytalesh/tally-mcp` published to npm
   - Tally CLI buildable from the tally repo
   - Documentation complete in tally repo
7. Skytale's `cargo tree --invert skytale-sdk` from `stoa-rs/` still returns "did not match any packages" — Tally must not have caused Tier 2 regression
8. Skytale repo unaffected by Tally's creation; no Tally code lives in the Skytale tree

### Product validation (the load-bearing acceptance criterion)

9. **Dogfooding pattern works end-to-end**: Two Claude Code sessions running on the same machine, in the same Stoa team, coordinating via Tally's runtime. Each session may be driven by a human user OR by an LLM agent — the architecture supports both modes equivalently. Phase 1B closes on successful verification in either mode (human-driven is sufficient).

   **Concrete test flow (human-driven mode, sufficient for Phase 1B close)**:
   - Two Claude Code instances configured with `@skytalesh/tally-mcp`
   - User A in Instance A types a task description; Instance A calls `tally_assign_task` targeting Instance B's identity
   - Tally routes the wake; Instance B receives notification
   - User B in Instance B processes the task; Instance B calls `tally_complete_task` with the result
   - Instance A's original assignment returns the result
   - Session log recorded in the tally repo

   **LLM-driven coordination (Phase 2 product validation, NOT gating Phase 1B close)**:
   - The same flow with autonomous LLM agents driving both instances
   - Surfaces a different class of architectural requirements (agent-to-agent autonomy, error handling under autonomy, multi-step coordination chains)
   - Validated separately when Phase 2 work surfaces the autonomous-agent product surface

Failure to verify (9) in the human-driven mode means Tally has shipped a runtime that doesn't actually do what it promises. The Phase 1B sequence does not exit without explicit (9) verification, parallel to Option B's Tier 2 verification discipline.

---

## Open commercial/operational questions (Phase 2 surface)

These don't need answers in Phase 1B. They need to be IN the spec so Phase 2 work has them as starting questions, not surprises. Phase 1B's architectural choices should remain compatible with the eventual answers; where they constrain Phase 2's options, this section names the constraint.

### Cloudflare account ownership

MVP runs on a single operator-owned Cloudflare account (Nick's, for the dogfooding pattern). Phase 2 commercial path probably involves multi-tenant Cloudflare hosting — one Cloudflare account hosting multiple customer teams. This affects:
- **DO naming**: today's `team_id`-keyed DO instances need tenancy-aware namespacing (e.g., `tenant_id:team_id`)
- **Resource limits**: Cloudflare per-account limits become per-tenant limits in multi-tenant; need accounting
- **Operator vs. customer responsibilities**: in single-tenant, operator == customer; in multi-tenant, separation needed

Phase 1B implication: the TallyTeamDO naming scheme should already support a tenancy prefix even if Phase 1B sets it to a constant. Lower migration cost later.

### Self-hosted vs Pronoic-managed ToS

BSL 1.1 prevents commercial competitors from running Tally as a service. But Tally itself is going to be run as a service by Pronoic. What's the ToS shape for:
- Self-hosted Tally (a developer runs their own Cloudflare deployment for their own use)
- Pronoic-managed Tally (subscription on Pronoic-operated Cloudflare account)

BSL 1.1 specifically permits self-hosted internal use. The ToS for Pronoic-managed deployment is a Phase 2 commercial document. Phase 1B implication: none architectural; the same Tally code runs either way.

### Authentication for agents and CLIs

MVP uses long-lived API keys issued by Tally CLI during agent registration. Each `@skytalesh/tally-mcp` instance is configured with one API key. The Worker validates API keys on each request via a stored key list in the TallyTeamDO.

Phase 2 commercial path probably needs subscription-tied auth (Pronoic-issued tokens that revoke when subscriptions lapse). Short-lived tokens with KV-backed validation is one approach; the runtime's validation primitive needs to support both long-lived (MVP) and short-lived (Phase 2) modes without architectural rewrite.

Phase 1B implication: API keys carry an issuer field. MVP issuer is `"tally-cli-local"`; Phase 2 issuer could be `"pronoic-subscription-svc"`. The Worker validates the issuer field; new issuer types extend the validation strategy without changing existing keys. If short-lived tokens land in Phase 2, KV (or another fast-lookup primitive) gets added then — not now.

### Billing infrastructure

Out of scope for Phase 1B per locked items. But the Phase 1B architecture should make billing easy to add later — specifically, the Worker should report usage metrics (DO invocations, wake dispatches, total bytes routed) without architectural changes.

Phase 1B implication: emit metrics via Cloudflare's Workers Analytics Engine or Workers Tail from day one, even if no one consumes them. Adding billing later means adding a consumer, not adding instrumentation.

### Multi-region deployment

Cloudflare's edge model gives Tally low-latency dispatch by default — DO instances are pinned to a specific colo on first instantiation. Phase 2 might want region-pinned tenants for compliance (EU AI Act Article 13, GDPR data residency).

Phase 1B implication: DO placement is an operational concern, not architectural. Phase 1B doesn't need to handle this; Phase 2 adds region-pinning controls when commercial demand surfaces.

### Audit trail as a separate commercial tier

Tally's AuditTrail implementation, when it lands (deferred from MVP), could become a commercial differentiator — Stoa-compliant audit logs as a service. Tiering options Phase 2 will need to decide:
- AuditTrail free for all Tally users
- AuditTrail in a paid tier only
- AuditTrail free up to N events / N MB / N retention days; paid above

Phase 1B implication: AuditTrail isn't in the MVP; when it lands (later Phase 1B sub-PR or Phase 2), the runtime architecture should support both "always on" and "tier-gated" without code changes (configuration toggle).

### MCP plugin distribution under BSL 1.1

`@skytalesh/tally-mcp` is an npm package. Distribution under BSL 1.1 on npm registry should work — npm doesn't restrict license types, and BSL 1.1 explicitly permits redistribution for non-commercial-service use. Phase 1B implication: include a clear LICENSE file and per-tool / per-file SPDX headers. Phase 2 may publish a "free for personal use, paid for commercial-service hosting" clarification on the package README.

### License compatibility for downstream products

The "Tally Repos" / "Tally Code" product-family framing is for a later strategic spec. The question Phase 2 needs to answer: does BSL 1.1 on Tally constrain what those products can be?

Likely answer: no, as long as those products are built ON TOP of Tally (consume its APIs) rather than redistributing Tally itself. But BSL 1.1's commercial-use prohibition needs explicit interpretation for derived products. Phase 2 commercial counsel question.

Phase 1B implication: none architectural.

### Audit-trail / wake-router shape implications for Stoa

Phase 1B is Stoa's first concrete consumer of the WakeRouter trait. Trait-surface refinements that surface during Tally implementation feed back to Stoa as follow-up Stoa PRs (`stoa` and `stoa-rs` repos). The provisional marking sub-PR 3 of #431 placed on WakeRouter is the contract that permits this — but it also means Tally's first sub-PR may carry "open question" status until the Stoa-side revision lands.

Recommended discipline: Tally Sub-PR 1 documents trait-surface gaps in its PR body; Stoa repo follow-up PRs land before Sub-PR 1's CI gate. Same dual-PR discipline as the Skytale + Stoa sub-PR sequencing.

### Per-agent persistent memory and conversation history

Long-running agents need persistent state beyond cryptographic identity + role pack + shared context: per-agent memory (what the agent knows from prior sessions), per-agent conversation history (what was said TO the agent, separate from shared context), tool-affordance binding to agent identity (which tools each agent has access to).

Architectural placement: Stoa trait, by parallel reasoning with AuditTrail. Putting persistence in the Tally runtime would mean a different Stoa-implementing runtime produces a different agent memory model, breaking cross-runtime portability that the platform play depends on. Design lives in Phase 2A spec; not implemented in Phase 1B.

### Orchestrator architectural model

Phase 2B's destination product is anticipated to surface an orchestrator that the human talks to primarily, with team-coordination delegated to team agents. The orchestrator is itself an agent in the team (with its own Skytale identity), not a router.

Architectural reasoning: making the orchestrator a router would either require it to be inside the MLS group (making it an agent with elevated access, which is the same thing) or outside it (making it unable to see plaintext, defeating routing). Stoa's premise is that team members are cryptographic peers; the orchestrator-as-agent model preserves that premise while delivering the UX. Design lives in Phase 2B spec; not implemented in Phase 1B.

### Destination product naming

The Phase 2B destination product (the agent operations console application) wants its own name describing what it does for users, not inherited from "Tally Code" framing in prior strategic conversations. Tally is the runtime brand; naming the destination product "Tally Code" would conflate layers the way "Skytale SDK" would have conflated the SDK with the protocol if Stoa hadn't been extracted. Decision is a Phase 2B Phase 0 deliverable; during Phase 1B execution, if a working name surfaces organically it gets captured in the Phase 1B execution log without committing.

### Team-level deliberation artifact pattern

The Phase 2B destination product anticipates multi-agent deliberation as a coordination pattern that current chat-style multi-agent systems handle badly (context pollution, ad-hoc summarization, decisions not durable as first-class artifacts). The architectural response is likely a structured deliberation artifact — bounded participant set, structured positions, consensus state machine, durable outcome. The outcome is consumable by non-participants (e.g., an orchestrator agent executing the decision) without requiring them to read the discussion history.

Architectural placement is an open question:
- **(a)** Stoa-level protocol primitive (parallel to TeamPrimitive, RolePackHandler, etc.) — high coupling, mandates the pattern for all Stoa runtimes
- **(b)** Higher-level standard built on Stoa primitives (shared context CRDT + WakeRouter + agent identities) — analogous to HTTP-on-TCP; cross-runtime standardizable without expanding Stoa's required surface
- **(c)** Tally-specific product feature with no cross-runtime standard — simplest but limits portability of deliberations across future Stoa runtimes

Initial inclination: (b). Specifically, a separate deliberation-protocol specification that runs on existing Stoa primitives. The specific name of that standard (e.g., "Stoa Deliberation Protocol" was the placeholder that surfaced during initial conversation) is itself deferred to the Phase 2A or 2B spec that addresses this. Decision on placement (a/b/c) also lives in Phase 2A or Phase 2B spec; not implemented in Phase 1B.

### crates.io publishing discipline

The four Option B extraction crates (skytale-base, stoa, stoa-rs, skytale-sdk) are not yet published to crates.io. Verification during Phase 1B Phase 0 surfaced two operational gaps and one strategic gap.

Operational gaps (tracked in issue #449):
- Three of four crates have `publish = false` and minimal metadata
- Inter-crate deps use path-only form; need `path + version` pattern for publish-ability
- Three new crates need READMEs

Strategic gap (tracked in issue #448):
- The "stoa" name is squatted on crates.io by an unrelated party. Three response paths (negotiate / rename / Pronoic-namespaced fallback) named in the tracking issue; decision deferred to Phase 1B Sub-PR 4 Phase 0 when publishing becomes time-pressured.

**Publishing discipline (proposed)**: publish on Phase boundaries, not on every change. Phase 1B Sub-PR 4 (documentation pass + dogfooding verification) includes a step to publish any changed crates. This means published crate versions lag master by the duration of an active phase, which is acceptable for pre-1.0 crates with a small consumer set.

**Strategic positioning rationale**: publishing the four crates as standalone packages captures the brand-stack benefit of Stoa being addressable as a consumable artifact without the cross-repo coordination cost of repo separation. Standard precedent: rust-lang/rust publishes std/core/alloc etc. as crates while keeping the monorepo. The option to extract stoa to its own repo later remains open per the triggers captured at the Stoa-repo-or-not decision point.

---

## Phase 1B execution log discipline

During Phase 1B's execution (Sub-PRs 1 through 4), specific signals get captured in the planning doc as they surface:

- **MCP plugin UX pain points** — what's painful about the copy-paste-between-Claude-Code-instances coordination pattern. Phase 2B's destination product addresses this; Phase 1B execution generates the data that informs the UX design.

- **Stoa trait-surface questions** — gaps surfaced during implementation that suggest Stoa trait revisions. Sub-PR 1's dedicated "Stoa trait-surface gap survey" Phase 0 step captures the up-front gaps; subsequent sub-PRs add to the running list as new gaps surface.

- **Persistence-need signals** — what the dogfooding pattern reveals about per-agent memory and conversation history needs. Raw material for Phase 2A's spec when Phase 1B closes.

- **Naming candidates** — if working names for the Phase 2B destination product surface organically during execution, captured here for Phase 2B Phase 0 consideration.

This section accumulates entries as Phase 1B sub-PRs land; at Phase 1B close, the accumulated log feeds Phase 2A and Phase 2B Phase 0 work.

---

## References

- **Option B precedent**: `docs/specs/option-b-extraction.md` on skytale master at `962c92a`. Sub-PR sequence, completion log, Phase 0 corrections summary patterns.
- **Skytale issue #431** (closed 2026-05-13): https://github.com/nicholasraimbault/skytale/issues/431. Tracking-issue template.
- **Stoa WakeRouter trait surface**: `stoa/src/wake_router.rs` on skytale master. Trait definition + the provisional marking that permits Tally-driven refinements.
- **Stoa-rs NoopWakeRouter**: `stoa-rs/src/wake_router.rs` on skytale master. The deferred-no-op reference implementation. Tally's runtime is the second WakeRouter implementation (first concrete one).
- **Sub-PR 3 PR (#437)**: the PR that defined the four trait surfaces.
- **Sub-PR 4 PR (#438)**: the PR that introduced stoa-rs as the reference implementation.
- **Deprecated Orchestration MCP server**: `sdk/examples/orchestration/README.md` on skytale master. Tally supersedes Orchestration; Orchestration's 6-tool MCP surface is the predecessor pattern that informed Tally MCP's bounded 4-tool MVP scope.

---

## Phase 0 status (this document)

**Investigation drafted**: 2026-05-13. **Phase 0 close-out approved**: 2026-05-13. The spec is now the canonical implementation reference for the Phase 1B sub-PR sequence.

**Locked items confirmed**: 8 of 8 from the prompt restated above without modification.

**Open architectural questions raised** (all routed to appropriate phases):
- WakeRouter trait revisions → surveyed as a dedicated "Stoa trait-surface gap survey" step in Sub-PR 1 Phase 0; Stoa-side PRs land in parallel with Tally Sub-PR 1's code work, not blocking it
- KV use for auth tokens → decided here: NOT for MVP. Long-lived API keys validated via the TallyTeamDO's stored key list. KV surfaces in Phase 2 if short-lived tokens land
- R2 for audit-trail blobs → decided here: not for MVP, add when AuditTrail lands
- Multi-tenancy in DO naming → decided here: include tenancy prefix from day one even if Phase 1B keeps it constant
- All Phase 2 commercial questions → surfaced in "Open commercial/operational questions" section above

**Next step**: strategic-layer review of this spec + the tracking issue draft. After approval, the spec gets committed to skytale (in `docs/specs/`) and the tracking issue gets filed. Implementation Sub-PR 1 then creates the tally repo and migrates the spec.

---

## Acceptance for spec close-out (Phase 0 → strategic-layer review)

For this spec to be considered Phase 0-complete and ready for strategic-layer review:
1. ✅ Document follows Option B's structural template (status header, strategic context, what's locked, architecture, sub-PR sequence, acceptance criteria, open questions, references)
2. ✅ 8 locked items from the prompt restated faithfully
3. ✅ Investigation areas 1-5 surfaced with concrete decisions where appropriate, surfaced-as-questions where Phase 2 work needs the surface area
4. ✅ Dogfooding pattern named as the load-bearing product-validation acceptance criterion
5. ✅ Trait-surface refinement feedback loop to Stoa explicitly named
6. ✅ Eventual home + current home declared in opening section
7. ✅ Tracking issue draft prepared as a sibling document
