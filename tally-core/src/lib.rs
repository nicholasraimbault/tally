//! Shared types for the Tally Cloudflare runtime and Tally CLI.
//!
//! Storage types for the `TallyTeamDO` state model land here so the
//! runtime crate (`tally-worker`) and future consumers (Tally CLI,
//! non-Cloudflare implementations) share a single canonical definition.

#![forbid(unsafe_code)]

pub mod team_meta;
pub mod wake_record;

pub use team_meta::TeamMeta;
pub use wake_record::{WakeRecord, WakeState};
