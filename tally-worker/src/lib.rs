//! Tally Cloudflare Worker entry point.
//!
//! The full router and DO bindings land in subsequent Workstream C PRs
//! per the Sub-PR 1 Phase 0 design notes (`docs/specs/phase-1b-sub-pr-1-phase-0.md`).
//! This first commit establishes the crate shape only.

#![forbid(unsafe_code)]

use worker::*;

#[event(fetch)]
async fn fetch(_req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    Response::ok("Tally placeholder. Implementation in progress per Phase 1B Sub-PR 1.")
}
