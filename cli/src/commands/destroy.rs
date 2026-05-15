//! `tally destroy` — subprocess-delegate to `wrangler delete`.
//! Implementation in the next commit per the test plan's commit
//! groupings.

#[derive(clap::Args, Debug)]
pub struct DestroyArgs {
    /// Skip the interactive confirmation prompt.
    #[arg(long)]
    pub force: bool,
}

pub async fn run(_args: DestroyArgs) -> Result<(), String> {
    Err("tally destroy: not yet implemented".to_string())
}
