//! `tally deploy` — subprocess-delegate to `wrangler deploy`.
//! Implementation in the next commit per the test plan's commit
//! groupings.

#[derive(clap::Args, Debug)]
pub struct DeployArgs {
    /// Path to `wrangler.toml`. Defaults to `wrangler.toml` in the
    /// current working directory.
    #[arg(long, value_name = "PATH")]
    pub wrangler_toml: Option<std::path::PathBuf>,
}

pub async fn run(_args: DeployArgs) -> Result<(), String> {
    Err("tally deploy: not yet implemented".to_string())
}
