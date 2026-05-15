//! Smoke tests for the `tally` CLI binary.
//!
//! Verifies clap-parser-level wiring of the 11-command catalog from
//! cli-sub-pr-phase-0.md. Runtime behaviors (`tally init` writing to
//! `~/.tally/`, `tally deploy` shelling to wrangler, etc.) are not
//! exercised here — those require operator state + external tooling
//! and live in higher-level integration coverage.

use clap::Parser;
use tally_cli::{Cli, Commands};

#[test]
fn clap_parses_version() {
    let cli = Cli::try_parse_from(["tally", "version"]).expect("parse 'tally version'");
    assert!(matches!(cli.command, Commands::Version));
}

#[test]
fn clap_parses_init_with_force() {
    let cli =
        Cli::try_parse_from(["tally", "init", "--force"]).expect("parse 'tally init --force'");
    match cli.command {
        Commands::Init(args) => assert!(args.force),
        other => panic!("expected Init, got {:?}", other),
    }
}

#[test]
fn clap_parses_teams_status() {
    let cli =
        Cli::try_parse_from(["tally", "teams", "status", "abc123"]).expect("parse teams status");
    match cli.command {
        Commands::Teams(tally_cli::commands::teams::TeamsCommand::Status { team_id }) => {
            assert_eq!(team_id, "abc123");
        }
        other => panic!("expected Teams::Status, got {:?}", other),
    }
}

#[test]
fn clap_parses_agents_register() {
    let cli = Cli::try_parse_from([
        "tally",
        "agents",
        "register",
        "--team",
        "T1",
        "--identity",
        "AAAA",
        "--context",
        "ctx-A",
    ])
    .expect("parse agents register");
    match cli.command {
        Commands::Agents(tally_cli::commands::agents::AgentsCommand::Register {
            team,
            identity,
            context,
        }) => {
            assert_eq!(team, "T1");
            assert_eq!(identity, "AAAA");
            assert_eq!(context, "ctx-A");
        }
        other => panic!("expected Agents::Register, got {:?}", other),
    }
}

#[test]
fn clap_parses_agents_key_issue() {
    let cli = Cli::try_parse_from([
        "tally",
        "agents",
        "key",
        "issue",
        "--team",
        "T1",
        "--identity",
        "AAAA",
    ])
    .expect("parse agents key issue");
    match cli.command {
        Commands::Agents(tally_cli::commands::agents::AgentsCommand::Key(
            tally_cli::commands::agents::KeyCommand::Issue { team, identity },
        )) => {
            assert_eq!(team, "T1");
            assert_eq!(identity, "AAAA");
        }
        other => panic!("expected Agents::Key::Issue, got {:?}", other),
    }
}

#[test]
fn clap_parses_agents_key_revoke() {
    let cli = Cli::try_parse_from([
        "tally",
        "agents",
        "key",
        "revoke",
        "--team",
        "T1",
        "--identity",
        "AAAA",
    ])
    .expect("parse agents key revoke");
    assert!(matches!(
        cli.command,
        Commands::Agents(tally_cli::commands::agents::AgentsCommand::Key(
            tally_cli::commands::agents::KeyCommand::Revoke { .. }
        ))
    ));
}

#[test]
fn clap_rejects_unknown_command() {
    let result = Cli::try_parse_from(["tally", "bogus"]);
    assert!(
        result.is_err(),
        "unknown command should fail to parse; got: {:?}",
        result
    );
}
