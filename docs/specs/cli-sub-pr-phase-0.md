# Tally CLI sub-PR — test plan

**Date:** 2026-05-15
**Scope:** Phase 1B Sub-PR 3 — Tally CLI binary with 11 commands per `phase-1b-spec.md` §"Sub-product 3: Tally CLI"
**Status:** Pre-implementation. Scoping decisions locked through one deliberation session.
**Prerequisite:** Sub-PR 1 merged (Cloudflare runtime; PRs #17, #18, #19, #20, #22 on `main`)

## Summary

Phase 1B Sub-PR 3 ships the `tally` binary — a Rust CLI for operators (Cloudflare deployment management), team-administrators (team-level Tally state provisioning), and agents (registration + API key issuance). 11 commands total (10 per spec table + `agents key revoke` per spec auth-model text). Mechanical implementation against documented spec; pattern matches skytale CLI at `skytale/cli/`.

Different artifact shape than Sub-PRs 1/2's Phase 0 design notes: scoping decisions + command catalog + verification surface, not "Architectural decisions" or "System Properties" sections. The work shape is mechanical implementation against documented spec, not architectural deliberation.

## Scoping decisions (locked)

### D1: Standalone crate at `tally/cli/`

CLI lives in a standalone crate that opts out of the workspace (matches skytale CLI's `skytale/cli/` + tally's existing `integration-tests/` pattern). The `skytale-sdk` dependency pulls heavy transitive deps (mls-rs, sqlcipher) that would force workspace-wide resolver constraints if CLI joined the workspace. Standalone keeps the workspace clean for `tally-core` + `tally-worker`.

### D2: `tally deploy` subprocess-delegates to `wrangler deploy`

`tally deploy` shells out to `wrangler deploy` rather than implementing the Cloudflare Workers Deploy API in pure Rust. Operators already require `wrangler` for local dev (`wrangler dev`); making it a CLI runtime dependency is acceptable for MVP. Phase 2 can replace with pure-Rust API if non-Node.js operators surface friction.

### D3: Cloudflare credentials delegate to `wrangler`'s auth state

`tally init` verifies `wrangler whoami` returns a logged-in identity; surfaces a clear actionable error if `wrangler` is not installed or unauthenticated. No separate Cloudflare auth flow in `tally init`; operators authenticate `wrangler` once and `tally` reuses that state.

### D4: Operator identity at `~/.tally/identity`; Bearer-construction differs by command level

`tally init` generates an operator identity (ed25519 keypair, base64-encoded) and stores it at `~/.tally/identity`. Distinct from `~/.skytale/identity` so operators can use Tally without using Skytale CLI.

**Operator-level commands** (`teams init`, `teams status`, `teams delete`) use the operator identity from `~/.tally/identity` as the Bearer. These commands hit the 3 new teams-level routes (per "Runtime API surface gap" lock below); those routes do NOT have URL-path identity (the URL path is just `/v1/teams/{team_id}/...`), so there's no identity-match enforcement — any well-formed Bearer is accepted at MVP per D5's uniform-true validation.

**Agent-level commands** (`agents register`, `agents unregister`) use the **agent's identity from the `--identity` arg** as BOTH the URL path identity AND the Bearer (since MVP D5 makes Bearer = `url_safe_b64(identity_bytes)`). PR #18's identity-match enforcement (HTTP API surface Decision 3) requires URL-path-identity == Bearer-derived-identity, else 403. The operator identity in `~/.tally/identity` is **not used** for these commands.

**Implication for operator-on-behalf-of-agent flows**: the operator must possess the agent's identity bytes (the raw url-safe-b64 encoding) to register on its behalf — there is no MVP delegation mechanism. This is acceptable for the single-user dogfooding scenario where the operator IS the agent (same machine; same identity flowing into both `~/.tally/identity` and the `--identity` arg of registration commands). Multi-operator delegation (operator A registers agent B without holding B's keypair) requires runtime-side auth changes and is **Phase 2 territory** — explicitly out of MVP scope.

**Agent-level commands that don't hit HTTP at MVP** (`agents key issue`, `agents key revoke` per command catalog #9 and #10) don't have Bearer concerns at MVP — they're client-side derivations / no-op acknowledgments. Phase 2 will introduce HTTP calls for these against real key tracking; the Bearer semantics for those Phase 2 routes will be locked then.

### D5: MVP API key = `url_safe_b64(identity_bytes)`; Phase 2 swaps without changing command shape

Per `http-api-surface-sub-pr-phase-0.md` Decision 1 (uniform-true validation), the MVP Bearer token IS the identity. `tally agents key issue --identity <bytes>` returns the url-safe-base64 encoding of those bytes as the API key. `tally agents key revoke` is structurally present but semantically a no-op against uniform-true validation. Phase 2 introduces real key tracking; the command surface remains stable across the transition.

### D6: Local config dir at `~/.tally/`

Matches skytale CLI's `~/.skytale/` precedent. Layout:

```
~/.tally/
├── identity           # raw ed25519 keypair (32 bytes), base64-encoded
├── runtime-endpoint   # text file: deployed Tally HTTP base URL (e.g. https://tally.workers.dev)
└── cloudflare-account # text file: Cloudflare account ID (for tally destroy reference)
```

No SQLite, no encrypted store at MVP — these are operator-facing config artifacts, not encrypted channel state.

### D7: Command-level error handling via `Result<(), String>`

Matches skytale CLI's command-level error type. Commands return `Result<(), String>`; main dispatcher prints the error string and exits with code 1 on `Err`. Internal helpers can use `anyhow::Result` for ergonomic error propagation; the command boundary converts to `Result<(), String>`.

### D8: Human-readable default output; `--output json` deferred

Each command prints human-readable output by default (labels, status indicators, multi-line layout). Machine-readable `--output json` flag is real future-work for scripting use cases but does not land in this sub-PR. Deferred to a coverage-expansion sub-PR if operator demand surfaces.

### D9: `tally version` content

Multi-line output with explicit labels:

```
tally 0.1.0
stoa-rs: rev <git rev pin>
runtime: https://tally.workers.dev (configured)
```

If runtime endpoint is not configured (e.g., pre-`tally deploy`), the runtime line reads `runtime: (not configured — run 'tally init' or 'tally deploy')`.

### D10: 11 commands total (spec table + `agents key revoke`)

Spec table in `phase-1b-spec.md:185-198` enumerates 10 commands. Spec auth-model text (line 157: "revocation is `tally agents key revoke`") references an 11th command not present in the table. CLI ships 11 commands for spec consistency. Test plan also flags a parallel one-line update to `phase-1b-spec.md`'s command table to include `key revoke` — same pattern as PR #18's `TallyError` refinement (spec drift caught during implementation lands as part of the implementing PR with honest acknowledgment).

The CLI sub-PR's diff includes:
- The CLI implementation of all 11 commands
- A one-line addition to `phase-1b-spec.md`'s command table inserting `tally agents key revoke --team <id> --identity <bytes>` between `key issue` and `tally version`

### D11: DTOs duplicated locally in CLI; not migrated to tally-core

`tally-core` currently exposes shared storage types (`TeamMeta`, `WakeRecord`, `WakeState`) used by the runtime's DO storage. The HTTP route DTOs (request/response shapes for the 6 public routes) live inside `tally-worker/src/routes/`.

CLI defines its own request/response DTOs per command (matches skytale CLI's `commands/agents.rs` precedent). Reasoning:
- Migrating tally-worker route DTOs into tally-core is desirable cleanup but bundles route refactoring into a mechanical CLI sub-PR (scope expansion against the mechanical work shape)
- ~50 LOC of duplication across 6 routes is below the practical-pain threshold
- A separate post-Sub-PR-4 cleanup PR can migrate route DTOs to tally-core if duplication becomes painful in practice

CLI source files that define request/response DTOs include a comment pointing at the runtime's canonical definition (`// Mirrors tally-worker/src/routes/dispatch.rs::DispatchRequest`).

## Command catalog (11 commands)

### Operator commands (3)

#### 1. `tally init`

- **Purpose**: Configure operator-level Tally state on the local machine
- **Args**: None positional; `--force` flag to re-initialize over existing config
- **Behavior**:
  1. Check `~/.tally/` exists; if not, create
  2. Check `~/.tally/identity` exists; if so and `--force` not set, error out with `"already initialized; use --force to reconfigure"`
  3. Generate ed25519 keypair; write to `~/.tally/identity`
  4. Run `wrangler whoami` subprocess; if fails, print actionable error pointing at `wrangler login`
  5. Prompt operator for the Cloudflare account ID (or read from `wrangler whoami` output); write to `~/.tally/cloudflare-account`
  6. Print success message with the operator's identity (url-safe-b64)
- **Errors**:
  - `wrangler` not installed: `"wrangler is not installed; install via 'npm install -g wrangler'"`
  - `wrangler whoami` returns unauthenticated: `"wrangler is not authenticated; run 'wrangler login'"`
  - `~/.tally/identity` exists without `--force`: see above

#### 2. `tally deploy`

- **Purpose**: Deploy the Tally Worker code to Cloudflare
- **Args**: `--wrangler-toml <path>` (optional; defaults to `wrangler.toml` in CWD)
- **Behavior**:
  1. Verify `~/.tally/identity` exists (operator initialized); error out cleanly if not
  2. Verify CWD or `--wrangler-toml` contains a `wrangler.toml` with the Tally Worker config
  3. Subprocess-delegate to `wrangler deploy` with appropriate `--config` arg
  4. Parse `wrangler deploy` output for the deployed URL
  5. Write the deployed URL to `~/.tally/runtime-endpoint`
  6. Print success with the deployed URL and a hint that `tally version` will show it
- **Errors**:
  - Operator not initialized: `"run 'tally init' first"`
  - `wrangler.toml` not found: `"wrangler.toml not found at <path>; cd to the tally repo or pass --wrangler-toml"`
  - `wrangler deploy` failure: forward `wrangler`'s stderr + exit code

#### 3. `tally destroy`

- **Purpose**: Tear down a Tally deployment
- **Args**: `--force` flag (skip confirmation prompt)
- **Behavior**:
  1. Read `~/.tally/runtime-endpoint`; error out if not configured
  2. Confirm interactively (`Are you sure you want to destroy <endpoint>? [y/N]`) unless `--force`
  3. Subprocess-delegate to `wrangler delete` (with the script name parsed from the configured endpoint)
  4. Clear `~/.tally/runtime-endpoint`
  5. Print success
- **Errors**: endpoint not configured; user declined confirmation; wrangler delete failure

### Team-administrative commands (3)

#### 4. `tally teams init <team_id>`

- **Purpose**: Provision the `TallyTeamDO` for an existing Stoa team
- **Args**: `<team_id>` (positional; the Stoa team's url-safe-b64 ID)
- **Behavior**:
  1. Read operator identity from `~/.tally/identity`; construct Bearer auth header
  2. Read runtime endpoint from `~/.tally/runtime-endpoint`
  3. POST to `<endpoint>/v1/teams/{team_id}/init` (route may already exist or this sub-PR may add it; check during implementation — see "Runtime API surface gap survey" below)
  4. Verify the response indicates the DO is provisioned
  5. Print success with team_id
- **Errors**: endpoint not configured; auth rejected (401); team_id malformed (400); upstream Stoa team does not exist (this is an integration check; runtime may not validate it at MVP)

#### 5. `tally teams status <team_id>`

- **Purpose**: Inspect the team's current routing state
- **Args**: `<team_id>` (positional)
- **Behavior**:
  1. Same auth setup as `teams init`
  2. GET `<endpoint>/v1/teams/{team_id}/status` (route may need to be added — see runtime surface gap survey)
  3. Parse response and print:
     - Registered agents (count + identities)
     - Inbox depth per agent
     - Recent wakes (last N, with timestamps)
- **Errors**: standard auth + not-found errors

#### 6. `tally teams delete <team_id>`

- **Purpose**: Tear down a team's Tally state (preserves the upstream Stoa team)
- **Args**: `<team_id>` (positional); `--force` flag (skip confirmation)
- **Behavior**:
  1. Confirm interactively unless `--force`
  2. Same auth setup
  3. DELETE `<endpoint>/v1/teams/{team_id}` (route may need to be added)
  4. Print success
- **Errors**: standard

### Agent commands (4)

#### 7. `tally agents register --team <id> --identity <bytes>`

- **Purpose**: Register an agent's identity with the team's `WakeRouter`
- **Args**: `--team <id>` (required); `--identity <url-safe-b64 bytes>` (required); `--context <context_id>` (optional; defaults to a single shared context)
- **Behavior** (per D4 agent-level Bearer semantics):
  1. Decode `--identity` from url-safe-b64 → raw identity bytes
  2. Construct Bearer = the same url-safe-b64 string passed as `--identity` (Bearer = identity per MVP D5; URL-path identity == Bearer-derived identity satisfies PR #18's identity-match enforcement)
  3. POST `<endpoint>/v1/teams/{team_id}/agents/{identity}/register` with `{"context_id": "..."}` body, Bearer auth
  4. Print success with the registered (team, identity, context)
- **Errors**: 400 (malformed `--identity` or `--team`), 401 (well-formed Bearer rejected — should not occur at MVP per uniform-true validation), 422 (handler already registered for this context — surface as "context already in use; use unregister first")
- **No 403 identity-mismatch**: Bearer is constructed from the same `--identity` bytes used in the URL path; mismatch is structurally impossible from the CLI side.
- **Single-user dogfooding flow**: operator possesses the agent's keypair (same operator IS the agent). Multi-operator delegation is Phase 2 — see D4.

#### 8. `tally agents unregister --team <id> --identity <bytes>`

- **Purpose**: Clean shutdown — remove the agent's registration
- **Args**: `--team <id>` (required); `--identity <url-safe-b64 bytes>` (required); `--context <context_id>` (required for unregister — explicit which context to remove)
- **Behavior** (per D4 agent-level Bearer semantics):
  1. Construct Bearer = the url-safe-b64 string from `--identity` (same construction as `register`)
  2. DELETE `<endpoint>/v1/teams/{team_id}/agents/{identity}/handlers/{context_id}` per PR #18's route surface, Bearer auth
  3. Print success
- **Errors**: 400 (malformed args), 404 (handler not found — surface as "no registration to remove for that context"), 401 (uniform-true should accept any well-formed Bearer at MVP)
- **No 403 identity-mismatch**: same reasoning as command #7 — Bearer derived from same `--identity` bytes as URL path.

#### 9. `tally agents key issue --team <id> --identity <bytes>`

- **Purpose**: Issue an API key for the agent's MCP server to authenticate with the runtime
- **Args**: `--team <id>` (required); `--identity <bytes>` (required)
- **Behavior**:
  1. MVP per D5: print `url_safe_b64(identity_bytes)` as the API key — this IS the Bearer value the MCP server passes
  2. Print usage hint: `Configure @skytalesh/tally-mcp with TALLY_API_KEY=<key> and TALLY_TEAM_ID=<team_id>`
- **No HTTP call at MVP** (uniform-true validation; the "key" is purely client-side derivable)
- **Phase 2 transition**: this command will POST to a key-issuance route; output unchanged from the operator's perspective

#### 10. `tally agents key revoke --team <id> --identity <bytes>` *(spec-table drift fix)*

- **Purpose**: Revoke an issued API key
- **Args**: same as `key issue`
- **Behavior**:
  1. MVP per D5: print a no-op-acknowledged message: `Revocation is a no-op against uniform-true validation (MVP). Phase 2 will track issued keys and remove on revoke.`
  2. Exit 0 (structurally successful)
- **Phase 2 transition**: this command will DELETE against a key-tracking route

### Diagnostic command (1)

#### 11. `tally version`

- **Purpose**: Print version info per D9 format
- **Args**: None
- **Behavior**:
  1. Print the CLI binary version (from `Cargo.toml`)
  2. Print the stoa-rs git rev pin (read from CLI's `Cargo.lock` at build time via `env!`)
  3. Print the configured runtime endpoint from `~/.tally/runtime-endpoint`, or `(not configured)` if absent

## Runtime API surface gap — Path A locked

PR #18 shipped 6 public routes (verified against `tally-worker/src/lib.rs:119-131`):

- `GET /v1/health`
- `POST /v1/teams/{team_id}/agents/{identity}/register`
- `DELETE /v1/teams/{team_id}/agents/{identity}/handlers/{context_id}`
- `POST /v1/teams/{team_id}/wakes` (dispatch)
- `GET /v1/teams/{team_id}/agents/{identity}/inbox`
- `POST /v1/teams/{team_id}/wakes/{wake_id}/complete`

The CLI's `teams init`, `teams status`, `teams delete` commands need 3 routes that **do not currently exist** in tally-worker:

- `POST /v1/teams/{team_id}/init` — explicit DO provisioning
- `GET /v1/teams/{team_id}/status` — read team routing state
- `DELETE /v1/teams/{team_id}` — tear down the DO

### Path A locked: CLI sub-PR adds the 3 missing routes

Locking pre-implementation, not as mid-implementation discovery. Resolving "do these 3 routes exist" is exactly the question pre-implementation test plans answer.

**Reasoning:**
- `teams status`'s shape is operator-facing diagnostic — needs a clean route surface, not constructed from inbox-endpoint plumbing. Constructing from existing routes adds CLI-side state-assembly complexity that mirrors what should be a single runtime route
- `teams init`'s implicit-on-first-request behavior is brittle; explicit init surface is cleaner architecturally and gives operators a verifiable provisioning step (the `tally teams init <team_id>` command needs SOMETHING to succeed-against)
- `teams delete`'s correct semantic is a DO-level destroy operation (clears DO storage); `wrangler`-side DO namespace deletion has different semantics (wipes ALL DOs in the namespace, not one team)
- The ~150-200 LOC inflation is real but isolated to the runtime; doesn't compound elsewhere in the CLI

### Auth model for the 3 new routes (MVP)

Per D5's uniform-true validation: any well-formed Bearer accepted; no identity-match enforcement (the routes have no URL-path identity to match against). Operator identity from `~/.tally/identity` is what the CLI sends. Phase 2 introduces team-admin tracking (`teams init` records the calling identity as team admin; `teams status`/`teams delete` enforce admin match) — out of MVP scope; bundling would expand this sub-PR beyond mechanical.

### Route-level behavior (MVP)

- `POST /v1/teams/{team_id}/init`: writes `TeamMeta { initialized_at: <timestamp> }` to DO storage; returns 200 with team_id echoed. Idempotent (re-init is a no-op that updates the timestamp). The current `TeamMeta` type in `tally-core` is sufficient — no storage-field expansion needed.
- `GET /v1/teams/{team_id}/status`: reads from DO storage and returns `{ team_id, initialized_at, registered_agents: [...], total_inbox_depth: N, recent_wakes: [...] }`. The `registered_agents` field requires the DO to maintain a registered-agents index — currently the `agent:{identity}:handlers` storage scheme doesn't surface a "list all agents" query. **This is the genuine stop-and-surface trigger**: implementing `teams status` may surface a missing storage index on the DO. If so, surface to user before extending — adding a registered-agents index is a real DO-state schema change, not a route-plumbing addition.
- `DELETE /v1/teams/{team_id}`: clears all DO storage keys for the team. Idempotent. Returns 204.

### Stop-and-surface trigger (refined)

The Path A lock is firm at "add these 3 routes." The genuine stop-and-surface trigger is **DO-state schema change** that surfaces during implementation. Specifically:
- If `teams status`'s `registered_agents` response field requires adding a new storage index to the DO (likely, given the current `agent:{identity}:handlers` scheme doesn't support a list-all query)
- If `teams delete` requires schema changes to safely iterate/clear all keys for a team
- If `TeamMeta` needs new fields beyond `initialized_at` to support the status response shape

These are real architectural decisions (DO-state schema is load-bearing for the runtime's correctness model); surface before extending rather than absorbing silently.

## What's explicitly deferred (not silently uncovered)

- `--output json` flag for machine-readable output (per D8)
- Real API key tracking (per D5; uniform-true MVP)
- Pure-Rust Cloudflare API for `tally deploy` (per D2; subprocess-delegate MVP)
- DTOs migrating into tally-core (per D11; local duplication MVP)
- Shell-completion generation (skytale CLI has this; tally CLI defers to post-MVP)
- Color output, progress bars (post-MVP UX polish)
- Interactive prompts beyond the bare minimum (D2's `wrangler whoami` check is the only interactivity at MVP; richer interactive workflows are post-MVP)
- Multi-environment config (`--env staging`); single deployment per `~/.tally/` at MVP

## Implementation scope

**Files created:**

- `tally/cli/Cargo.toml` (new; standalone, opts out of workspace; deps per spec + clap/reqwest/serde/tokio/dirs/base64/chrono per skytale CLI pattern)
- `tally/cli/src/main.rs` (clap `Parser` + dispatch; ~80 lines)
- `tally/cli/src/lib.rs` (`pub mod commands;` + shared helpers; ~30 lines)
- `tally/cli/src/config.rs` (`~/.tally/` management; identity read/write; runtime-endpoint read/write; ~120 lines)
- `tally/cli/src/http.rs` (reqwest client wrapper; Bearer auth header construction; ~80 lines)
- `tally/cli/src/commands/mod.rs` (re-exports; ~20 lines)
- `tally/cli/src/commands/init.rs` (D1's `tally init`; ~100 lines)
- `tally/cli/src/commands/deploy.rs` (`tally deploy`; ~100 lines)
- `tally/cli/src/commands/destroy.rs` (`tally destroy`; ~80 lines)
- `tally/cli/src/commands/teams.rs` (`teams init/status/delete`; ~200 lines)
- `tally/cli/src/commands/agents.rs` (`agents register/unregister/key issue/key revoke`; ~200 lines)
- `tally/cli/src/commands/version.rs` (`tally version`; ~30 lines)
- `tally/cli/tests/cli_smoke.rs` (compile-test for clap parser + smoke test for `tally version`; ~50 lines)

**Files modified:**

- `tally/docs/specs/phase-1b-spec.md` — one-line table entry insertion for `agents key revoke` per D10
- `.github/workflows/ci.yml` — add `tally-cli` checks (compile + clippy + fmt) as a new job, paralleling `integration-tests-crate-checks` (does NOT add a runtime smoke job; the smoke test in `cli/tests/` exercises clap parsing without needing a deployed runtime)
- `tally/.gitignore` (if not already): add `tally/cli/target/` if standalone crate creates a separate target dir

**Runtime surface additions (Path A locked):**

- `tally-worker/src/lib.rs` — route registration update for `init` / `status` / `delete` (~20 lines); new handler functions for the 3 routes (~150 lines, may split into a new module if it bloats `lib.rs` past the 500-line file-organization guideline)
- `tally-worker/src/durable_object.rs` — DO-internal handlers for the 3 new sub-routes (`/init`, `/status`, `/delete`) following the existing `forward_to_do` pattern (~80 lines)
- Corresponding integration tests added to `integration-tests/tests/` covering happy paths + auth error cases for the 3 new routes (per `integration-tests-sub-pr-test-plan.md`'s pattern; ~100 lines)

**Dependencies (CLI crate only):**

```toml
[package]
name = "tally-cli"
version = "0.1.0"
edition = "2021"
publish = false

[workspace]  # opt out

[[bin]]
name = "tally"
path = "src/main.rs"

[dependencies]
# Spec-required
stoa-rs = { git = "https://github.com/pronoic/stoa", rev = "<pinned>" }  # team primitive types
skytale-sdk = { path = "../../skytale/sdk" }  # SDK orchestration
tally-core = { path = "../tally-core" }  # shared storage types

# CLI tooling (matches skytale CLI patterns)
clap = { version = "4", features = ["derive"] }
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
dirs = "6"
base64 = "0.22"
chrono = "0.4"

# Crypto for identity generation (matches skytale's identity model)
ed25519-dalek = { version = "2", features = ["serde", "rand_core"] }
rand = "0.10"
```

**Estimated diff:** ~1200-1500 lines (Path A bundled; above spec's ~800-1200 CLI-only estimate because the genuine runtime-surface gap is bundled). Breakdown:
- CLI crate: ~900-1100 LOC
- Runtime additions (Path A routes + DO handlers + integration tests): ~300-400 LOC

If the DO-state schema stop-and-surface trigger fires during implementation (e.g., `teams status`'s `registered_agents` requires a new storage index), the schema-change scope is separately surfaced and not absorbed silently.

## Verification

Existing 8 CI checks must continue to pass (per PR #22's CI surface):

1. `cargo fmt --check` (workspace)
2. `cargo clippy --workspace --exclude tally-worker --all-targets -- -D warnings`
3. `cargo clippy --package tally-worker --target wasm32-unknown-unknown --all-targets -- -D warnings`
4. `cargo test --workspace --exclude tally-worker`
5. `cargo check --package tally-worker --target wasm32-unknown-unknown --tests`
6. `cargo doc --workspace --exclude tally-worker --no-deps` + `cargo doc --package tally-worker --target wasm32-unknown-unknown --no-deps` (both with `RUSTDOCFLAGS=-D warnings`)
7. `cargo machete`
8. `cargo build --package tally-worker --target wasm32-unknown-unknown`
9. Integration-tests-crate checks (compile + clippy + fmt)
10. Integration-tests runtime (`cargo test --jobs 1 --manifest-path integration-tests/Cargo.toml --tests -- --test-threads=1`)

Plus new for `tally-cli` (paralleling `integration-tests-crate-checks`):

- **Tally CLI crate (compile + lint + test)**:
  - `cargo check --manifest-path cli/Cargo.toml --tests`
  - `cargo clippy --manifest-path cli/Cargo.toml --all-targets -- -D warnings`
  - `cargo fmt --check` (in `cli/`)
  - `cargo test --manifest-path cli/Cargo.toml --tests`
  - `cargo doc --manifest-path cli/Cargo.toml --no-deps` (with `RUSTDOCFLAGS=-D warnings`)

The CLI test runs the clap-parser smoke test + `tally version` invocation (no deployed runtime required); no integration-runtime CI surface for the CLI at this sub-PR (Sub-PR 4's dogfooding closes the integration loop).

`cargo machete` will scan the CLI crate; all listed deps must be genuinely used (matches the discipline from PR #20 where `humantime` was removed when unused).

## Methodology note

Test plan, not Phase 0 design notes. No "Architectural decisions" section, no "System Properties," no Layer-N verification framing. Mechanical implementation against documented spec.

The §9.1/§9.2/§9.3 lessons that DO carry forward:

- **Full CI surface verification** (not just spec'd commands) — see Verification section above with all 8 + 5 checks enumerated
- **Stop-and-surface for genuine deviations**:
  - DO-state schema changes required to support Path A's new routes (per "Runtime API surface gap — Path A locked" section); especially the `registered_agents` index for `teams status`
  - If `wrangler` subprocess integration surfaces friction beyond simple shell-out (e.g., output format changes in newer wrangler versions that break URL extraction in `tally deploy`)
  - If skytale-sdk dep brings transitive complications (e.g., wasm32 build of CLI for unexpected reasons; sqlcipher native-build failure on operator's machine)
  - If `teams status`'s response shape needs storage fields beyond what `TeamMeta` currently exposes
- **Audit-before-iterate** if a fix surfaces a new gap (e.g., if implementing `teams status` reveals a missing storage field in `TeamMeta`, audit the entire status-response shape for similar gaps before applying a one-off fix)
- **Mid-implementation corrections** to prior sub-PRs' decisions are folded into this PR's branch with honest acknowledgment in commit messages and the PR description (same pattern as PR #18's `TallyError` + integration-tests' `worker-rs` upgrade)

Pattern C's lock-architectural-commitments cadence doesn't apply here — applying it would be over-elaboration for what is fundamentally "implement 11 clap subcommands against a documented HTTP surface."
