//! `tally init` — generate operator identity + verify wrangler auth.
//!
//! Per cli-sub-pr-phase-0.md D3 + D4:
//!
//! 1. Create `~/.tally/` if missing
//! 2. Refuse to overwrite an existing identity unless `--force`
//! 3. Generate an ed25519 keypair via OsRng; persist the secret seed
//!    as url-safe-b64 at `~/.tally/identity` with 0600 perms
//! 4. Invoke `wrangler whoami` as a subprocess; surface an actionable
//!    error if wrangler is missing or unauthenticated
//! 5. Prompt the operator for their Cloudflare account ID; persist at
//!    `~/.tally/cloudflare-account`
//! 6. Print the operator's identity (url-safe-b64 of the public key)

use std::io::{self, BufRead, Write};
use std::process::Command;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD;
use base64::Engine as _;
use ed25519_dalek::SigningKey;
use rand_core::OsRng;

use crate::config;

#[derive(clap::Args, Debug)]
pub struct InitArgs {
    /// Reconfigure even if `~/.tally/identity` already exists.
    #[arg(long)]
    pub force: bool,
}

pub async fn run(args: InitArgs) -> Result<(), String> {
    config::ensure_config_dir().map_err(|e| format!("create ~/.tally/: {}", e))?;

    if config::identity_exists().map_err(|e| format!("check identity path: {}", e))? && !args.force
    {
        return Err("operator identity already exists at ~/.tally/identity; \
             use --force to reconfigure"
            .to_string());
    }

    // Generate a fresh ed25519 keypair. SigningKey::generate consumes
    // an `RngCore + CryptoRng` source; OsRng (rand_core 0.6) provides
    // both. The 32-byte secret seed lives in `~/.tally/identity`; the
    // public component is derivable via signing_key.verifying_key().
    let signing_key = SigningKey::generate(&mut OsRng);
    config::write_identity(signing_key.as_bytes())
        .map_err(|e| format!("persist identity: {}", e))?;
    let public_b64 = BASE64_URL_SAFE_NO_PAD.encode(signing_key.verifying_key().as_bytes());

    // Verify wrangler is installed + authenticated. D3 locks wrangler
    // as the Cloudflare auth surface; tally delegates rather than
    // implementing its own Cloudflare API flow.
    verify_wrangler_authenticated()?;

    // Prompt for the Cloudflare account ID. Operators can run
    // `wrangler whoami` separately to see their account IDs; we don't
    // try to parse them out of wrangler's stdout (format unstable).
    let account_id = prompt("Cloudflare account ID (from `wrangler whoami`): ")?;
    if account_id.is_empty() {
        return Err("Cloudflare account ID is required".into());
    }
    config::write_cloudflare_account(&account_id)
        .map_err(|e| format!("save cloudflare account: {}", e))?;

    println!();
    println!("Operator identity initialized.");
    println!("  Public identity (url-safe-b64): {}", public_b64);
    println!("  Stored at: ~/.tally/identity (0600)");
    println!("  Cloudflare account: {}", account_id);
    println!();
    println!("Next: run `tally deploy` to provision the Worker.");
    Ok(())
}

/// Run `wrangler whoami` and surface an actionable error if wrangler
/// is missing, fails, or reports unauthenticated.
fn verify_wrangler_authenticated() -> Result<(), String> {
    let output = Command::new("wrangler").arg("whoami").output();
    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("wrangler is not installed; install via `npm install -g wrangler`".into());
        }
        Err(e) => return Err(format!("spawn `wrangler whoami`: {}", e)),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "wrangler is not authenticated; run `wrangler login`\n\nwrangler stderr:\n{}",
            stderr.trim()
        ));
    }
    Ok(())
}

/// Print a prompt and read a single trimmed line from stdin.
fn prompt(message: &str) -> Result<String, String> {
    print!("{}", message);
    io::stdout()
        .flush()
        .map_err(|e| format!("flush stdout: {}", e))?;
    let stdin = io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("read stdin: {}", e))?;
    Ok(line.trim().to_string())
}
