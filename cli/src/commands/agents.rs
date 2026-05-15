//! `tally agents register|unregister|key issue|key revoke` — agent-level
//! commands. Implementation in the next commit per the test plan's
//! commit groupings.

use clap::Subcommand;

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
        /// The context_id to register for (single shared context if
        /// omitted).
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

pub async fn run(_cmd: AgentsCommand) -> Result<(), String> {
    Err("tally agents: not yet implemented".to_string())
}
