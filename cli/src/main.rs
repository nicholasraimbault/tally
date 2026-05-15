//! `tally` CLI binary entry point.
//!
//! Dispatches the parsed `Cli` to the appropriate command module.
//! Per cli-sub-pr-phase-0.md D7: commands return `Result<(), String>`;
//! on `Err` the dispatcher prints the message to stderr and exits with
//! code 1.

use std::process::ExitCode;

use clap::Parser;
use tally_cli::{commands, Cli, Commands};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Init(args) => commands::init::run(args).await,
        Commands::Deploy(args) => commands::deploy::run(args).await,
        Commands::Destroy(args) => commands::destroy::run(args).await,
        Commands::Teams(cmd) => commands::teams::run(cmd).await,
        Commands::Agents(cmd) => commands::agents::run(cmd).await,
        Commands::Version => commands::version::run().await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {}", msg);
            ExitCode::FAILURE
        }
    }
}
