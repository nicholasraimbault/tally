//! `tally init` — generate operator identity + verify wrangler auth.
//! Implementation in the next commit per the test plan's commit
//! groupings.

#[derive(clap::Args, Debug)]
pub struct InitArgs {
    /// Reconfigure even if `~/.tally/identity` already exists.
    #[arg(long)]
    pub force: bool,
}

pub async fn run(_args: InitArgs) -> Result<(), String> {
    Err("tally init: not yet implemented".to_string())
}
