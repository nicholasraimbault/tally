//! `tally teams init|status|delete` — team-administrative commands.
//! Implementation in the next commit per the test plan's commit
//! groupings.

use clap::Subcommand;

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

pub async fn run(_cmd: TeamsCommand) -> Result<(), String> {
    Err("tally teams: not yet implemented".to_string())
}
