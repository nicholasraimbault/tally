//! `tally teams init|status|delete` — operator-facing team-state
//! commands.
//!
//! Per cli-sub-pr-phase-0.md D4 operator-level commands use the
//! operator identity from `~/.tally/identity` as the Bearer; the
//! team-level routes have no URL-path identity so authentication is
//! uniform-true Bearer (any well-formed Bearer accepted at MVP).

use std::io::{self, BufRead, Write};

use clap::Subcommand;
use serde::Deserialize;

use crate::http;

#[derive(Subcommand, Debug)]
pub enum TeamsCommand {
    /// Provision the TallyTeamDO for an existing Stoa team.
    Init {
        /// The Stoa team's url-safe identifier.
        team_id: String,
    },
    /// Inspect the team's routing state (registered agents, inbox depths).
    Status {
        /// The team to inspect.
        team_id: String,
    },
    /// Tear down a team's Tally state (preserves the upstream Stoa team).
    Delete {
        /// The team to tear down.
        team_id: String,
        /// Skip the interactive confirmation prompt.
        #[arg(long)]
        force: bool,
    },
}

/// Response shape for `POST /v1/teams/{team_id}/init`. Mirrors
/// `tally-worker/src/rpc.rs::PublicInitTeamResponse`.
#[derive(Debug, Deserialize)]
struct InitTeamResponse {
    team_id: String,
    initialized_at: String,
    tenancy_prefix: String,
}

/// Response shape for `GET /v1/teams/{team_id}/status`. Mirrors
/// `tally-worker/src/rpc.rs::PublicTeamStatusResponse`.
#[derive(Debug, Deserialize)]
struct TeamStatusResponse {
    team_id: String,
    initialized_at: String,
    tenancy_prefix: String,
    registered_agents: Vec<RegisteredAgent>,
    total_inbox_depth: u64,
}

#[derive(Debug, Deserialize)]
struct RegisteredAgent {
    identity: String,
    contexts: Vec<String>,
    inbox_depth: u64,
}

pub async fn run(cmd: TeamsCommand) -> Result<(), String> {
    match cmd {
        TeamsCommand::Init { team_id } => init(&team_id).await,
        TeamsCommand::Status { team_id } => status(&team_id).await,
        TeamsCommand::Delete { team_id, force } => delete(&team_id, force).await,
    }
}

async fn init(team_id: &str) -> Result<(), String> {
    let endpoint = http::runtime_endpoint().map_err(|e| e.to_string())?;
    let bearer = http::operator_bearer().map_err(|e| e.to_string())?;
    let client = http::client().map_err(|e| format!("build http client: {}", e))?;
    let url = format!("{}/v1/teams/{}/init", endpoint, team_id);
    let resp = client
        .post(&url)
        .bearer_auth(&bearer)
        .send()
        .await
        .map_err(|e| format!("POST {}: {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "teams init failed (status {}): {}",
            status.as_u16(),
            body
        ));
    }
    let body: InitTeamResponse = resp
        .json()
        .await
        .map_err(|e| format!("decode init response: {}", e))?;
    println!("Team initialized.");
    println!("  team_id: {}", body.team_id);
    println!("  initialized_at: {}", body.initialized_at);
    println!("  tenancy_prefix: {}", body.tenancy_prefix);
    Ok(())
}

async fn status(team_id: &str) -> Result<(), String> {
    let endpoint = http::runtime_endpoint().map_err(|e| e.to_string())?;
    let bearer = http::operator_bearer().map_err(|e| e.to_string())?;
    let client = http::client().map_err(|e| format!("build http client: {}", e))?;
    let url = format!("{}/v1/teams/{}/status", endpoint, team_id);
    let resp = client
        .get(&url)
        .bearer_auth(&bearer)
        .send()
        .await
        .map_err(|e| format!("GET {}: {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "teams status failed (status {}): {}",
            status.as_u16(),
            body
        ));
    }
    let body: TeamStatusResponse = resp
        .json()
        .await
        .map_err(|e| format!("decode status response: {}", e))?;
    println!("Team: {}", body.team_id);
    println!("  initialized_at: {}", body.initialized_at);
    println!("  tenancy_prefix: {}", body.tenancy_prefix);
    println!("  total_inbox_depth: {}", body.total_inbox_depth);
    println!("  registered_agents: {}", body.registered_agents.len());
    for agent in &body.registered_agents {
        println!("    - identity: {}", agent.identity);
        println!("      contexts: {}", agent.contexts.join(", "));
        println!("      inbox_depth: {}", agent.inbox_depth);
    }
    Ok(())
}

async fn delete(team_id: &str, force: bool) -> Result<(), String> {
    let endpoint = http::runtime_endpoint().map_err(|e| e.to_string())?;
    let bearer = http::operator_bearer().map_err(|e| e.to_string())?;

    if !force {
        print!(
            "Delete team {}'s Tally state at {}? [y/N] ",
            team_id, endpoint
        );
        io::stdout()
            .flush()
            .map_err(|e| format!("flush stdout: {}", e))?;
        let mut line = String::new();
        io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| format!("read stdin: {}", e))?;
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return Err("delete aborted by user".into());
        }
    }

    let client = http::client().map_err(|e| format!("build http client: {}", e))?;
    let url = format!("{}/v1/teams/{}", endpoint, team_id);
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
            "teams delete failed (status {}): {}",
            status.as_u16(),
            body
        ));
    }
    println!("Team {}'s Tally state deleted.", team_id);
    println!("(The upstream Stoa team is preserved.)");
    Ok(())
}
