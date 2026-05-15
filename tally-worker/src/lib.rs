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

use crate::rpc::{ValidateApiKeyRequest, ValidateApiKeyResponse};

/// `Durable Object` binding name configured in `wrangler.toml`.
///
/// The Worker reaches the TallyTeamDO namespace via
/// `env.durable_object(DO_BINDING)`. The string is a binding name (not
/// a class name) per Cloudflare's wrangler.toml convention.
const DO_BINDING: &str = "tally-team";

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
/// Per Phase 0 §4.3 the DO ID is the hex stringified form of the
/// `State::id()`. The Worker accepts the same hex form as the URL
/// `{team_id}` path segment; on malformed hex the Cloudflare runtime
/// raises an error which we surface as 400.
fn lookup_stub(env: &Env, team_id: &str) -> Result<worker::Stub> {
    let namespace = env.durable_object(DO_BINDING)?;
    namespace.id_from_string(team_id)?.get_stub()
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
async fn authenticate(
    req: &Request,
    env: &Env,
    team_id: &str,
    url_identity: Option<&str>,
) -> Result<std::result::Result<(worker::Stub, String), Response>> {
    let bearer = match parse_bearer(req)? {
        Some(b) => b,
        None => {
            return Ok(Err(Response::error(
                "missing or malformed Authorization header",
                401,
            )?))
        }
    };

    let stub = match lookup_stub(env, team_id) {
        Ok(s) => s,
        Err(_) => {
            return Ok(Err(Response::error(
                "invalid team_id (malformed Durable Object id)",
                400,
            )?))
        }
    };

    let validation = match validate_bearer(&stub, &bearer).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(Err(Response::error(
                format!("internal auth error: {}", e),
                500,
            )?))
        }
    };

    if !validation.valid {
        return Ok(Err(Response::error("invalid Bearer token", 401)?));
    }
    let identity_b64 = match validation.identity_b64 {
        Some(id) => id,
        None => {
            return Ok(Err(Response::error(
                "validation returned valid=true without identity_b64",
                500,
            )?))
        }
    };

    if let Some(url_id) = url_identity {
        if url_id != identity_b64 {
            return Ok(Err(Response::error(
                "URL identity does not match authenticated identity",
                403,
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

/// `POST /v1/teams/:team_id/agents/:identity/register` handler.
///
/// Per Phase 0 Decision 1 implementation contract: the URL `{identity}`
/// is authenticated against the Bearer; on match the Worker forwards
/// the authenticated identity to the DO as the `identity_b64` body
/// field. Public body's `context_id` flows through verbatim;
/// `metadata` (per phase-1b §3.3) is opaque and forwarded unchanged
/// (the DO currently ignores it but accepts it via serde-flatten
/// tolerance — the internal `RegisterRequest` defines only
/// `identity_b64` and `context_id`).
async fn handle_register(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return Response::error("missing team_id", 400),
    };
    let identity = match ctx.param("identity") {
        Some(i) => i.to_string(),
        None => return Response::error("missing identity", 400),
    };

    let public_body: serde_json::Value = match req.json().await {
        Ok(v) => v,
        Err(e) => return Response::error(format!("invalid request body: {}", e), 400),
    };
    let context_id = match public_body.get("context_id").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return Response::error("missing or non-string context_id", 400),
    };

    let (stub, identity_b64) = match authenticate(&req, &ctx.env, &team_id, Some(&identity)).await?
    {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    let body = serde_json::to_vec(&crate::rpc::RegisterRequest {
        identity_b64,
        context_id,
    })?;
    forward_to_do(&stub, Method::Post, "/register", Some(body)).await
}

/// `DELETE /v1/teams/:team_id/agents/:identity/handlers/:context_id` handler.
///
/// The public surface is `DELETE` with the `context_id` in the path; the
/// internal DO RPC remains `POST /unregister` taking
/// `UnregisterRequest`. The Worker constructs the internal body from
/// the URL path segments.
async fn handle_unregister(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return Response::error("missing team_id", 400),
    };
    let identity = match ctx.param("identity") {
        Some(i) => i.to_string(),
        None => return Response::error("missing identity", 400),
    };
    let context_id = match ctx.param("context_id") {
        Some(c) => c.to_string(),
        None => return Response::error("missing context_id", 400),
    };

    let (stub, identity_b64) = match authenticate(&req, &ctx.env, &team_id, Some(&identity)).await?
    {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    let body = serde_json::to_vec(&crate::rpc::UnregisterRequest {
        identity_b64,
        context_id,
    })?;
    forward_to_do(&stub, Method::Post, "/unregister", Some(body)).await
}

/// `POST /v1/teams/:team_id/wakes` handler — dispatch a wake.
///
/// Per Phase 0 Decision 1 + phase-1b §3.3: "Caller identity from the
/// validated API key; not a request parameter." The Worker injects the
/// authenticated identity as `caller_identity_b64` into the forwarded
/// body. Other body fields (`target_identity_b64`, `context_id`,
/// `payload_b64`, `timeout_ms`) flow through; the DO validates them.
async fn handle_dispatch(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return Response::error("missing team_id", 400),
    };

    let mut public_body: serde_json::Value = match req.json().await {
        Ok(v) => v,
        Err(e) => return Response::error(format!("invalid request body: {}", e), 400),
    };

    // Dispatch is the only authenticated route whose URL has no
    // `{identity}` segment; auth still happens, but no URL-identity
    // comparison.
    let (stub, identity_b64) = match authenticate(&req, &ctx.env, &team_id, None).await? {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    // Inject the authenticated identity. Override any client-supplied
    // value: the URL+Bearer is the source of truth for caller identity.
    if let Some(obj) = public_body.as_object_mut() {
        obj.insert(
            "caller_identity_b64".to_string(),
            serde_json::Value::String(identity_b64),
        );
    } else {
        return Response::error("dispatch body must be a JSON object", 400);
    }

    let body = serde_json::to_vec(&public_body)?;
    forward_to_do(&stub, Method::Post, "/dispatch", Some(body)).await
}

/// `GET /v1/teams/:team_id/agents/:identity/inbox` handler.
async fn handle_inbox(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return Response::error("missing team_id", 400),
    };
    let identity = match ctx.param("identity") {
        Some(i) => i.to_string(),
        None => return Response::error("missing identity", 400),
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
    forward_to_do(&stub, Method::Get, &do_path, None).await
}

/// `POST /v1/teams/:team_id/wakes/:wake_id/complete` handler.
///
/// Per Phase 0 Decision 1: the Worker injects the authenticated
/// identity as `by_identity_b64` (the wake's target completes its own
/// wake) and the URL `{wake_id}` segment into the forwarded body.
/// `response_payload_b64` flows through from the public body.
async fn handle_complete(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let team_id = match ctx.param("team_id") {
        Some(t) => t.to_string(),
        None => return Response::error("missing team_id", 400),
    };
    let wake_id = match ctx.param("wake_id") {
        Some(w) => w.to_string(),
        None => return Response::error("missing wake_id", 400),
    };

    let public_body: serde_json::Value = match req.json().await {
        Ok(v) => v,
        Err(e) => return Response::error(format!("invalid request body: {}", e), 400),
    };

    let (stub, identity_b64) = match authenticate(&req, &ctx.env, &team_id, None).await? {
        Ok(pair) => pair,
        Err(resp) => return Ok(resp),
    };

    // The internal CompleteRequest uses `response_payload_b64`. The
    // public body matches this internal contract (phase-1b §3.3's
    // shorter `response` name is a separate spec-alignment concern;
    // §9.1's RPC contract is what this PR's DO accepts).
    let response_payload_b64 = public_body
        .get("response_payload_b64")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let body = serde_json::to_vec(&crate::rpc::CompleteRequest {
        by_identity_b64: identity_b64,
        wake_id,
        response_payload_b64,
    })?;
    forward_to_do(&stub, Method::Post, "/complete", Some(body)).await
}
