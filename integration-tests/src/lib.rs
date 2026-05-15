//! End-to-end integration test fixture for tally's public HTTP API surface.
//!
//! Scope per `docs/specs/integration-tests-sub-pr-test-plan.md`: drives a
//! `wrangler dev` subprocess in the background, polls `/v1/health` until
//! ready, and exposes a [`TestHarness`] fixture that test files use to
//! exercise the six public routes per Phase 0 §3.3.
//!
//! # Why standalone
//!
//! - `tally-worker` is a wasm32-only `cdylib`; pulling reqwest/tokio
//!   into its dep tree would break the wasm build.
//! - Workspace-level `cargo test` already covers host-target unit
//!   tests; this crate's tests are end-to-end against a running
//!   `wrangler dev` and are run separately in a dedicated CI job.
//!
//! # Test pattern
//!
//! Each test constructs a multi-thread tokio runtime and uses
//! `runtime.block_on(TestHarness::setup())` for fixture setup, then
//! drives HTTP calls through `runtime.block_on(async { ... })` blocks.
//! `#[tokio::test]` is intentionally avoided to match the skytale
//! e2e crate's `tests/agent_teams_e2e/` pattern (manual runtime gives
//! callers control over teardown ordering relative to runtime drop).
//!
//! See test files in `tests/` for concrete usage:
//! - `happy_path.rs` (P0 — 6 scenarios)
//! - `error_codes.rs` (P1 — 7 scenarios)
//! - `multi_target.rs` (P2 — 3 scenarios)

use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD;
use base64::Engine as _;
use reqwest::Response;
use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};

/// Default port the harness asks `wrangler dev` to listen on when the
/// `TALLY_INTEGRATION_PORT` env var is unset. 8787 is wrangler's
/// out-of-the-box default; tests using a fixed port are simpler than
/// negotiating a freshly-allocated port through stdout parsing (which
/// wrangler's startup banner doesn't make ergonomic).
const DEFAULT_PORT: u16 = 8787;

/// Total wall-time budget for `wrangler dev` to come up and reply 200
/// from `/v1/health`. Wrangler cold-start (build + miniflare bootstrap)
/// runs ~5-10s on a fresh machine; 30s is generous headroom for the
/// CI cache-cold case.
const READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// Interval between readiness polls. 200ms balances responsiveness
/// against test setup overhead — at 5s typical readiness wall-time
/// that's ~25 probes, which is fine.
const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Composed lifecycle: a `wrangler dev` subprocess + a configured
/// `reqwest::Client`. Created once per test; Drop kills the subprocess
/// so each test runs against a fresh DO storage state.
///
/// # Lifecycle
///
/// 1. `TestHarness::setup()` spawns `wrangler dev --port <port> --local
///    --persist-to <tmpdir>`, then polls `/v1/health` until 200.
/// 2. Test calls the helper methods (`register`, `dispatch`, etc.) or
///    builds requests directly via `harness.client` against
///    `harness.base_url`.
/// 3. Drop sends SIGKILL to the subprocess. Tokio's `Child::start_kill`
///    is best-effort; the OS reaps the process. Storage is
///    `--persist-to`'d to a per-test tempdir which is cleaned up on
///    drop separately.
///
/// # Concurrency model
///
/// Tests run **sequentially** by default — `cargo test` per crate uses a
/// thread pool but multiple tests sharing port 8787 would collide. If
/// the suite needs parallel runs, switch to `cargo test --
/// --test-threads=1` (CI default), or evolve the harness to allocate a
/// free port per fixture. MVP is sequential.
pub struct TestHarness {
    /// Base URL of the running wrangler dev instance, e.g.
    /// `http://127.0.0.1:8787`.
    pub base_url: String,
    /// Pre-configured HTTP client. Has a 60s request timeout that
    /// accommodates long-poll dispatch (which blocks server-side for
    /// the duration of `timeout_seconds`).
    pub client: reqwest::Client,
    /// The wrangler subprocess. `tokio::process::Child` because we
    /// spawn from an async context; SIGKILL on Drop.
    wrangler_process: Child,
}

impl TestHarness {
    /// Bring up a fresh wrangler dev instance and return the harness.
    ///
    /// Looks for `wrangler` on `PATH`; if absent, returns an error
    /// telling the caller to install it (CI installs via
    /// `npm install -g wrangler`).
    ///
    /// `wrangler dev` is invoked with:
    /// - `--port <port>`: from `TALLY_INTEGRATION_PORT` env var or
    ///   the module's default (8787).
    /// - `--local`: forces miniflare local emulation; no Cloudflare
    ///   account / network calls.
    /// - `--persist-to <tmpdir>`: per-test ephemeral storage; on next
    ///   test setup the DO state is cold.
    ///
    /// # Errors
    /// - `wrangler` binary not found on PATH.
    /// - Subprocess spawn failure.
    /// - `/v1/health` doesn't return 200 within 30s.
    pub async fn setup() -> Result<Self> {
        let port: u16 = std::env::var("TALLY_INTEGRATION_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_PORT);
        let base_url = format!("http://127.0.0.1:{}", port);

        // Per-test ephemeral persistence dir. We don't tie its lifetime
        // to TestHarness because wrangler holds it open; we
        // intentionally leak it (under /tmp) for the test run and let
        // CI's runner cleanup handle it. Local dev: tests under /tmp
        // get cleaned at reboot.
        let persist_dir = std::env::temp_dir().join(format!(
            "tally-integration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&persist_dir)
            .with_context(|| format!("create persist dir {:?}", persist_dir))?;

        // Locate wrangler.toml at the repo root. CARGO_MANIFEST_DIR is
        // <repo>/integration-tests; walk up one to find wrangler.toml
        // alongside the workspace root.
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .ok_or_else(|| anyhow!("CARGO_MANIFEST_DIR has no parent"))?
            .to_path_buf();

        let wrangler_process = Command::new("wrangler")
            .arg("dev")
            .arg("--port")
            .arg(port.to_string())
            .arg("--local")
            .arg("--persist-to")
            .arg(&persist_dir)
            .current_dir(&repo_root)
            // Inherit stderr/stdout to surface wrangler build errors
            // during local development. CI captures via the workflow
            // step's log output.
            // DIAG: inherit (instead of pipe) so wrangler dev's stdout/stderr
            // (including worker `console_log!` output) flows to the test
            // process's stdout. This makes alarm_diag logs visible in CI's
            // captured test output. Temporary — pipe semantics will return
            // when diagnostic logging is removed.
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            // Best-effort cleanup if test panics before TestHarness drop runs.
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                "spawn `wrangler dev` — is wrangler installed? Try `npm install -g wrangler`."
                    .to_string()
            })?;

        let client = reqwest::Client::builder()
            // Long-poll dispatch blocks server-side for up to
            // `timeout_seconds`. Default to 60s so the harness's HTTP
            // timeout doesn't fire before the server's wake timeout.
            .timeout(Duration::from_secs(60))
            .build()
            .context("build reqwest client")?;

        let mut harness = Self {
            base_url,
            client,
            wrangler_process,
        };

        harness.wait_for_ready().await?;
        Ok(harness)
    }

    /// Poll `/v1/health` until 200 or timeout. Returns Err with
    /// captured stderr on timeout so test failures surface wrangler
    /// startup issues directly in the panic message.
    async fn wait_for_ready(&mut self) -> Result<()> {
        let url = format!("{}/v1/health", self.base_url);
        let deadline = Instant::now() + READINESS_TIMEOUT;
        let probe_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .context("build readiness probe client")?;

        while Instant::now() < deadline {
            if let Ok(resp) = probe_client.get(&url).send().await {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            tokio::time::sleep(READINESS_POLL_INTERVAL).await;
        }

        // Best-effort drain of wrangler stderr to enrich the error.
        let stderr = self.drain_stderr_best_effort().await;
        Err(anyhow!(
            "wrangler dev didn't become ready within {:?} at {}. \
             Captured stderr:\n{}",
            READINESS_TIMEOUT,
            url,
            stderr
        ))
    }

    /// Best-effort: read whatever wrangler has emitted on stderr so far,
    /// up to ~16KiB, with a short timeout. Used only for error
    /// reporting. Not for ongoing log capture.
    async fn drain_stderr_best_effort(&mut self) -> String {
        use tokio::io::AsyncReadExt as _;
        let Some(mut stderr) = self.wrangler_process.stderr.take() else {
            return "<no stderr captured>".to_string();
        };
        let mut buf = vec![0u8; 16 * 1024];
        let read = tokio::time::timeout(Duration::from_millis(500), stderr.read(&mut buf))
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or(0);
        if read == 0 {
            "<no stderr captured>".to_string()
        } else {
            String::from_utf8_lossy(&buf[..read]).into_owned()
        }
    }

    /// Generate a fresh test identity.
    ///
    /// Returns `(identity_b64, bearer_string)` where:
    /// - `identity_b64` is the URL-safe-base64 of 32 random bytes
    ///   (matches the MVP wire convention where identities are raw
    ///   bytes carried as url-safe-b64).
    /// - `bearer_string` is the same `identity_b64` — under the MVP
    ///   §9.2 Decision 1 scheme, the bearer *is* the identity_b64.
    ///   The DO's `/validate_api_key` handler decodes the bearer
    ///   directly via `stoa::types::Identity::from_url_safe_b64`.
    ///
    /// Identity bytes come from `Ulid::new().to_bytes()` repeated twice
    /// (16+16 = 32 bytes); this avoids adding `rand` as a dep while
    /// still giving distinct values across calls (ULID's
    /// monotonic-millisecond + random suffix changes per call).
    pub fn new_identity(&self) -> (String, String) {
        let a = ulid::Ulid::new().to_bytes();
        let b = ulid::Ulid::new().to_bytes();
        let mut bytes = Vec::with_capacity(32);
        bytes.extend_from_slice(&a);
        bytes.extend_from_slice(&b);
        let identity_b64 = BASE64_URL_SAFE_NO_PAD.encode(&bytes);
        let bearer = identity_b64.clone();
        (identity_b64, bearer)
    }

    /// Generate a fresh team_id as a 64-hex-char string.
    ///
    /// The Worker resolves `{team_id}` URL path segments via
    /// `namespace.id_from_string(team_id)`, which Cloudflare's runtime
    /// requires to be a 64-digit hexadecimal string. ULIDs (26 chars
    /// Crockford-base32) don't match this shape; we compose two ULIDs'
    /// raw 16-byte payloads (32 bytes total) and hex-encode them to
    /// produce a fresh, well-formed 64-hex team_id per call.
    ///
    /// Returns a fresh ULID string per call; Cloudflare's `id_from_name`
    /// (which `lookup_stub` calls) derives the DO instance via SHA-256
    /// hash of the name, so identical names map to identical DOs and
    /// distinct names map to distinct DOs. Fresh ULIDs guarantee test
    /// isolation between scenarios.
    pub fn new_team_id(&self) -> String {
        ulid::Ulid::new().to_string()
    }

    // ─── HTTP helpers ─────────────────────────────────────────────────
    //
    // Thin wrappers around the public routes. Each helper builds the
    // request body (when applicable), sets the Bearer header, and
    // returns the raw `reqwest::Response` so the caller can assert on
    // status + body shape as the scenario requires.

    /// `POST /v1/teams/{team_id}/agents/{identity}/register` — register
    /// the agent identity as a handler for `context_id`.
    ///
    /// `bearer` should generally equal `identity_b64` (MVP scheme).
    pub async fn register(
        &self,
        team_id: &str,
        identity_b64: &str,
        bearer: &str,
        context_id: &str,
    ) -> Result<Response> {
        let url = format!(
            "{}/v1/teams/{}/agents/{}/register",
            self.base_url, team_id, identity_b64
        );
        let body = serde_json::json!({ "context_id": context_id });
        self.client
            .post(&url)
            .bearer_auth(bearer)
            .json(&body)
            .send()
            .await
            .context("POST .../register")
    }

    /// `DELETE /v1/teams/{team_id}/agents/{identity}/handlers/{context_id}`.
    pub async fn unregister(
        &self,
        team_id: &str,
        identity_b64: &str,
        bearer: &str,
        context_id: &str,
    ) -> Result<Response> {
        let url = format!(
            "{}/v1/teams/{}/agents/{}/handlers/{}",
            self.base_url, team_id, identity_b64, context_id
        );
        self.client
            .delete(&url)
            .bearer_auth(bearer)
            .send()
            .await
            .context("DELETE .../handlers/{context_id}")
    }

    /// `POST /v1/teams/{team_id}/wakes` — dispatch a wake. Blocks
    /// server-side until the wake completes, times out, or returns
    /// 422/etc immediately.
    ///
    /// `caller_bearer` authenticates as the caller; the server reads
    /// `target_identity` from the JSON body.
    pub async fn dispatch(
        &self,
        team_id: &str,
        caller_bearer: &str,
        target_identity_b64: &str,
        context_id: &str,
        payload_b64: &str,
        timeout_seconds: u32,
    ) -> Result<Response> {
        let url = format!("{}/v1/teams/{}/wakes", self.base_url, team_id);
        let body = serde_json::json!({
            "target_identity": target_identity_b64,
            "context_id": context_id,
            "payload": payload_b64,
            "timeout_seconds": timeout_seconds,
        });
        self.client
            .post(&url)
            .bearer_auth(caller_bearer)
            .json(&body)
            .send()
            .await
            .context("POST .../wakes")
    }

    /// `POST /v1/teams/{team_id}/wakes/{wake_id}/complete` — complete
    /// a pending wake with a response payload.
    ///
    /// `responder_bearer` authenticates as the wake's target.
    pub async fn complete(
        &self,
        team_id: &str,
        wake_id: &str,
        responder_bearer: &str,
        response_b64: &str,
    ) -> Result<Response> {
        let url = format!(
            "{}/v1/teams/{}/wakes/{}/complete",
            self.base_url, team_id, wake_id
        );
        let body = serde_json::json!({ "response": response_b64 });
        self.client
            .post(&url)
            .bearer_auth(responder_bearer)
            .json(&body)
            .send()
            .await
            .context("POST .../wakes/{wake_id}/complete")
    }

    /// `GET /v1/teams/{team_id}/agents/{identity}/inbox` — read pending
    /// wakes for `identity`.
    ///
    /// `wait_seconds = 0` returns immediately; `wait_seconds > 0`
    /// long-polls until either a wake arrives or the timeout fires.
    /// `limit` caps the number of returned wakes; on a full inbox the
    /// response's `more_available` flag is true.
    pub async fn read_inbox(
        &self,
        team_id: &str,
        identity_b64: &str,
        bearer: &str,
        wait_seconds: u32,
        limit: Option<u32>,
    ) -> Result<Response> {
        let mut url = format!(
            "{}/v1/teams/{}/agents/{}/inbox?wait_seconds={}",
            self.base_url, team_id, identity_b64, wait_seconds
        );
        if let Some(l) = limit {
            url.push_str(&format!("&limit={}", l));
        }
        self.client
            .get(&url)
            .bearer_auth(bearer)
            .send()
            .await
            .context("GET .../inbox")
    }

    /// `GET /v1/health` — Worker-only healthcheck (no DO call).
    pub async fn health(&self) -> Result<Response> {
        let url = format!("{}/v1/health", self.base_url);
        self.client.get(&url).send().await.context("GET /v1/health")
    }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        // tokio::process::Child has `start_kill`; this returns
        // immediately (sends SIGKILL to the process group). We don't
        // need to wait — the OS reaps the process; for tests that's
        // fine. `kill_on_drop(true)` set during spawn ensures the
        // subprocess also dies if the runtime is dropped before
        // Drop fires.
        let _ = self.wrangler_process.start_kill();
    }
}

// ─── Wire-format helpers for response parsing in tests ────────────────
//
// These mirror the §3.3 public response shapes. Defined inline so the
// integration-tests crate doesn't import tally-worker (which is a
// wasm32-only cdylib; host-target build is unsupported). Field names
// match §3.3 verbatim.

/// Public response body for `GET /v1/health`.
#[derive(Debug, Deserialize, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

/// Public response body for `POST .../register`.
#[derive(Debug, Deserialize, Serialize)]
pub struct RegisterResponse {
    pub registered: bool,
    pub context_id: String,
}

/// Public success response body for `POST .../wakes`.
#[derive(Debug, Deserialize, Serialize)]
pub struct DispatchResponse {
    pub wake_id: String,
    pub response: String,
    pub completed_at: String,
}

/// Public success response body for `POST .../wakes/{wake_id}/complete`.
#[derive(Debug, Deserialize, Serialize)]
pub struct CompleteResponse {
    pub completed: bool,
    pub wake_id: String,
}

/// Public response body for `GET .../inbox`.
#[derive(Debug, Deserialize, Serialize)]
pub struct ReadInboxResponse {
    pub wakes: Vec<WakeSummary>,
    pub more_available: bool,
}

/// Per-wake summary in the public inbox response.
#[derive(Debug, Deserialize, Serialize)]
pub struct WakeSummary {
    pub wake_id: String,
    pub caller_identity: String,
    pub context_id: String,
    pub payload: String,
    pub expires_at: String,
}

/// Public structured-JSON error body per §3.3. Some variants carry
/// additional contextual fields (`wake_id`, `context_id`,
/// `timeout_seconds`); tests use `serde_json::Value` directly when
/// they need to assert on those.
#[derive(Debug, Deserialize, Serialize)]
pub struct ErrorBody {
    pub error: String,
}

/// Base64-encode a byte slice as URL-safe (no padding). Matches the
/// `payload` / `response` wire format per §3.3.
pub fn b64_encode(bytes: &[u8]) -> String {
    BASE64_URL_SAFE_NO_PAD.encode(bytes)
}
