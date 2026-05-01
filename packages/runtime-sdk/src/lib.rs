//! Tally Runtime SDK
//!
//! The SDK that ephemeral executors use to interact with the Tally runtime.
//! This includes wake event handling, heartbeat, idle exit, and catchup.
//!
//! See the wake router protocol spec at
//! <https://docs.skytale.sh/spec/wake-router> for the contract this SDK
//! implements.
//!
//! Status: skeleton. Real implementation comes in Phase 1B.

/// The current version of the runtime SDK.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
