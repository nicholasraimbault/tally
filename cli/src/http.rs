//! HTTP client wrapper for talking to the deployed Tally runtime.
//!
//! Per cli-sub-pr-phase-0.md D4 there are two Bearer-construction
//! pathways:
//!
//! - **Operator-level** ([`operator_bearer`]): read from
//!   `~/.tally/identity`. Used by `teams init/status/delete`. Team-
//!   level routes have no URL-path identity, so authentication uses
//!   uniform-true Bearer per D5 — any well-formed Bearer is accepted
//!   at MVP.
//! - **Agent-level** ([`agent_bearer`]): construct directly from the
//!   `--identity <bytes>` CLI arg. Used by `agents register` +
//!   `agents unregister`. PR #18's URL-path-identity-must-equal-
//!   Bearer-derived-identity enforcement requires that the same
//!   url-safe-b64 string flows into BOTH the URL path AND the Bearer.
//!
//! `agents key issue` and `agents key revoke` are client-side
//! derivations at MVP and don't call HTTP.

use anyhow::{anyhow, Context, Result};
use reqwest::Client;

use crate::config;

/// Default timeout for CLI HTTP calls. Long enough to accommodate
/// `wrangler dev` cold starts; short enough that a hung runtime
/// surfaces quickly.
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Build a fresh `reqwest::Client` with the default timeout.
pub fn client() -> Result<Client> {
    Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .context("build reqwest client")
}

/// Resolve the deployed Tally runtime endpoint from config.
///
/// Returns an actionable error if no endpoint has been configured
/// (operator hasn't run `tally deploy`).
pub fn runtime_endpoint() -> Result<String> {
    config::read_runtime_endpoint()?.ok_or_else(|| {
        anyhow!(
            "no Tally runtime endpoint configured; run 'tally deploy' \
             to deploy the Worker and persist the endpoint"
        )
    })
}

/// Read the operator-level Bearer from `~/.tally/identity`. Used by
/// operator-level commands per D4.
pub fn operator_bearer() -> Result<String> {
    config::read_identity()
        .context("could not read operator identity; run 'tally init' to initialize")
}

/// Construct an agent-level Bearer from the `--identity` CLI arg.
/// Per MVP D5 `Bearer == identity_b64`; this is a thin alias for
/// clarity at the call site (signals "use this as both URL path
/// identity AND Bearer").
pub fn agent_bearer(identity_b64: &str) -> String {
    identity_b64.to_string()
}
