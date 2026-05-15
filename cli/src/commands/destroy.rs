//! `tally destroy` — subprocess-delegate to `wrangler delete`.
//!
//! Per cli-sub-pr-phase-0.md command catalog #3: reads the configured
//! runtime endpoint, confirms interactively (unless `--force`), shells
//! to `wrangler delete`, and clears `~/.tally/runtime-endpoint`.

use std::io::{self, BufRead, Write};
use std::process::Command;

use crate::config;

#[derive(clap::Args, Debug)]
pub struct DestroyArgs {
    /// Skip the interactive confirmation prompt.
    #[arg(long)]
    pub force: bool,
}

pub async fn run(args: DestroyArgs) -> Result<(), String> {
    let endpoint = config::read_runtime_endpoint()
        .map_err(|e| format!("read runtime endpoint: {}", e))?
        .ok_or_else(|| {
            "no runtime endpoint configured; nothing to destroy. \
             Run `tally deploy --runtime-endpoint <URL>` first."
                .to_string()
        })?;

    if !args.force {
        print!("Destroy deployment at {}? [y/N] ", endpoint);
        io::stdout()
            .flush()
            .map_err(|e| format!("flush stdout: {}", e))?;
        let mut line = String::new();
        io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| format!("read stdin: {}", e))?;
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return Err("destroy aborted by user".into());
        }
    }

    // Subprocess-delegate to wrangler. wrangler's `delete` subcommand
    // tears down the Worker; the exact script name comes from the
    // operator's wrangler.toml.
    let status = match Command::new("wrangler").arg("delete").status() {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("wrangler is not installed; install via `npm install -g wrangler`".into());
        }
        Err(e) => return Err(format!("spawn `wrangler delete`: {}", e)),
    };
    if !status.success() {
        return Err(format!(
            "wrangler delete failed (exit {})",
            status.code().unwrap_or(-1)
        ));
    }

    config::clear_runtime_endpoint().map_err(|e| format!("clear runtime endpoint: {}", e))?;
    println!();
    println!("Deployment destroyed.");
    println!("Cleared ~/.tally/runtime-endpoint.");
    Ok(())
}
