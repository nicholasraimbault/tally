//! Tally MCP Plugin
//!
//! Claude Code MCP plugin that exposes Tally team coordination tools.
//! Distributed via npm as `@skytale/tally-mcp` (the npm package wraps
//! a node binary that calls this Rust core via NAPI or similar).
//!
//! Status: skeleton. Real implementation comes in Phase 1B.

/// The current version of the MCP plugin.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
