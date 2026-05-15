//! `tally deploy` — subprocess-delegate to `wrangler deploy`.
//!
//! Per cli-sub-pr-phase-0.md D2 the CLI shells out to wrangler rather
//! than reimplementing the Cloudflare Workers Deploy API in pure
//! Rust. The deployed URL is captured separately via the
//! `--runtime-endpoint` arg (operator copies it from wrangler's
//! output and passes it back); attempting to parse wrangler's output
//! couples to an unstable format across versions.

use std::path::PathBuf;
use std::process::Command;

use crate::config;

#[derive(clap::Args, Debug)]
pub struct DeployArgs {
    /// Path to `wrangler.toml`. Defaults to `wrangler.toml` in the
    /// current working directory.
    #[arg(long, value_name = "PATH")]
    pub wrangler_toml: Option<PathBuf>,
    /// The deployed runtime URL to persist at `~/.tally/runtime-endpoint`.
    /// Required because wrangler's output format is unstable across
    /// versions; operators copy the URL from wrangler's deploy banner.
    #[arg(long, value_name = "URL")]
    pub runtime_endpoint: Option<String>,
}

pub async fn run(args: DeployArgs) -> Result<(), String> {
    // 1. Verify operator initialized.
    config::read_identity()
        .map_err(|_| "run `tally init` first to initialize operator identity".to_string())?;

    // 2. Locate wrangler.toml.
    let toml_path = args
        .wrangler_toml
        .clone()
        .unwrap_or_else(|| PathBuf::from("wrangler.toml"));
    if !toml_path.exists() {
        return Err(format!(
            "wrangler.toml not found at {}; cd to the tally repo or pass --wrangler-toml",
            toml_path.display()
        ));
    }

    // 3. Run `wrangler deploy` with inherited stdout/stderr so the
    // operator sees wrangler's output directly. Pass --config if a
    // custom path was provided.
    let mut cmd = Command::new("wrangler");
    cmd.arg("deploy");
    if let Some(p) = args.wrangler_toml.as_ref() {
        cmd.arg("--config").arg(p);
    }
    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("wrangler is not installed; install via `npm install -g wrangler`".into());
        }
        Err(e) => return Err(format!("spawn `wrangler deploy`: {}", e)),
    };
    if !status.success() {
        return Err(format!(
            "wrangler deploy failed (exit {})",
            status.code().unwrap_or(-1)
        ));
    }

    // 4. Persist the runtime endpoint if provided.
    match args.runtime_endpoint {
        Some(url) => {
            let url = url.trim().to_string();
            if url.is_empty() {
                return Err("--runtime-endpoint must be non-empty".into());
            }
            config::write_runtime_endpoint(&url)
                .map_err(|e| format!("save runtime endpoint: {}", e))?;
            println!();
            println!("Saved runtime endpoint: {}", url);
            println!("Run `tally version` to confirm.");
        }
        None => {
            println!();
            println!(
                "Deployment succeeded. The CLI did not persist the runtime URL because \
                 --runtime-endpoint was not supplied. Re-run with:"
            );
            println!();
            println!("  tally deploy --runtime-endpoint <URL-from-wrangler-output>");
            println!();
            println!("Or write the URL directly to ~/.tally/runtime-endpoint.");
        }
    }
    Ok(())
}
