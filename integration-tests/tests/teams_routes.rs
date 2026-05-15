//! Path A coverage per `docs/specs/cli-sub-pr-phase-0.md`'s
//! "Runtime API surface gap — Path A locked" section.
//!
//! Five scenarios covering the 3 new team-administrative routes:
//! 1. `team_init_idempotent` — POST /init twice; same initialized_at
//! 2. `team_status_empty_team` — GET /status on fresh team; zero agents
//! 3. `team_status_with_registrations` — register 2 agents in distinct
//!    contexts; status enumerates them with contexts + zero inbox depth
//! 4. `team_delete_resets_state` — register; delete (204); subsequent
//!    status shows fresh empty team with a NEW initialized_at
//! 5. `team_init_no_bearer_401` — missing Authorization → 401

use std::time::Duration;

use tally_integration_tests::{InitTeamResponse, TeamStatusResponse, TestHarness};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

#[test]
fn team_init_idempotent() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (_op_id, op_bearer) = harness.new_identity();

    runtime.block_on(async {
        // First init.
        let resp = harness
            .team_init(&team_id, &op_bearer)
            .await
            .expect("team_init #1");
        assert_eq!(resp.status().as_u16(), 200, "first init should be 200");
        let first: InitTeamResponse = resp.json().await.expect("decode #1");
        assert!(!first.team_id.is_empty(), "team_id should be non-empty");
        assert!(
            first.initialized_at.ends_with('Z'),
            "initialized_at should be ISO-8601 UTC: {}",
            first.initialized_at
        );
        assert_eq!(
            first.tenancy_prefix, "tally-cli-local",
            "MVP tenancy_prefix matches TENANCY_PREFIX_MVP"
        );

        // Second init — must return identical metadata (first-init
        // timestamp preserved per idempotency contract).
        let resp = harness
            .team_init(&team_id, &op_bearer)
            .await
            .expect("team_init #2");
        assert_eq!(resp.status().as_u16(), 200, "second init should be 200");
        let second: InitTeamResponse = resp.json().await.expect("decode #2");
        assert_eq!(
            second.team_id, first.team_id,
            "team_id stable across re-init"
        );
        assert_eq!(
            second.initialized_at, first.initialized_at,
            "initialized_at preserved across re-init (idempotent)"
        );
        assert_eq!(second.tenancy_prefix, first.tenancy_prefix);
    });
}

#[test]
fn team_status_empty_team() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (_op_id, op_bearer) = harness.new_identity();

    runtime.block_on(async {
        let resp = harness
            .team_status(&team_id, &op_bearer)
            .await
            .expect("team_status");
        assert_eq!(resp.status().as_u16(), 200);
        let status: TeamStatusResponse = resp.json().await.expect("decode");
        assert!(
            status.registered_agents.is_empty(),
            "fresh team should have no registered agents; got: {:?}",
            status.registered_agents
        );
        assert_eq!(
            status.total_inbox_depth, 0,
            "fresh team should have zero total inbox depth"
        );
        assert_eq!(status.tenancy_prefix, "tally-cli-local");
    });
}

#[test]
fn team_status_with_registrations() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (_op_id, op_bearer) = harness.new_identity();
    let (alice_id, alice_bearer) = harness.new_identity();
    let (bob_id, bob_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Register alice in ctx-A.
        let resp = harness
            .register(&team_id, &alice_id, &alice_bearer, "ctx-A")
            .await
            .expect("alice register");
        assert_eq!(resp.status().as_u16(), 201);

        // Register bob in ctx-B and ctx-C.
        let resp = harness
            .register(&team_id, &bob_id, &bob_bearer, "ctx-B")
            .await
            .expect("bob register ctx-B");
        assert_eq!(resp.status().as_u16(), 201);
        let resp = harness
            .register(&team_id, &bob_id, &bob_bearer, "ctx-C")
            .await
            .expect("bob register ctx-C");
        assert_eq!(resp.status().as_u16(), 201);

        // Read team status via operator bearer.
        let resp = harness
            .team_status(&team_id, &op_bearer)
            .await
            .expect("team_status");
        assert_eq!(resp.status().as_u16(), 200);
        let status: TeamStatusResponse = resp.json().await.expect("decode");

        assert_eq!(
            status.registered_agents.len(),
            2,
            "should enumerate 2 registered agents; got: {:?}",
            status.registered_agents
        );

        // Index by identity for stable assertion regardless of
        // BTreeSet iteration order.
        let alice_entry = status
            .registered_agents
            .iter()
            .find(|a| a.identity == alice_id)
            .expect("alice entry present");
        let bob_entry = status
            .registered_agents
            .iter()
            .find(|a| a.identity == bob_id)
            .expect("bob entry present");

        assert_eq!(alice_entry.contexts, vec!["ctx-A".to_string()]);
        assert_eq!(alice_entry.inbox_depth, 0);

        // bob has two contexts; the DO returns them sorted (BTreeSet
        // ordering).
        let mut bob_contexts = bob_entry.contexts.clone();
        bob_contexts.sort();
        assert_eq!(bob_contexts, vec!["ctx-B".to_string(), "ctx-C".to_string()]);
        assert_eq!(bob_entry.inbox_depth, 0);

        assert_eq!(status.total_inbox_depth, 0);
    });
}

#[test]
fn team_delete_resets_state() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (_op_id, op_bearer) = harness.new_identity();
    let (alice_id, alice_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Register alice; capture pre-delete initialized_at.
        let resp = harness
            .register(&team_id, &alice_id, &alice_bearer, "ctx-A")
            .await
            .expect("alice register");
        assert_eq!(resp.status().as_u16(), 201);
        let resp = harness
            .team_status(&team_id, &op_bearer)
            .await
            .expect("status pre-delete");
        let pre: TeamStatusResponse = resp.json().await.expect("decode pre");
        assert_eq!(pre.registered_agents.len(), 1);
        let pre_initialized_at = pre.initialized_at.clone();

        // Sleep briefly so post-delete re-init produces a NEW timestamp
        // (ISO-8601 second-precision; 1.1s ensures the second tick).
        tokio::time::sleep(Duration::from_millis(1_100)).await;

        // Delete.
        let resp = harness
            .team_delete(&team_id, &op_bearer)
            .await
            .expect("team_delete");
        assert_eq!(
            resp.status().as_u16(),
            204,
            "delete should return 204 No Content"
        );

        // Post-delete status — fresh empty team with new initialized_at.
        let resp = harness
            .team_status(&team_id, &op_bearer)
            .await
            .expect("status post-delete");
        assert_eq!(resp.status().as_u16(), 200);
        let post: TeamStatusResponse = resp.json().await.expect("decode post");
        assert!(
            post.registered_agents.is_empty(),
            "post-delete agents should be empty; got: {:?}",
            post.registered_agents
        );
        assert_eq!(post.total_inbox_depth, 0);
        assert_ne!(
            post.initialized_at, pre_initialized_at,
            "post-delete initialized_at should differ (fresh re-init)"
        );
    });
}

#[test]
fn team_init_no_bearer_401() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();

    runtime.block_on(async {
        // Direct request without Authorization header.
        let url = format!("{}/v1/teams/{}/init", harness.base_url, team_id);
        let resp = harness
            .client
            .post(&url)
            .send()
            .await
            .expect("post without bearer");
        assert_eq!(
            resp.status().as_u16(),
            401,
            "missing Authorization should yield 401"
        );
        let body: serde_json::Value = resp.json().await.expect("decode 401 body");
        assert!(
            body.get("error").is_some(),
            "401 body should carry an error field per §3.3 structured-error shape"
        );
    });
}
