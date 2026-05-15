//! `tally agents register|unregister|key issue|key revoke` — agent-level
//! commands.
//!
//! Per cli-sub-pr-phase-0.md D4 agent-level commands use the
//! `--identity <bytes>` arg as BOTH the URL path identity AND the
//! Bearer (Bearer = identity per MVP D5; satisfies PR #18's identity-
//! match enforcement at the runtime, so no 403 is possible from
//! mismatch). The operator must possess the agent's keypair bytes —
//! acceptable for the single-user dogfooding flow.
//!
//! `key issue` and `key revoke` are client-side derivations at MVP:
//! the API key IS the agent identity under uniform-true validation,
//! so no HTTP call is needed. Phase 2 introduces real key tracking;
//! the command surface stays stable across the transition.

use clap::Subcommand;
use serde::Deserialize;
use serde_json::json;

use crate::http;

#[derive(Subcommand, Debug)]
pub enum AgentsCommand {
    /// Register an agent's identity with the team's WakeRouter.
    Register {
        /// The team to register against.
        #[arg(long)]
        team: String,
        /// Url-safe-b64 of the agent's identity bytes.
        #[arg(long)]
        identity: String,
        /// The context_id to register for.
        #[arg(long, default_value = "default")]
        context: String,
    },
    /// Remove an agent's registration.
    Unregister {
        #[arg(long)]
        team: String,
        #[arg(long)]
        identity: String,
        /// The context_id to remove.
        #[arg(long)]
        context: String,
    },
    /// Issue or revoke API keys for an agent's MCP server.
    #[command(subcommand)]
    Key(KeyCommand),
}

#[derive(Subcommand, Debug)]
pub enum KeyCommand {
    /// Issue an API key for an agent.
    Issue {
        #[arg(long)]
        team: String,
        #[arg(long)]
        identity: String,
    },
    /// Revoke an issued API key (MVP: no-op against uniform-true
    /// validation; Phase 2 wires real key tracking).
    Revoke {
        #[arg(long)]
        team: String,
        #[arg(long)]
        identity: String,
    },
}

/// Response shape for `POST /v1/teams/{T}/agents/{id}/register`.
/// Mirrors tally-worker's PublicRegisterResponse.
#[derive(Debug, Deserialize)]
struct RegisterResponse {
    registered: bool,
    context_id: String,
}

pub async fn run(cmd: AgentsCommand) -> Result<(), String> {
    match cmd {
        AgentsCommand::Register {
            team,
            identity,
            context,
        } => register(&team, &identity, &context).await,
        AgentsCommand::Unregister {
            team,
            identity,
            context,
        } => unregister(&team, &identity, &context).await,
        AgentsCommand::Key(KeyCommand::Issue { team, identity }) => {
            key_issue(&team, &identity).await
        }
        AgentsCommand::Key(KeyCommand::Revoke { team, identity }) => {
            key_revoke(&team, &identity).await
        }
    }
}

async fn register(team: &str, identity_b64: &str, context: &str) -> Result<(), String> {
    let endpoint = http::runtime_endpoint().map_err(|e| e.to_string())?;
    let bearer = http::agent_bearer(identity_b64);
    let client = http::client().map_err(|e| format!("build http client: {}", e))?;
    let url = format!(
        "{}/v1/teams/{}/agents/{}/register",
        endpoint, team, identity_b64
    );
    let body = json!({ "context_id": context });
    let resp = client
        .post(&url)
        .bearer_auth(&bearer)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("POST {}: {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "agents register failed (status {}): {}",
            status.as_u16(),
            body
        ));
    }
    let body: RegisterResponse = resp
        .json()
        .await
        .map_err(|e| format!("decode register response: {}", e))?;
    if !body.registered {
        return Err("server returned registered=false unexpectedly".into());
    }
    println!("Agent registered.");
    println!("  team: {}", team);
    println!("  identity: {}", identity_b64);
    println!("  context: {}", body.context_id);
    Ok(())
}

async fn unregister(team: &str, identity_b64: &str, context: &str) -> Result<(), String> {
    let endpoint = http::runtime_endpoint().map_err(|e| e.to_string())?;
    let bearer = http::agent_bearer(identity_b64);
    let client = http::client().map_err(|e| format!("build http client: {}", e))?;
    let url = format!(
        "{}/v1/teams/{}/agents/{}/handlers/{}",
        endpoint, team, identity_b64, context
    );
    let resp = client
        .delete(&url)
        .bearer_auth(&bearer)
        .send()
        .await
        .map_err(|e| format!("DELETE {}: {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "agents unregister failed (status {}): {}",
            status.as_u16(),
            body
        ));
    }
    println!("Agent unregistered.");
    println!("  team: {}", team);
    println!("  identity: {}", identity_b64);
    println!("  context: {}", context);
    Ok(())
}

async fn key_issue(team: &str, identity_b64: &str) -> Result<(), String> {
    // Per D5: MVP API key = url_safe_b64(identity_bytes). The Bearer
    // value the MCP server passes IS this string. No HTTP call needed
    // at MVP — the runtime validates via uniform-true Bearer decode.
    println!("API key (use as Bearer for the deployed Tally runtime):");
    println!();
    println!("  {}", identity_b64);
    println!();
    println!(
        "Configure @skytalesh/tally-mcp with TALLY_API_KEY=<above> and TALLY_TEAM_ID={}",
        team
    );
    println!();
    println!(
        "Note: MVP key issuance is a client-side derivation under uniform-true \
         validation. Phase 2 will introduce real key tracking; the command surface \
         stays stable across the transition."
    );
    Ok(())
}

async fn key_revoke(team: &str, identity_b64: &str) -> Result<(), String> {
    // Per D5: MVP no-op against uniform-true validation. Print
    // acknowledgment so operators see the command worked and
    // understand the MVP semantic.
    println!("Revocation acknowledged.");
    println!("  team: {}", team);
    println!("  identity: {}", identity_b64);
    println!();
    println!(
        "Note: MVP revocation is a no-op against uniform-true validation. The agent's \
         identity is still a valid Bearer until Phase 2 introduces real key tracking. \
         To prevent the agent from acting, unregister its handlers via \
         `tally agents unregister`."
    );
    Ok(())
}
