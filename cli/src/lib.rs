//! Tally CLI — operator/team/agent commands for the Tally Cloudflare runtime.
//!
//! Public surface for the `tally` binary plus shared helpers used by
//! the smoke tests in `tests/`. See
//! [`tally/docs/specs/cli-sub-pr-phase-0.md`](../../docs/specs/cli-sub-pr-phase-0.md)
//! for the locked design decisions (D1-D11) and command catalog.
//!
//! # Modules
//!
//! - [`commands`] — clap subcommand definitions + per-command logic
//! - [`config`] — `~/.tally/` directory management (identity, runtime
//!   endpoint, Cloudflare account)
//! - [`http`] — reqwest client wrapper + Bearer auth construction per
//!   the D4 split (operator vs agent Bearer semantics)

#![forbid(unsafe_code)]

pub mod commands;
pub mod config;
pub mod http;

use clap::{Parser, Subcommand};

/// Top-level `tally` CLI entry. Per cli-sub-pr-phase-0.md D7 each
/// command returns `Result<(), String>` at the boundary; the
/// dispatcher in `main.rs` prints the error and exits 1.
#[derive(Parser, Debug)]
#[command(
    name = "tally",
    version,
    about = "Tally CLI — operator/team/agent commands for the Tally Cloudflare runtime",
    long_about = None,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Top-level subcommand enum. Mirrors the 11-command catalog from
/// cli-sub-pr-phase-0.md "Command catalog" section.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Configure operator-level Tally state on the local machine.
    Init(commands::init::InitArgs),
    /// Deploy the Tally Worker code to Cloudflare.
    Deploy(commands::deploy::DeployArgs),
    /// Tear down a Tally deployment.
    Destroy(commands::destroy::DestroyArgs),
    /// Team-administrative commands (init/status/delete).
    #[command(subcommand)]
    Teams(commands::teams::TeamsCommand),
    /// Agent-level commands (register/unregister/key issue/key revoke).
    #[command(subcommand)]
    Agents(commands::agents::AgentsCommand),
    /// Print Tally version + Stoa trait surface version + runtime endpoint.
    Version,
}
