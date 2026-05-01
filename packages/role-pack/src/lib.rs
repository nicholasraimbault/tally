//! Tally Role Pack Format
//!
//! Parser, validator, and types for Tally role packs. The role pack format
//! is the same as Skytale's (defined in skytale's role-pack module); this
//! crate provides a Rust-native implementation for runtime executors.
//!
//! See the role pack format spec at
//! <https://docs.skytale.sh/spec/role-pack> for the canonical schema.
//!
//! Status: skeleton. Real implementation comes in Phase 1B.

/// The current version of the role-pack library.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
