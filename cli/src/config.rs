//! `~/.tally/` config directory management.
//!
//! Per cli-sub-pr-phase-0.md D6 the CLI's local state lives under
//! `~/.tally/`:
//!
//! ```text
//! ~/.tally/
//! ├── identity            # raw ed25519 secret seed (32 bytes), url-safe-b64
//! ├── runtime-endpoint    # text: deployed Tally HTTP base URL
//! └── cloudflare-account  # text: Cloudflare account ID (for tally destroy)
//! ```
//!
//! No SQLite, no encrypted store at MVP — these are operator-facing
//! config artifacts, not encrypted channel state.

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD;
use base64::Engine as _;

const CONFIG_DIR: &str = ".tally";
const IDENTITY_FILE: &str = "identity";
const RUNTIME_ENDPOINT_FILE: &str = "runtime-endpoint";
const CLOUDFLARE_ACCOUNT_FILE: &str = "cloudflare-account";

/// Resolve `~/.tally/`. Returns an error if the home directory cannot
/// be determined (rare on Linux/macOS; can happen on stripped-down
/// containers without `HOME` set).
pub fn config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not determine home directory"))?;
    Ok(home.join(CONFIG_DIR))
}

/// Resolve `~/.tally/identity`.
pub fn identity_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(IDENTITY_FILE))
}

/// Resolve `~/.tally/runtime-endpoint`.
pub fn runtime_endpoint_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(RUNTIME_ENDPOINT_FILE))
}

/// Resolve `~/.tally/cloudflare-account`.
pub fn cloudflare_account_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(CLOUDFLARE_ACCOUNT_FILE))
}

/// Ensure `~/.tally/` exists; create with 0700 perms on Unix.
pub fn ensure_config_dir() -> Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("create config dir {}", dir.display()))?;
    Ok(())
}

/// Read the operator identity from `~/.tally/identity`. Returns the
/// url-safe-base64 string the file contains (no padding); callers can
/// pass this directly as a Bearer token under MVP D5 where
/// `Bearer = url_safe_b64(identity_bytes)`.
pub fn read_identity() -> Result<String> {
    let path = identity_path()?;
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read identity at {}", path.display()))?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return Err(anyhow!(
            "identity file at {} is empty; run 'tally init' to initialize",
            path.display()
        ));
    }
    Ok(trimmed)
}

/// Write the operator identity to `~/.tally/identity`. `raw_bytes` is
/// the ed25519 secret seed (32 bytes); this function url-safe-base64
/// encodes it and writes the resulting string.
pub fn write_identity(raw_bytes: &[u8]) -> Result<()> {
    let path = identity_path()?;
    let encoded = BASE64_URL_SAFE_NO_PAD.encode(raw_bytes);
    fs::write(&path, encoded).with_context(|| format!("write identity to {}", path.display()))?;
    // Best-effort 0600 perms on Unix; ignore on other platforms.
    set_owner_only(&path)?;
    Ok(())
}

/// Check whether `~/.tally/identity` exists.
pub fn identity_exists() -> Result<bool> {
    Ok(identity_path()?.exists())
}

/// Read the runtime endpoint URL from `~/.tally/runtime-endpoint`, or
/// `None` if the file is absent (operator hasn't yet deployed).
pub fn read_runtime_endpoint() -> Result<Option<String>> {
    let path = runtime_endpoint_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read runtime endpoint at {}", path.display()))?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed))
    }
}

/// Write the runtime endpoint URL to `~/.tally/runtime-endpoint`.
pub fn write_runtime_endpoint(url: &str) -> Result<()> {
    let path = runtime_endpoint_path()?;
    fs::write(&path, url)
        .with_context(|| format!("write runtime endpoint to {}", path.display()))?;
    Ok(())
}

/// Clear the runtime endpoint (e.g., after `tally destroy`).
pub fn clear_runtime_endpoint() -> Result<()> {
    let path = runtime_endpoint_path()?;
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("remove runtime endpoint at {}", path.display()))?;
    }
    Ok(())
}

/// Write the Cloudflare account ID to `~/.tally/cloudflare-account`.
pub fn write_cloudflare_account(account_id: &str) -> Result<()> {
    let path = cloudflare_account_path()?;
    fs::write(&path, account_id)
        .with_context(|| format!("write cloudflare account to {}", path.display()))?;
    Ok(())
}

/// Read the Cloudflare account ID from `~/.tally/cloudflare-account`.
pub fn read_cloudflare_account() -> Result<Option<String>> {
    let path = cloudflare_account_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read cloudflare account at {}", path.display()))?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed))
    }
}

#[cfg(unix)]
fn set_owner_only(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)
        .with_context(|| format!("set 0600 perms on {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_is_under_home() {
        // Smoke check: config_dir resolves and ends with `.tally`.
        if let Ok(dir) = config_dir() {
            assert!(dir.ends_with(CONFIG_DIR));
        }
    }
}
