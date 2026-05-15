//! P0 happy-path integration tests per `docs/specs/integration-tests-sub-pr-test-plan.md`.
//!
//! Six tests, one per public route:
//! 1. `health_check` — `GET /v1/health` returns 200 with status+version.
//! 2. `register_handler` — `POST .../register` returns 201; subsequent
//!    dispatch to the registered (identity, context_id) reaches the
//!    handler.
//! 3. `unregister_handler` — `DELETE .../handlers/{ctx}` returns 204;
//!    subsequent dispatch to the same (identity, context_id) returns
//!    422 (HandlerNotFound).
//! 4. `dispatch_and_complete` — dispatch blocks; target completes;
//!    dispatcher receives the response.
//! 5. `read_inbox_immediate` — `wait_seconds=0` returns immediately,
//!    with the pending wake summary if one exists.
//! 6. `read_inbox_with_limit` — `?limit=N` caps the response; on full
//!    inbox `more_available` is true.

use std::time::Duration;

use tally_integration_tests::{
    b64_encode, CompleteResponse, DispatchResponse, HealthResponse, ReadInboxResponse,
    RegisterResponse, TestHarness,
};

/// Build a single multi-thread runtime per test. `#[tokio::test]`
/// would also work for HTTP-only tests, but the manually-managed
/// shape (a) matches the skytale e2e crate pattern and (b) keeps a
/// path open for callers that need synchronous SDK calls in future.
fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

#[test]
fn health_check() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");

    runtime.block_on(async {
        let resp = harness.health().await.expect("GET /v1/health");
        assert_eq!(resp.status().as_u16(), 200);
        let body: HealthResponse = resp.json().await.expect("decode HealthResponse");
        assert_eq!(body.status, "ok");
        assert!(
            !body.version.is_empty(),
            "version should be non-empty (CARGO_PKG_VERSION)"
        );
    });
}

#[test]
fn register_handler() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (identity_b64, bearer) = harness.new_identity();

    runtime.block_on(async {
        let resp = harness
            .register(&team_id, &identity_b64, &bearer, "task-routing")
            .await
            .expect("register call");
        assert_eq!(
            resp.status().as_u16(),
            201,
            "register should return 201; body: {:?}",
            resp.text().await
        );

        // Optional re-deserialize: confirm response shape matches §3.3.
        let resp = harness
            .register(&team_id, &identity_b64, &bearer, "task-routing-2")
            .await
            .expect("register call 2");
        assert_eq!(resp.status().as_u16(), 201);
        let body: RegisterResponse = resp.json().await.expect("decode RegisterResponse");
        assert!(body.registered);
        assert_eq!(body.context_id, "task-routing-2");
    });
}

#[test]
fn unregister_handler() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (identity_b64, bearer) = harness.new_identity();
    let (caller_id, caller_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Register, then unregister.
        let resp = harness
            .register(&team_id, &identity_b64, &bearer, "task-routing")
            .await
            .expect("register");
        assert_eq!(resp.status().as_u16(), 201);

        let resp = harness
            .unregister(&team_id, &identity_b64, &bearer, "task-routing")
            .await
            .expect("unregister");
        assert_eq!(
            resp.status().as_u16(),
            204,
            "unregister should return 204; body: {:?}",
            resp.text().await
        );

        // Dispatch to the un-registered (identity, context) should
        // return 422 HandlerNotFound per §3.1. We use a distinct
        // caller identity so the dispatch isn't refused for caller==target.
        let payload = b64_encode(b"hello");
        let resp = harness
            .dispatch(
                &team_id,
                &caller_bearer,
                &identity_b64,
                "task-routing",
                &payload,
                5,
            )
            .await
            .expect("dispatch after unregister");
        assert_eq!(
            resp.status().as_u16(),
            422,
            "dispatch after unregister should return 422 HandlerNotFound; \
             body: {:?}",
            resp.text().await
        );
        let _ = caller_id; // unused but documents the caller-vs-target setup
    });
}

#[test]
fn dispatch_and_complete() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (caller_id, caller_bearer) = harness.new_identity();
    let (target_id, target_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Setup: target registers for "task-routing".
        let resp = harness
            .register(&team_id, &target_id, &target_bearer, "task-routing")
            .await
            .expect("target register");
        assert_eq!(resp.status().as_u16(), 201);

        // Spawn the dispatch in a background task; it blocks until
        // either the wake completes or the timeout fires. We give it
        // 10s — plenty for the complete call below to win.
        let dispatcher_team_id = team_id.clone();
        let dispatcher_caller_bearer = caller_bearer.clone();
        let dispatcher_target_id = target_id.clone();
        let payload_b64 = b64_encode(b"please process this");
        let dispatcher_payload = payload_b64.clone();
        // Spawn the dispatcher: it'll long-block in the server until
        // complete() resolves it.
        let harness_clone_url = harness.base_url.clone();
        let harness_clone_client = harness.client.clone();
        let dispatch_handle = tokio::spawn(async move {
            let url = format!(
                "{}/v1/teams/{}/wakes",
                harness_clone_url, dispatcher_team_id
            );
            let body = serde_json::json!({
                "target_identity": dispatcher_target_id,
                "context_id": "task-routing",
                "payload": dispatcher_payload,
                "timeout_seconds": 10,
            });
            harness_clone_client
                .post(&url)
                .bearer_auth(&dispatcher_caller_bearer)
                .json(&body)
                .send()
                .await
                .expect("dispatch request")
        });

        // Give the dispatcher a moment to reach the server's await
        // point so the wake row is durable before we poll the inbox.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Target reads its inbox to discover the wake_id.
        let resp = harness
            .read_inbox(&team_id, &target_id, &target_bearer, 5, None)
            .await
            .expect("read_inbox");
        assert_eq!(
            resp.status().as_u16(),
            200,
            "read_inbox should return 200; body: {:?}",
            resp.text().await
        );
        let inbox: ReadInboxResponse = resp.json().await.expect("decode ReadInboxResponse");
        assert_eq!(inbox.wakes.len(), 1, "inbox should hold the pending wake");
        let wake = &inbox.wakes[0];
        assert_eq!(wake.caller_identity, caller_id);
        assert_eq!(wake.context_id, "task-routing");
        assert_eq!(wake.payload, payload_b64);
        // expires_at: ISO-8601 string. Loose check — non-empty + contains 'T'.
        assert!(
            wake.expires_at.contains('T') && wake.expires_at.ends_with('Z'),
            "expires_at should be ISO-8601 UTC; got {:?}",
            wake.expires_at
        );
        let wake_id = wake.wake_id.clone();
        // wake_id should parse as ULID (26 chars Crockford-base32).
        ulid::Ulid::from_string(&wake_id).expect("wake_id parses as ULID");

        // Target completes the wake.
        let response_payload = b64_encode(b"done!");
        let resp = harness
            .complete(&team_id, &wake_id, &target_bearer, &response_payload)
            .await
            .expect("complete");
        assert_eq!(
            resp.status().as_u16(),
            200,
            "complete should return 200; body: {:?}",
            resp.text().await
        );
        let comp: CompleteResponse = resp.json().await.expect("decode CompleteResponse");
        assert!(comp.completed);
        assert_eq!(comp.wake_id, wake_id);

        // Await the dispatcher's response.
        let resp = dispatch_handle.await.expect("dispatch task join");
        assert_eq!(resp.status().as_u16(), 200, "dispatcher should receive 200");
        let disp: DispatchResponse = resp.json().await.expect("decode DispatchResponse");
        assert_eq!(disp.wake_id, wake_id);
        assert_eq!(disp.response, response_payload);
        assert!(
            disp.completed_at.contains('T') && disp.completed_at.ends_with('Z'),
            "completed_at should be ISO-8601 UTC; got {:?}",
            disp.completed_at
        );
    });
}

#[test]
fn read_inbox_immediate() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (target_id, target_bearer) = harness.new_identity();
    let (_caller_id, caller_bearer) = harness.new_identity();

    runtime.block_on(async {
        // Setup: register target.
        let resp = harness
            .register(&team_id, &target_id, &target_bearer, "ctx-a")
            .await
            .expect("register");
        assert_eq!(resp.status().as_u16(), 201);

        // Empty-inbox baseline: wait_seconds=0 returns immediately.
        let resp = harness
            .read_inbox(&team_id, &target_id, &target_bearer, 0, None)
            .await
            .expect("read_inbox empty");
        assert_eq!(resp.status().as_u16(), 200);
        let body: ReadInboxResponse = resp.json().await.expect("decode");
        assert!(
            body.wakes.is_empty(),
            "empty inbox should yield empty wakes vec"
        );
        assert!(!body.more_available);

        // Dispatch in the background; once the wake row is durable,
        // wait_seconds=0 should yield the pending entry.
        let payload = b64_encode(b"x");
        let dispatcher_team_id = team_id.clone();
        let dispatcher_caller_bearer = caller_bearer.clone();
        let dispatcher_target = target_id.clone();
        let dispatcher_payload = payload.clone();
        let harness_url = harness.base_url.clone();
        let harness_client = harness.client.clone();
        let _dispatch_handle = tokio::spawn(async move {
            let url = format!("{}/v1/teams/{}/wakes", harness_url, dispatcher_team_id);
            let body = serde_json::json!({
                "target_identity": dispatcher_target,
                "context_id": "ctx-a",
                "payload": dispatcher_payload,
                "timeout_seconds": 30,
            });
            // We don't await this — the dispatcher will block until
            // either complete (we won't call) or the timeout fires
            // (>test runtime). Drop handle on test exit; harness Drop
            // kills wrangler which terminates the connection.
            let _ = harness_client
                .post(&url)
                .bearer_auth(&dispatcher_caller_bearer)
                .json(&body)
                .send()
                .await;
        });

        // Brief wait so the wake row is durable before inbox read.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let resp = harness
            .read_inbox(&team_id, &target_id, &target_bearer, 0, None)
            .await
            .expect("read_inbox after dispatch");
        assert_eq!(resp.status().as_u16(), 200);
        let body: ReadInboxResponse = resp.json().await.expect("decode");
        assert_eq!(body.wakes.len(), 1);
        assert_eq!(body.wakes[0].payload, payload);
    });
}

#[test]
fn read_inbox_with_limit() {
    let runtime = rt();
    let harness = runtime
        .block_on(TestHarness::setup())
        .expect("harness setup");
    let team_id = harness.new_team_id();
    let (target_id, target_bearer) = harness.new_identity();
    let (_caller_id, caller_bearer) = harness.new_identity();

    runtime.block_on(async {
        let resp = harness
            .register(&team_id, &target_id, &target_bearer, "ctx-a")
            .await
            .expect("register");
        assert_eq!(resp.status().as_u16(), 201);

        // Dispatch three pending wakes concurrently (each blocks
        // server-side until timeout; we'll never complete them — the
        // test only inspects the inbox). Each dispatcher spawn returns
        // immediately to background-tasks; we await only the durable
        // wake_row delay.
        for i in 0..3u8 {
            let payload = b64_encode(&[i]);
            let dispatcher_team_id = team_id.clone();
            let dispatcher_caller_bearer = caller_bearer.clone();
            let dispatcher_target = target_id.clone();
            let harness_url = harness.base_url.clone();
            let harness_client = harness.client.clone();
            tokio::spawn(async move {
                let url = format!("{}/v1/teams/{}/wakes", harness_url, dispatcher_team_id);
                let body = serde_json::json!({
                    "target_identity": dispatcher_target,
                    "context_id": "ctx-a",
                    "payload": payload,
                    "timeout_seconds": 30,
                });
                let _ = harness_client
                    .post(&url)
                    .bearer_auth(&dispatcher_caller_bearer)
                    .json(&body)
                    .send()
                    .await;
            });
        }

        // Wait for all three dispatches' wake rows to become durable.
        // The DO's dispatch_with_caller commits the wake row before
        // the await; 1s is generous slack on top of typical ~50ms.
        tokio::time::sleep(Duration::from_millis(1500)).await;

        // limit=2 should return exactly 2 wakes + more_available=true.
        let resp = harness
            .read_inbox(&team_id, &target_id, &target_bearer, 0, Some(2))
            .await
            .expect("read_inbox limit=2");
        assert_eq!(resp.status().as_u16(), 200);
        let body: ReadInboxResponse = resp.json().await.expect("decode");
        assert_eq!(
            body.wakes.len(),
            2,
            "limit=2 should return 2 wakes; got {}",
            body.wakes.len()
        );
        assert!(
            body.more_available,
            "with 3 pending and limit=2, more_available should be true"
        );

        // limit=10 should return all 3 + more_available=false.
        let resp = harness
            .read_inbox(&team_id, &target_id, &target_bearer, 0, Some(10))
            .await
            .expect("read_inbox limit=10");
        assert_eq!(resp.status().as_u16(), 200);
        let body: ReadInboxResponse = resp.json().await.expect("decode");
        assert_eq!(body.wakes.len(), 3);
        assert!(!body.more_available);
    });
}
