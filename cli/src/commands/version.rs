//! `tally version` — print binary version + Stoa trait surface version
//! + runtime endpoint (per cli-sub-pr-phase-0.md D9).
//!
//! Multi-line output with explicit labels:
//!
//! ```text
//! tally 0.1.0
//! stoa-rs: rev 1527a7b
//! runtime: https://tally.workers.dev (configured)
//! ```
//!
//! If the runtime endpoint is not configured (pre-`tally deploy`),
//! the runtime line reads `runtime: (not configured — run 'tally init'
//! or 'tally deploy')`.

use crate::config;

/// Stoa git rev pin (matches `tally/Cargo.toml`'s workspace.dependencies
/// `stoa = { git = "...", rev = "1527a7b" }`). Bump in lockstep when
/// the workspace pin moves.
const STOA_RS_REV: &str = "1527a7b";

/// CLI binary version. `env!("CARGO_PKG_VERSION")` reads from this
/// crate's `Cargo.toml` at build time.
const TALLY_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run `tally version`.
pub async fn run() -> Result<(), String> {
    println!("tally {}", TALLY_CLI_VERSION);
    println!("stoa-rs: rev {}", STOA_RS_REV);

    match config::read_runtime_endpoint() {
        Ok(Some(endpoint)) => println!("runtime: {} (configured)", endpoint),
        Ok(None) => println!("runtime: (not configured — run 'tally init' or 'tally deploy')"),
        Err(e) => {
            // Config-dir resolution failure (rare). Surface as soft
            // diagnostic rather than failing the command.
            println!("runtime: (could not read config: {})", e);
        }
    }
    Ok(())
}
