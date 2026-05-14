//! Tally Cloudflare Worker entry point.
//!
//! Worker-side routing translates public HTTP requests (per Sub-PR 1 Phase 0
//! design notes §3.3) into Worker→DO RPC calls. The HTTP API surface
//! implementation that performs the translation is deferred to a subsequent
//! Workstream C PR per TallyTeamDO Phase 0 §9.2; this entry point currently
//! responds with a placeholder.

#![forbid(unsafe_code)]

use worker::*;

pub mod durable_object;
pub mod rpc;
pub mod wake_router;

#[event(fetch)]
async fn fetch(_req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    Response::ok(
        "Tally placeholder. HTTP API surface implementation in progress per Phase 1B Sub-PR 1.",
    )
}
