//! P1 error-code coverage tests per
//! `docs/specs/integration-tests-sub-pr-test-plan.md` §"Scenario catalog" P1.
//!
//! Each scenario asserts:
//! - HTTP status matches §3.1 mapping
//! - response body is `{ "error": "..." [, ...contextual fields] }` per §3.3
//!
//! Seven tests:
//! 1. `error_400_malformed` — malformed JSON returns 400.
//! 2. `error_401_missing_bearer` — missing Authorization → 401.
//! 3. `error_401_invalid_bearer` — undecodable Bearer → 401.
//! 4. `error_403_identity_mismatch` — Bearer A on /agents/B/inbox → 403.
//! 5. `error_404_wake_not_found` — complete a nonexistent wake_id → 404
//!    with `wake_id` in body.
//! 6. `error_408_timeout` *(alarm-fire — real wait)* — dispatch with
//!    `timeout_seconds=1`; nothing completes; after ~2s the dispatcher
//!    receives 408 with `wake_id` + `timeout_seconds` in body.
//! 7. `error_410_already_terminal` — complete a wake twice; second
//!    call returns 410 with `wake_id`.
//! 8. `error_422_handler_not_found` — dispatch to unregistered
//!    (target, context_id) returns 422 with `context_id`.

use std::time::{Duration, Instant};

use tally_integration_tests::{b64_encode, CompleteResponse, ReadInboxResponse, TestHarness};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

#[test]
fn error_400_malformed() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (_caller, caller_bearer) = harness.new_identity();

    runtime.block_on(async {
        let url = format!("{}/v1/teams/{}/wakes", harness.base_url, team_id);
        let resp = harness
            .client
            .post(&url)
            .bearer_auth(&caller_bearer)
            .header("content-type", "application/json")
            .body("not valid json at all }{")
            .send()
            .await
            .expect("malformed POST");
        assert_eq!(
            resp.status().as_u16(),
            400,
            "malformed JSON should return 400; body: {:?}",
            resp.text().await
        );
    });
}

#[test]
fn error_401_missing_bearer() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (target_id, _) = harness.new_identity();

    runtime.block_on(async {
        // Request without Authorization header → 401 per §3.1.
        let url = format!(
            "{}/v1/teams/{}/agents/{}/inbox?wait_seconds=0",
            harness.base_url, team_id, target_id
        );
        let resp = harness
            .client
            .get(&url)
            .send()
            .await
            .expect("GET without auth");
        assert_eq!(resp.status().as_u16(), 401);
        let body: serde_json::Value = resp.json().await.expect("decode 401 body");
        assert!(
            body.get("error").is_some(),
            "401 body should have `error` field per §3.3; got: {}",
            body
        );
    });
}

#[test]
fn error_401_invalid_bearer() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (target_id, _) = harness.new_identity();

    runtime.block_on(async {
        // Use a bearer that's not valid url-safe base64 of identity
        // bytes — the DO's validate_api_key parses bearer as identity
        // via from_url_safe_b64; malformed-base64 → invalid.
        let bad_bearer = "@@@@not!valid!base64@@@@";
        let url = format!(
            "{}/v1/teams/{}/agents/{}/inbox?wait_seconds=0",
            harness.base_url, team_id, target_id
        );
        let resp = harness
            .client
            .get(&url)
            .bearer_auth(bad_bearer)
            .send()
            .await
            .expect("GET invalid bearer");
        assert_eq!(resp.status().as_u16(), 401);
        let body: serde_json::Value = resp.json().await.expect("decode 401 body");
        assert!(body.get("error").is_some());
    });
}

#[test]
fn error_403_identity_mismatch() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (alice_id, alice_bearer) = harness.new_identity();
    let (bob_id, _bob_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Alice's bearer hitting Bob's inbox path → 403.
        let resp = harness
            .read_inbox(&team_id, &bob_id, &alice_bearer, 0, None)
            .await
            .expect("read bob's inbox as alice");
        assert_eq!(
            resp.status().as_u16(),
            403,
            "alice's bearer on /agents/bob/inbox should yield 403; \
             body: {:?}",
            resp.text().await
        );
        let _ = alice_id;
    });
}

#[test]
fn error_404_wake_not_found() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (_caller, caller_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Synthesize a well-formed wake_id (ULID) that has no
        // corresponding storage row. The server's complete_wake
        // returns TallyError::WakeNotFound → 404 per §3.1, body
        // includes `wake_id` per §3.3.
        let fake_wake_id = ulid::Ulid::new().to_string();
        let response = b64_encode(b"unused");
        let resp = harness
            .complete(&team_id, &fake_wake_id, &caller_bearer, &response)
            .await
            .expect("complete nonexistent wake");
        assert_eq!(
            resp.status().as_u16(),
            404,
            "complete of nonexistent wake should return 404"
        );
        let body: serde_json::Value = resp.json().await.expect("decode 404 body");
        assert!(body.get("error").is_some());
        assert_eq!(
            body.get("wake_id").and_then(|v| v.as_str()),
            Some(fake_wake_id.as_str()),
            "404 body should echo the wake_id per §3.3; got: {}",
            body
        );
    });
}

#[test]
// Previously #[ignore]'d (PR #20) with attribution to "wrangler dev
// --local does not invoke DO alarm handlers; miniflare/wrangler-dev
// emulation gap." That attribution was misdiagnosis. The actual root
// cause was that tally-worker's `reschedule_alarm` passed an absolute
// unix-millisecond timestamp to `set_alarm(i64)` while worker-rs's
// `From<i64> for ScheduledTime` interprets the value as offset-ms-
// from-`Date::now()`. Alarms were scheduled for ~year 57,000 CE and
// never fired in EITHER wrangler dev --local OR production. Fixed on
// the `alarm-fire-diag` branch; the test is now un-ignored.
fn error_408_timeout() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (target_id, target_bearer) = harness.new_identity();
    let (_caller, caller_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Register so dispatch is not refused for HandlerNotFound.
        let resp = harness
            .register(&team_id, &target_id, &target_bearer, "ctx-timeout")
            .await
            .expect("register");
        assert_eq!(resp.status().as_u16(), 201);

        // Dispatch with timeout_seconds=1; never complete. The alarm
        // fires after ~1s (plus the SAFETY_BUFFER); the dispatch call
        // returns 408 with `wake_id` + `timeout_seconds` per §3.3.
        let started = Instant::now();
        let payload = b64_encode(b"will timeout");
        let resp = harness
            .dispatch(
                &team_id,
                &caller_bearer,
                &target_id,
                "ctx-timeout",
                &payload,
                1,
            )
            .await
            .expect("dispatch (will timeout)");
        let elapsed = started.elapsed();
        assert_eq!(
            resp.status().as_u16(),
            408,
            "dispatch with timeout_seconds=1 and no complete should return 408; \
             elapsed: {:?}; body: {:?}",
            elapsed,
            resp.text().await
        );
        // <10s wall-clock budget per test plan; <5s allowance per scenario.
        assert!(
            elapsed < Duration::from_secs(5),
            "408 should fire within 5s of dispatch start; got {:?}",
            elapsed
        );
        let body: serde_json::Value = resp.json().await.expect("decode 408 body");
        assert!(body.get("error").is_some());
        assert!(
            body.get("wake_id").and_then(|v| v.as_str()).is_some(),
            "408 body should include `wake_id` per §3.3; got: {}",
            body
        );
        assert_eq!(
            body.get("timeout_seconds").and_then(|v| v.as_u64()),
            Some(1),
            "408 body should include `timeout_seconds: 1` per §3.3; got: {}",
            body
        );
    });
}

#[test]
fn error_410_already_terminal() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (target_id, target_bearer) = harness.new_identity();
    let (_caller, caller_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Setup: target registers, caller dispatches, target reads
        // inbox to find wake_id, target completes, target tries to
        // complete again → 410.
        let resp = harness
            .register(&team_id, &target_id, &target_bearer, "ctx-x")
            .await
            .expect("register");
        assert_eq!(resp.status().as_u16(), 201);

        // Spawn dispatcher.
        let payload = b64_encode(b"once");
        let dispatcher_team_id = team_id.clone();
        let dispatcher_caller_bearer = caller_bearer.clone();
        let dispatcher_target = target_id.clone();
        let dispatcher_payload = payload.clone();
        let harness_url = harness.base_url.clone();
        let harness_client = harness.client.clone();
        let dispatch_handle = tokio::spawn(async move {
            let url = format!("{}/v1/teams/{}/wakes", harness_url, dispatcher_team_id);
            let body = serde_json::json!({
                "target_identity": dispatcher_target,
                "context_id": "ctx-x",
                "payload": dispatcher_payload,
                "timeout_seconds": 10,
            });
            harness_client
                .post(&url)
                .bearer_auth(&dispatcher_caller_bearer)
                .json(&body)
                .send()
                .await
                .expect("dispatch req")
        });

        tokio::time::sleep(Duration::from_millis(500)).await;

        let resp = harness
            .read_inbox(&team_id, &target_id, &target_bearer, 5, None)
            .await
            .expect("read_inbox");
        let inbox: ReadInboxResponse = resp.json().await.expect("decode");
        assert_eq!(inbox.wakes.len(), 1);
        let wake_id = inbox.wakes[0].wake_id.clone();

        // First complete: succeeds.
        let response_payload = b64_encode(b"answer");
        let resp = harness
            .complete(&team_id, &wake_id, &target_bearer, &response_payload)
            .await
            .expect("first complete");
        assert_eq!(resp.status().as_u16(), 200);
        let _comp: CompleteResponse = resp.json().await.expect("decode");

        // Drain dispatcher.
        let _ = dispatch_handle.await;

        // Second complete on the same wake_id: 410.
        let resp = harness
            .complete(&team_id, &wake_id, &target_bearer, &response_payload)
            .await
            .expect("second complete");
        assert_eq!(
            resp.status().as_u16(),
            410,
            "second complete should return 410 AlreadyTerminal"
        );
        let body: serde_json::Value = resp.json().await.expect("decode 410 body");
        assert!(body.get("error").is_some());
        assert_eq!(
            body.get("wake_id").and_then(|v| v.as_str()),
            Some(wake_id.as_str()),
            "410 body should echo wake_id; got: {}",
            body
        );
    });
}

#[test]
fn error_422_handler_not_found() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (target_id, _target_bearer) = harness.new_identity();
    let (_caller, caller_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Dispatch to a target that hasn't registered any handler →
        // 422 HandlerNotFound. Per §3.3 the body includes `context_id`.
        let payload = b64_encode(b"x");
        let resp = harness
            .dispatch(
                &team_id,
                &caller_bearer,
                &target_id,
                "no-such-context",
                &payload,
                5,
            )
            .await
            .expect("dispatch to unregistered");
        assert_eq!(
            resp.status().as_u16(),
            422,
            "dispatch to unregistered handler should return 422; \
             body: {:?}",
            resp.text().await
        );
        let body: serde_json::Value = resp.json().await.expect("decode 422 body");
        assert!(body.get("error").is_some());
        assert_eq!(
            body.get("context_id").and_then(|v| v.as_str()),
            Some("no-such-context"),
            "422 body should echo context_id per §3.3; got: {}",
            body
        );
    });
}
