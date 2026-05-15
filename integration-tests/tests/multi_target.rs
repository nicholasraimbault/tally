//! P2 multi-target scenarios per `docs/specs/integration-tests-sub-pr-test-plan.md` §"Scenario catalog" P2.
//!
//! Three tests:
//! 1. `multi_agent_same_team` — A in T1 dispatches to B in T1; B
//!    completes; A receives. Note: structurally identical to
//!    `happy_path::dispatch_and_complete` (P0 #4). The P0 test serves
//!    as the canonical happy-path coverage per the test plan's
//!    deduplication note; this P2 version exists as a slight variation
//!    (distinct identity-pair names, lighter assertions) so the P2
//!    track has standalone coverage of the multi-agent shape.
//! 2. `cross_team_isolation` — A in T1 dispatches to B in T2; T1's DO
//!    has no registration record of B (registration is per-team, since
//!    each (team_id, identity, context_id) tuple lives inside the
//!    team's DO); returns 422 HandlerNotFound.
//! 3. `long_poll_wake_up` *(alarm-fire-adjacent — real wait)* — B
//!    GETs inbox with wait_seconds=30; A dispatches at T+1s; B's poll
//!    returns at ~T+1s with the dispatched wake (assertion loose:
//!    returns before T+5s with non-empty inbox).

use std::time::{Duration, Instant};

use tally_integration_tests::{
    b64_encode, CompleteResponse, DispatchResponse, ReadInboxResponse, TestHarness,
};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

#[test]
fn multi_agent_same_team() {
    // Per test plan §"Scenario catalog" P2 #1: variation of P0 #4
    // `dispatch_and_complete`. Same shape, different identity-pair
    // names (alice/bob explicit) and a smaller end-to-end assertion
    // surface. The canonical assertions (ISO-8601 timestamps, wake_id
    // ULID parsing) live in P0; this version is a deduplicated
    // smoke-check.
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (alice_id, alice_bearer) = harness.new_identity();
    let (bob_id, bob_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Bob registers for "task-routing".
        let resp = harness
            .register(&team_id, &bob_id, &bob_bearer, "task-routing")
            .await
            .expect("bob register");
        assert_eq!(resp.status().as_u16(), 201);

        // Alice dispatches to bob (blocks server-side).
        let payload = b64_encode(b"alice-to-bob");
        let team_clone = team_id.clone();
        let alice_bearer_clone = alice_bearer.clone();
        let bob_id_clone = bob_id.clone();
        let payload_clone = payload.clone();
        let harness_url = harness.base_url.clone();
        let harness_client = harness.client.clone();
        let dispatch_handle = tokio::spawn(async move {
            let url = format!("{}/v1/teams/{}/wakes", harness_url, team_clone);
            let body = serde_json::json!({
                "target_identity": bob_id_clone,
                "context_id": "task-routing",
                "payload": payload_clone,
                "timeout_seconds": 10,
            });
            harness_client
                .post(&url)
                .bearer_auth(&alice_bearer_clone)
                .json(&body)
                .send()
                .await
                .expect("alice dispatch")
        });

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Bob reads his inbox, completes the wake.
        let resp = harness
            .read_inbox(&team_id, &bob_id, &bob_bearer, 5, None)
            .await
            .expect("bob read_inbox");
        let inbox: ReadInboxResponse = resp.json().await.expect("decode");
        assert_eq!(inbox.wakes.len(), 1);
        assert_eq!(inbox.wakes[0].caller_identity, alice_id);
        let wake_id = inbox.wakes[0].wake_id.clone();

        let response = b64_encode(b"bob-to-alice");
        let resp = harness
            .complete(&team_id, &wake_id, &bob_bearer, &response)
            .await
            .expect("bob complete");
        assert_eq!(resp.status().as_u16(), 200);
        let _comp: CompleteResponse = resp.json().await.expect("decode");

        // Alice's dispatch resolves.
        let resp = dispatch_handle.await.expect("join");
        assert_eq!(resp.status().as_u16(), 200);
        let disp: DispatchResponse = resp.json().await.expect("decode");
        assert_eq!(disp.wake_id, wake_id);
        assert_eq!(disp.response, response);
    });
}

#[test]
fn cross_team_isolation() {
    // Per test plan §"Scenario catalog" P2 #2: alice in T1 dispatches
    // to bob; T1's DO has no record of bob's registration (bob
    // registered in T2). T1's dispatch returns 422 HandlerNotFound.
    //
    // Subtle point: tally's DO key schema is `agent:{identity}:handlers`
    // (no team_id prefix because each team has its own DO with its
    // own storage). Registering bob in T2's DO is invisible to T1's
    // DO. Strict isolation by construction.
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_1 = harness.new_team_id();
    let team_2 = harness.new_team_id();
    let (alice_id, alice_bearer) = harness.new_identity();
    let (bob_id, bob_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Bob registers in T2 only.
        let resp = harness
            .register(&team_2, &bob_id, &bob_bearer, "ctx-shared")
            .await
            .expect("bob register in T2");
        assert_eq!(resp.status().as_u16(), 201);

        // Alice in T1 dispatches to bob; T1's DO sees no handler.
        let payload = b64_encode(b"cross-team");
        let resp = harness
            .dispatch(&team_1, &alice_bearer, &bob_id, "ctx-shared", &payload, 5)
            .await
            .expect("alice dispatch in T1");
        assert_eq!(
            resp.status().as_u16(),
            422,
            "cross-team dispatch should yield 422 HandlerNotFound; \
             body: {:?}",
            resp.text().await
        );
        let body: serde_json::Value = resp.json().await.expect("decode 422 body");
        assert_eq!(
            body.get("context_id").and_then(|v| v.as_str()),
            Some("ctx-shared"),
            "422 body should echo context_id per §3.3; got: {}",
            body
        );
        let _ = alice_id;
    });
}

#[test]
fn long_poll_wake_up() {
    // Per test plan §"Scenario catalog" P2 #3: bob subscribes to his
    // inbox with wait_seconds=30; alice dispatches at T+1s; bob's
    // poll returns before T+5s with a non-empty inbox (the dispatched
    // wake). Loose timing assertion (real-wait alarm-fire-adjacent).
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (bob_id, bob_bearer) = harness.new_identity();
    let (_alice_id, alice_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Bob registers.
        let resp = harness
            .register(&team_id, &bob_id, &bob_bearer, "ctx-poll")
            .await
            .expect("bob register");
        assert_eq!(resp.status().as_u16(), 201);

        // Bob subscribes with wait_seconds=30 in a background task.
        // The poll will block server-side until alice's dispatch
        // signals or 30s elapses.
        let team_clone = team_id.clone();
        let bob_id_clone = bob_id.clone();
        let bob_bearer_clone = bob_bearer.clone();
        let harness_url = harness.base_url.clone();
        let harness_client = harness.client.clone();
        let poll_start = Instant::now();
        let poll_handle = tokio::spawn(async move {
            let url = format!(
                "{}/v1/teams/{}/agents/{}/inbox?wait_seconds=30",
                harness_url, team_clone, bob_id_clone
            );
            let resp = harness_client
                .get(&url)
                .bearer_auth(&bob_bearer_clone)
                .send()
                .await
                .expect("bob long-poll");
            (resp, Instant::now())
        });

        // Wait ~1s, then alice dispatches.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let payload = b64_encode(b"wake up!");
        // Spawn dispatch — it blocks server-side until timeout. We
        // don't await it; the test only inspects bob's long-poll.
        let team_clone = team_id.clone();
        let alice_bearer_clone = alice_bearer.clone();
        let bob_id_clone = bob_id.clone();
        let payload_clone = payload.clone();
        let harness_url = harness.base_url.clone();
        let harness_client = harness.client.clone();
        let _dispatch = tokio::spawn(async move {
            let url = format!("{}/v1/teams/{}/wakes", harness_url, team_clone);
            let body = serde_json::json!({
                "target_identity": bob_id_clone,
                "context_id": "ctx-poll",
                "payload": payload_clone,
                "timeout_seconds": 30,
            });
            let _ = harness_client
                .post(&url)
                .bearer_auth(&alice_bearer_clone)
                .json(&body)
                .send()
                .await;
        });

        // Bob's long-poll should resolve shortly after alice dispatches.
        let (resp, returned_at) = poll_handle.await.expect("join poll");
        let elapsed = returned_at - poll_start;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "bob's long-poll should return 200"
        );
        // Loose timing: poll resolved before the 30s wait_seconds AND
        // within ~5s of poll start (must've been signal-driven, not
        // timeout-driven). 5s budget includes ~1s dispatch latency +
        // 1s wait + slack.
        assert!(
            elapsed < Duration::from_secs(5),
            "long-poll should be signal-driven (return <5s); got {:?}",
            elapsed
        );
        let body: ReadInboxResponse = resp.json().await.expect("decode");
        assert_eq!(
            body.wakes.len(),
            1,
            "bob's inbox should contain alice's dispatched wake"
        );
        assert_eq!(body.wakes[0].payload, payload);
    });
}
