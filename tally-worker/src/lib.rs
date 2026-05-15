//! Tally Cloudflare Worker entry point.
//!
//! Worker-side routing translates public HTTP requests (per Sub-PR 1
//! Phase 0 design notes §3.3 + HTTP API surface sub-PR Phase 0 §3.3)
//! into Worker→DO RPC calls. For each authenticated request the Worker:
//!
//! 1. Parses `Authorization: Bearer <token>` from the request headers
//!    (missing/malformed → 401).
//! 2. Forwards the bearer to the TallyTeamDO via the internal
//!    `/validate_api_key` RPC; non-`valid` → 401.
//! 3. Compares the URL `{identity}` path segment (when present) against
//!    the validated identity; mismatch → 403.
//! 4. Forwards the public request to the appropriate internal DO
//!    endpoint and returns the DO's response unchanged.
//!
//! Six public routes per Phase 0 §3.3:
//! - `POST /v1/teams/{team_id}/agents/{identity}/register`
//! - `DELETE /v1/teams/{team_id}/agents/{identity}/handlers/{context_id}`
//! - `POST /v1/teams/{team_id}/wakes`
//! - `GET /v1/teams/{team_id}/agents/{identity}/inbox`
//! - `POST /v1/teams/{team_id}/wakes/{wake_id}/complete`
//! - `GET /v1/health` (Worker-only; no DO call)
//!
//! The `tally-team` Durable Object namespace binding is read from
//! `env.durable_object("tally-team")`; the team_id path segment maps to
//! the DO instance via `id_from_string` (hex form, per worker-rs 0.5's
//! `State::id().to_string()`).

#![forbid(unsafe_code)]

use serde::Serialize;
use worker::*;

pub mod dispatch_consts;
pub mod durable_object;
pub mod error;
pub mod rpc;
pub mod wake_router;
pub mod wake_types;

use crate::rpc::{
    CompleteRequest, DispatchRequest, DispatchResponse, InitTeamResponse, PublicCompleteRequest,
    PublicCompleteResponse, PublicDispatchRequest, PublicDispatchResponse, PublicInitTeamResponse,
    PublicReadInboxResponse, PublicRegisterRequest, PublicRegisterResponse, PublicRegisteredAgent,
    PublicTeamStatusResponse, PublicWakeSummary, ReadInboxResponse, RegisterRequest,
    TeamStatusResponse, UnregisterRequest, ValidateApiKeyRequest, ValidateApiKeyResponse,
};

/// Format a unix-millisecond timestamp as an ISO-8601 second-precision
/// UTC string (e.g. `"2026-05-13T20:00:00Z"`).
///
/// Per HTTP API surface sub-PR F.1 expansion, the public response
/// shapes for [`PublicDispatchResponse::completed_at`] and
/// [`PublicWakeSummary::expires_at`] use ISO-8601 strings while the
/// internal DO shapes use raw unix-millis. The Worker is the
/// translation site.
///
/// Implementation uses Howard Hinnant's `civil_from_days` algorithm
/// for the date-component conversion — pure arithmetic, no external
/// time/datetime crate needed. Second precision (drops sub-second
/// fraction) matches §3.3's example format. Always UTC ("Z" suffix).
fn format_iso8601_utc(millis: u64) -> String {
    let total_secs = millis / 1000;
    let day = (total_secs / 86_400) as i64;
    let seconds_of_day = (total_secs % 86_400) as u32;
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;

    // Howard Hinnant civil_from_days: converts days-since-unix-epoch to
    // (year, month, day). Algorithm: shift epoch to 0000-03-01, treat
    // year as starting from March (so leap-day Feb 29 lives at year-end).
    // Reference: https://howardhinnant.github.io/date_algorithms.html
    let z = day + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, d, hour, minute, second
    )
}

/// Build a structured-JSON error response for Worker-layer errors per
/// HTTP API surface sub-PR F.1 expansion.
///
/// Wire shape: `{ "error": "..." }`. Worker-layer errors (auth failure,
/// malformed team_id, etc.) don't carry contextual fields like
/// `wake_id` (those originate from the DO and are populated by the
/// DO's structured-JSON error mappers). The DO's response is forwarded
/// to the public caller unchanged for DO-originated errors.
fn json_error_response(status: u16, error: &str) -> Result<Response> {
    let body = serde_json::json!({ "error": error });
    Ok(Response::from_json(&body)?.with_status(status))
}

/// `Durable Object` binding name configured in `wrangler.toml`.
///
/// The Worker reaches the TallyTeamDO namespace via
/// `env.durable_object(DO_BINDING)`. The string is a binding name (not
/// a class name) per Cloudflare's wrangler.toml convention.
const DO_BINDING: &str = "TALLY_TEAM_DO";

/// Worker entry point — routes requests through the `Router` table and
/// returns the resulting `Response`.
///
/// Per HTTP API surface sub-PR Phase 0 §3.3 the six routes are wired
/// here. Each authenticated route shares a common bearer→identity →
/// DO-RPC pipeline (see `authenticate` + `forward_to_do` below).
#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    Router::new()
        .get_async("/v1/health", |_req, _ctx| async move { health_response() })
        .post_async(
            "/v1/teams/:team_id/agents/:identity/register",
            handle_register,
        )
        .delete_async(
            "/v1/teams/:team_id/agents/:identity/handlers/:context_id",
            handle_unregister,
        )
        .post_async("/v1/teams/:team_id/wakes", handle_dispatch)
        .get_async("/v1/teams/:team_id/agents/:identity/inbox", handle_inbox)
        .post_async(
            "/v1/teams/:team_id/wakes/:wake_id/complete",
            handle_complete,
        )
        // CLI sub-PR Path A: team-administrative routes for operator-
        // facing teams init/status/delete commands. Uniform-true Bearer
        // auth (per cli-sub-pr-phase-0.md D5); no URL-path identity to
        // match.
        .post_async("/v1/teams/:team_id/init", handle_team_init)
        .get_async("/v1/teams/:team_id/status", handle_team_status)
        .delete_async("/v1/teams/:team_id", handle_team_delete)
        .run(req, env)
        .await
}

/// Health endpoint per Phase 0 §3.3.
///
/// Returns 200 `{ "status": "ok", "version": <CARGO_PKG_VERSION> }`.
/// No auth, no DO call.
fn health_response() -> Result<Response> {
    #[derive(Serialize)]
    struct Health<'a> {
        status: &'a str,
        version: &'a str,
    }
    Response::from_json(&Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Extract the `Bearer <token>` from the Authorization header.
///
/// Returns `Ok(Some(token))` on a well-formed header, `Ok(None)` on
/// missing/malformed header (the caller maps both no-header and
/// malformed-prefix to 401 per Phase 0 §3.1).
fn parse_bearer(req: &Request) -> Result<Option<String>> {
    let header = match req.headers().get("Authorization")? {
        Some(h) => h,
        None => return Ok(None),
    };
    // RFC 6750: `Authorization: Bearer <token>`. Case-insensitive scheme;
    // exactly one space separator.
    let prefix = "Bearer ";
    if header.len() < prefix.len() {
        return Ok(None);
    }
    let (scheme, token) = header.split_at(prefix.len());
    if !scheme.eq_ignore_ascii_case(prefix) {
        return Ok(None);
    }
    Ok(Some(token.to_string()))
}

/// Look up the TallyTeamDO stub for a given `team_id` path parameter.
///
/// Per Phase 0 §3.2, `team_id` is a URL-safe identifier the caller
/// provides; the DO ID is derived server-side via Cloudflare's
/// `id_from_name` (internal SHA-256 hash of the name). This accepts
/// any UTF-8 string for `team_id`; identical names always map to the
/// same DO instance (the routing primitive callers rely on).
///
/// Earlier draft used `id_from_string`, which requires a 64-hex DO ID
/// from `State::id()`'s stringified form — incompatible with
/// caller-provided URL-safe team_ids. Corrected during §9.3
/// integration-test implementation (this PR).
fn lookup_stub(env: &Env, team_id: &str) -> Result<worker::Stub> {
    let namespace = env.durable_object(DO_BINDING)?;
    namespace.id_from_name(team_id)?.get_stub()
}

/// Internal Worker→DO RPC: `/validate_api_key`. See
/// [`ValidateApiKeyRequest`] for the wire contract.
async fn validate_bearer(stub: &worker::Stub, bearer: &str) -> Result<ValidateApiKeyResponse> {
    let body = serde_json::to_string(&ValidateApiKeyRequest {
        bearer: bearer.to_string(),
    })?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    init.with_body(Some(wasm_bindgen::JsValue::from_str(&body)));
    let do_url = "https://tally-internal/validate_api_key";
    let do_req = Request::new_with_init(do_url, &init)?;
    let mut do_resp = stub.fetch_with_request(do_req).await?;
    do_resp.json::<ValidateApiKeyResponse>().await
}

/// Shared bearer-auth pipeline per Phase 0 §1 implementation contract.
///
/// Returns either an early `Response` (401 / 403 / 5xx) or the
/// resolved DO stub + the authenticated identity_b64. Callers use the
/// stub + identity to construct the per-route internal DO request.
///
/// F.1 expansion: all early-return responses use structured JSON
/// bodies (`{ "error": "..." }`) per §3.3.
async fn authenticate(
    req: &Request,
    env: &Env,
    team_id: &str,
    url_identity: Option<&str>,
) -> Result<std::result::Result<(worker::Stub, String), Response>> {
    let bearer = match parse_bearer(req)? {
        Some(b) => b,
        None => {
            return Ok(Err(json_error_response(
                401,
                "missing or malformed Authorization header",
            )?));
        }
    };

    let stub = match lookup_stub(env, team_id) {
        Ok(s) => s,
        Err(_) => {
            return Ok(Err(json_error_response(
                400,
                "invalid team_id (malformed Durable Object id)",
            )?));
        }
    };

    let validation = match validate_bearer(&stub, &bearer).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(Err(json_error_response(
                500,
                &format!("internal auth error: {}", e),
            )?));
        }
    };

    if !validation.valid {
        return Ok(Err(json_error_response(401, "invalid Bearer token")?));
    }
    let identity_b64 = match validation.identity_b64 {
        Some(id) => id,
        None => {
            return Ok(Err(json_error_response(
                500,
                "validation returned valid=true without identity_b64",
            )?));
        }
    };

    if let Some(url_id) = url_identity {
        if url_id != identity_b64 {
            return Ok(Err(json_error_response(
                403,
                "URL identity does not match authenticated identity",
            )?));
        }
    }

    Ok(Ok((stub, identity_b64)))
}

/// Forward a request to an internal DO endpoint and return the DO's
/// response unchanged. Used by all auth'd routes after authentication
/// succeeds.
async fn forward_to_do(
    stub: &worker::Stub,
    method: Method,
    do_path: &str,
    body_bytes: Option<Vec<u8>>,
) -> Result<Response> {
    let mut init = RequestInit::new();
    init.with_method(method);
    if let Some(bytes) = body_bytes {
        // Construct a `Uint8Array` from the request body and pass it
        // as the JsValue body. `Uint8Array::from` copies a slice into
        // a fresh JS-side typed array.
        let arr = js_sys::Uint8Array::from(bytes.as_slice());
        init.with_body(Some(arr.into()));
    }
    let do_url = format!("https://tally-internal{}", do_path);
    let do_req = Request::new_with_init(&do_url, &init)?;
    stub.fetch_with_request(do_req).await
}

// ─── Helper: parse + translate DO success body → public response ──────
//
// Each handler below converges on the same pattern: forward to DO,
// inspect the DO's response status, and either translate the success
// body to the public shape or forward the structured-JSON error body
// unchanged (the DO's structured error responses are already in the
// `{ "error": "...", + contextual }` shape required by §3.3).

/// Forward to DO; if DO returns success (2xx), apply `translate` to the
/// parsed JSON body to produce the public response; if DO returns an
/// error (4xx/5xx), forward the raw response body + status unchanged
/// (the DO's `stoa_error_to_response` / `tally_error_to_response`
/// emits §3.3-compliant structured JSON).
async fn forward_and_translate_success<T, F>(
    stub: &worker::Stub,
    method: Method,
    do_path: &str,
    body_bytes: Option<Vec<u8>>,
    translate: F,
) -> Result<Response>
where
    T: serde::de::DeserializeOwned,
    F: FnOnce(T) -> Result<Response>,
{
    let mut do_resp = forward_to_do(stub, method, do_path, body_bytes).await?;
    let status = do_resp.status_code();
    if (200..300).contains(&status) {
        let internal: T = do_resp.json().await?;
        translate(internal)
    } else {
        // Pass through the DO's error response body + status unchanged.
        // worker-rs's `Response::from_body` paired with `with_status`
        // re-emits the body bytes the DO produced (which is already the
        // structured JSON `{ "error": "...", + contextual }` per §3.3).
        let body_bytes = do_resp.bytes().await?;
        Ok(Response::from_bytes(body_bytes)?.with_status(status))
    }
}

/// `POST /v1/teams/:team_id/agents/:identity/register` handler.
///
/// Public→internal translation (F.1 expansion):
/// - Deserialize body as [`PublicRegisterRequest`] (`{ context_id,
///   metadata? }`).
/// - Authenticate; inject authenticated identity (== URL identity
///   after the 403-mismatch check).
/// - Forward as internal [`RegisterRequest`] (`{ identity_b64,
///   context_id }`). `metadata` is dropped per [`PublicRegisterRequest`]
///   MVP scope boundary (Phase 2 wires storage).
/// - On DO 2xx success: emit public [`PublicRegisterResponse`] (201
///   `{ "registered": true, "context_id": "..." }`).
/// - On DO error: pass through the DO's structured-JSON error body.
async fn handle_register(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return json_error_response(400, "missing team_id"),
    };
    let identity = match ctx.param("identity") {
        Some(i) => i.to_string(),
        None => return json_error_response(400, "missing identity"),
    };

    let public: PublicRegisterRequest = match req.json().await {
        Ok(v) => v,
        Err(e) => return json_error_response(400, &format!("invalid request body: {}", e)),
    };

    let (stub, identity_b64) = match authenticate(&req, &ctx.env, &team_id, Some(&identity)).await?
    {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    let body = serde_json::to_vec(&RegisterRequest {
        identity_b64,
        context_id: public.context_id.clone(),
    })?;
    // Internal success body is `OkResponse` (empty object); the public
    // response shape comes from `public.context_id` and a hardcoded
    // `registered: true`.
    let ctx_id_for_translate = public.context_id;
    let mut do_resp = forward_to_do(&stub, Method::Post, "/register", Some(body)).await?;
    let status = do_resp.status_code();
    if (200..300).contains(&status) {
        // Construct public response. §3.3 specifies 201 for register
        // success.
        let public_resp = PublicRegisterResponse {
            registered: true,
            context_id: ctx_id_for_translate,
        };
        Ok(Response::from_json(&public_resp)?.with_status(201))
    } else {
        let body_bytes = do_resp.bytes().await?;
        Ok(Response::from_bytes(body_bytes)?.with_status(status))
    }
}

/// `DELETE /v1/teams/:team_id/agents/:identity/handlers/:context_id` handler.
///
/// Public→internal translation (F.1 expansion):
/// - URL provides `identity` (path) and `context_id` (path); no public
///   body.
/// - Authenticate; the authenticated identity must equal the URL
///   identity (403 on mismatch).
/// - Forward as internal [`UnregisterRequest`].
/// - On DO 2xx success: return 204 No Content (per §3.3).
/// - On DO error: pass through the DO's structured-JSON error body.
async fn handle_unregister(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return json_error_response(400, "missing team_id"),
    };
    let identity = match ctx.param("identity") {
        Some(i) => i.to_string(),
        None => return json_error_response(400, "missing identity"),
    };
    let context_id = match ctx.param("context_id") {
        Some(c) => c.to_string(),
        None => return json_error_response(400, "missing context_id"),
    };

    let (stub, identity_b64) = match authenticate(&req, &ctx.env, &team_id, Some(&identity)).await?
    {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    let body = serde_json::to_vec(&UnregisterRequest {
        identity_b64,
        context_id,
    })?;
    let mut do_resp = forward_to_do(&stub, Method::Post, "/unregister", Some(body)).await?;
    let status = do_resp.status_code();
    if (200..300).contains(&status) {
        // §3.3: unregister returns 204 No Content (no body).
        Ok(Response::empty()?.with_status(204))
    } else {
        let body_bytes = do_resp.bytes().await?;
        Ok(Response::from_bytes(body_bytes)?.with_status(status))
    }
}

/// `POST /v1/teams/:team_id/wakes` handler — dispatch a wake.
///
/// Public→internal translation (F.1 expansion):
/// - Deserialize body as [`PublicDispatchRequest`] (`{ target_identity,
///   context_id, payload, timeout_seconds }`).
/// - Authenticate; no URL-identity check (caller identity comes from
///   bearer, not URL).
/// - Translate to internal [`DispatchRequest`]:
///   - `target_identity` → `target_identity_b64`
///   - `payload` → `payload_b64`
///   - `timeout_seconds * 1000` → `timeout_ms`
///   - `caller_identity_b64` injected from authenticated identity
///   - `context_id` unchanged
/// - Forward via DO RPC. The DO returns internal [`DispatchResponse`]
///   with `wake_id` + `responding_identity_b64` + `response_payload_b64`
///   + `completed_at` (the F.1 refinement).
/// - On DO 2xx: translate internal [`DispatchResponse`] → public
///   [`PublicDispatchResponse`]:
///   - `wake_id` unchanged
///   - `response_payload_b64` → `response`
///   - `completed_at: u64` → `completed_at: String` (ISO-8601)
///   - `responding_identity_b64` is internal-only; dropped
/// - On DO error: pass through.
async fn handle_dispatch(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return json_error_response(400, "missing team_id"),
    };

    let public: PublicDispatchRequest = match req.json().await {
        Ok(v) => v,
        Err(e) => return json_error_response(400, &format!("invalid request body: {}", e)),
    };

    let (stub, identity_b64) = match authenticate(&req, &ctx.env, &team_id, None).await? {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    // Translate public → internal. The DO validates each field's
    // semantic content; the Worker only translates shapes.
    let timeout_ms = public.timeout_seconds.saturating_mul(1000);
    let internal = DispatchRequest {
        caller_identity_b64: identity_b64,
        target_identity_b64: public.target_identity,
        context_id: public.context_id,
        payload_b64: public.payload,
        timeout_ms,
    };

    let body = serde_json::to_vec(&internal)?;
    forward_and_translate_success::<DispatchResponse, _>(
        &stub,
        Method::Post,
        "/dispatch",
        Some(body),
        |internal_resp| {
            let public_resp = PublicDispatchResponse {
                wake_id: internal_resp.wake_id.to_string(),
                response: internal_resp.response_payload_b64,
                completed_at: format_iso8601_utc(internal_resp.completed_at),
            };
            Response::from_json(&public_resp)
        },
    )
    .await
}

/// `GET /v1/teams/:team_id/agents/:identity/inbox` handler.
///
/// Public→internal translation (F.1 expansion):
/// - URL provides `identity` (path); query string carries
///   `wait_seconds` and `limit` per §3.3.
/// - Authenticate; URL identity must equal authenticated identity
///   (403 mismatch).
/// - Forward to DO `/inbox/{identity_b64}` preserving the query
///   string.
/// - On DO 2xx: translate internal [`ReadInboxResponse`] →
///   [`PublicReadInboxResponse`]:
///   - per-entry [`WakeSummary`] → [`PublicWakeSummary`]
///     (`caller_identity_b64` → `caller_identity`, `payload_b64` →
///     `payload`, `expires_at_ms: u64` → `expires_at: String`
///     ISO-8601)
///   - `more_available` flows through unchanged
/// - On DO error: pass through.
async fn handle_inbox(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return json_error_response(400, "missing team_id"),
    };
    let identity = match ctx.param("identity") {
        Some(i) => i.to_string(),
        None => return json_error_response(400, "missing identity"),
    };

    let (stub, identity_b64) = match authenticate(&req, &ctx.env, &team_id, Some(&identity)).await?
    {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    // Preserve the query string (wait_seconds, limit) when forwarding
    // to the DO. Decision 3: identity is in the path, not the query.
    let query_str = req.url()?.query().unwrap_or("").to_string();
    let do_path = if query_str.is_empty() {
        format!("/inbox/{}", identity_b64)
    } else {
        format!("/inbox/{}?{}", identity_b64, query_str)
    };

    forward_and_translate_success::<ReadInboxResponse, _>(
        &stub,
        Method::Get,
        &do_path,
        None,
        |internal_resp| {
            let wakes = internal_resp
                .wakes
                .into_iter()
                .map(|w| PublicWakeSummary {
                    wake_id: w.wake_id,
                    caller_identity: w.caller_identity_b64,
                    context_id: w.context_id,
                    payload: w.payload_b64,
                    expires_at: format_iso8601_utc(w.expires_at_ms),
                })
                .collect();
            let public_resp = PublicReadInboxResponse {
                wakes,
                more_available: internal_resp.more_available,
            };
            Response::from_json(&public_resp)
        },
    )
    .await
}

/// `POST /v1/teams/:team_id/wakes/:wake_id/complete` handler.
///
/// Public→internal translation (F.1 expansion):
/// - URL provides `wake_id` (path); body is [`PublicCompleteRequest`]
///   (`{ response }`).
/// - Authenticate; no URL-identity check (the caller-identity check
///   happens DO-side via `by_identity` vs `wake.target_identity`).
/// - Translate to internal [`CompleteRequest`]:
///   - `response` → `response_payload_b64`
///   - `wake_id` injected from URL path
///   - `by_identity_b64` injected from authenticated identity
/// - Forward via DO RPC.
/// - On DO 2xx: emit public [`PublicCompleteResponse`] (200
///   `{ "completed": true, "wake_id": "..." }`).
/// - On DO error: pass through.
async fn handle_complete(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return json_error_response(400, "missing team_id"),
    };
    let wake_id = match ctx.param("wake_id") {
        Some(w) => w.to_string(),
        None => return json_error_response(400, "missing wake_id"),
    };

    let public: PublicCompleteRequest = match req.json().await {
        Ok(v) => v,
        Err(e) => return json_error_response(400, &format!("invalid request body: {}", e)),
    };

    let (stub, identity_b64) = match authenticate(&req, &ctx.env, &team_id, None).await? {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    let wake_id_for_response = wake_id.clone();
    let internal = CompleteRequest {
        by_identity_b64: identity_b64,
        wake_id,
        response_payload_b64: public.response,
    };

    let body = serde_json::to_vec(&internal)?;
    let mut do_resp = forward_to_do(&stub, Method::Post, "/complete", Some(body)).await?;
    let status = do_resp.status_code();
    if (200..300).contains(&status) {
        // DO's success body carries `{ "completed_at": <ms> }` (F.1's
        // pass-through of complete_wake's returned timestamp).
        // §3.3's public success shape doesn't include completed_at,
        // so we drop it here and emit just { completed, wake_id }.
        // Reading the DO body is required to discard it cleanly.
        let _ = do_resp.bytes().await;
        let public_resp = PublicCompleteResponse {
            completed: true,
            wake_id: wake_id_for_response,
        };
        Response::from_json(&public_resp)
    } else {
        let body_bytes = do_resp.bytes().await?;
        Ok(Response::from_bytes(body_bytes)?.with_status(status))
    }
}

// ─── CLI sub-PR Path A: team-administrative route handlers ────────────────
//
// Three public routes (init/status/delete) call into the DO's
// `/team/*` sub-routes. Auth: uniform-true Bearer; no URL-path
// identity, so `authenticate(..., None)` accepts any well-formed
// Bearer per cli-sub-pr-phase-0.md D5.

/// `POST /v1/teams/:team_id/init` handler.
///
/// Idempotent provisioning of the TallyTeamDO. `ensure_team_meta_initialized`
/// runs on every DO `fetch` so first-touch is implicit; this route is
/// an explicit acknowledgment of the lifecycle event plus a return of
/// the team's metadata for the CLI to display. Public response shape
/// per cli-sub-pr-phase-0.md "Runtime API surface gap — Path A locked"
/// section.
async fn handle_team_init(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return json_error_response(400, "missing team_id"),
    };

    let (stub, _identity_b64) = match authenticate(&req, &ctx.env, &team_id, None).await? {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    forward_and_translate_success::<InitTeamResponse, _>(
        &stub,
        Method::Post,
        "/team/init",
        None,
        |internal_resp| {
            let public_resp = PublicInitTeamResponse {
                team_id: internal_resp.team_id_b64,
                initialized_at: format_iso8601_utc(internal_resp.initialized_at_ms),
                tenancy_prefix: internal_resp.tenancy_prefix,
            };
            Response::from_json(&public_resp)
        },
    )
    .await
}

/// `GET /v1/teams/:team_id/status` handler.
///
/// Returns the team's TeamMeta + registered-agents summary + total
/// inbox depth. The DO derives `registered_agents` by listing
/// `agent:`-prefixed storage keys (no separate registered-agents index
/// in MVP storage schema).
async fn handle_team_status(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return json_error_response(400, "missing team_id"),
    };

    let (stub, _identity_b64) = match authenticate(&req, &ctx.env, &team_id, None).await? {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    forward_and_translate_success::<TeamStatusResponse, _>(
        &stub,
        Method::Get,
        "/team/status",
        None,
        |internal_resp| {
            let registered_agents = internal_resp
                .registered_agents
                .into_iter()
                .map(|a| PublicRegisteredAgent {
                    identity: a.identity_b64,
                    contexts: a.contexts,
                    inbox_depth: a.inbox_depth,
                })
                .collect();
            let public_resp = PublicTeamStatusResponse {
                team_id: internal_resp.team_id_b64,
                initialized_at: format_iso8601_utc(internal_resp.initialized_at_ms),
                tenancy_prefix: internal_resp.tenancy_prefix,
                registered_agents,
                total_inbox_depth: internal_resp.total_inbox_depth,
            };
            Response::from_json(&public_resp)
        },
    )
    .await
}

/// `DELETE /v1/teams/:team_id` handler.
///
/// Clears all DO storage + scheduled alarm. Returns 204 No Content
/// on success. Idempotent — repeated deletes on an already-empty DO
/// trigger `ensure_team_meta_initialized` (writes fresh metadata) then
/// `delete_all` again (wipes it).
async fn handle_team_delete(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return json_error_response(400, "missing team_id"),
    };

    let (stub, _identity_b64) = match authenticate(&req, &ctx.env, &team_id, None).await? {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    let mut do_resp = forward_to_do(&stub, Method::Post, "/team/delete", None).await?;
    let status = do_resp.status_code();
    if (200..300).contains(&status) {
        // DO returned 204 (empty body); pass through.
        Ok(Response::empty()?.with_status(204))
    } else {
        let body_bytes = do_resp.bytes().await?;
        Ok(Response::from_bytes(body_bytes)?.with_status(status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── format_iso8601_utc ────────────────────────────────────────────

    #[test]
    fn iso8601_format_unix_epoch() {
        // Unix epoch: 1970-01-01T00:00:00Z.
        assert_eq!(format_iso8601_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_format_phase_0_example() {
        // §3.3 example value: "2026-05-13T20:00:00Z". Corresponding
        // unix-millisecond timestamp is 1_778_702_400_000 (derived by
        // walking 20_587 full days from 1970-01-01 plus 20h offset).
        assert_eq!(
            format_iso8601_utc(1_778_702_400_000),
            "2026-05-13T20:00:00Z"
        );
    }

    #[test]
    fn iso8601_format_known_y2k() {
        // Y2K: 2000-01-01T00:00:00Z = 946_684_800 unix seconds.
        assert_eq!(format_iso8601_utc(946_684_800_000), "2000-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_format_leap_year_feb_29() {
        // 2024-02-29T12:34:56Z is a valid date (2024 is a leap year).
        // Unix seconds: 1_709_210_096.
        assert_eq!(
            format_iso8601_utc(1_709_210_096_000),
            "2024-02-29T12:34:56Z"
        );
    }

    #[test]
    fn iso8601_format_drops_subsecond_fraction() {
        // 1_700_000_000_999 ms → 1_700_000_000 secs (drops .999).
        let with_frac = format_iso8601_utc(1_700_000_000_999);
        let without_frac = format_iso8601_utc(1_700_000_000_000);
        assert_eq!(with_frac, without_frac);
    }

    #[test]
    fn iso8601_format_end_of_day_boundary() {
        // 2025-12-31T23:59:59Z = 1_767_225_599 unix seconds.
        assert_eq!(
            format_iso8601_utc(1_767_225_599_000),
            "2025-12-31T23:59:59Z"
        );
    }

    // ─── parse_bearer ──────────────────────────────────────────────────
    // Note: parse_bearer takes &Request which requires a wasm runtime
    // to construct; coverage deferred to §9.3 integration tests. The
    // format_iso8601_utc tests above are pure-arithmetic so they run
    // under `cargo test` without a wasm target.
}
