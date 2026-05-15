//! Per-command modules.
//!
//! Each module owns its clap args struct + the `run` function that
//! `main.rs` dispatches to. Command-boundary error type is
//! `Result<(), String>` per cli-sub-pr-phase-0.md D7; internal helpers
//! use `anyhow::Result` and convert at the boundary.

pub mod agents;
pub mod deploy;
pub mod destroy;
pub mod init;
pub mod teams;
pub mod version;
