//! OPS-NOTIFY-01 — Discord webhook notification proof tests.
//!
//! Proves that:
//! N01: a configured notifier fires on an accepted control action and the
//!      payload reflects authoritative daemon truth.
//! N02: missing webhook config produces a no-op — no delivery attempted,
//!      daemon action result unchanged.
//! N03: Discord delivery failure (bad URL) does not corrupt the primary
//!      daemon action result.
//! N04: the alerts/active GET route does NOT trigger the notifier —
//!      notifications are only emitted from accepted control actions.
//!
//! All four tests run without a DB (pure in-process).
//! No real Discord webhook is contacted.

use std::sync::Arc;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc as StdArc, Mutex as StdMutex,
};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use mqk_daemon::{notify::DiscordNotifier, routes::build_router, state::AppState};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<Body>) -> (StatusCode, Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect")
        .to_bytes();
    (status, body)
}

// ---------------------------------------------------------------------------
// Helper: spin up a tiny in-process webhook sink and return its URL + counters.
// ---------------------------------------------------------------------------

struct WebhookSink {
    url: String,
    call_count: StdArc<AtomicUsize>,
    received: StdArc<StdMutex<Vec<Value>>>,
}

async fn start_webhook_sink() -> WebhookSink {
    let call_count = StdArc::new(AtomicUsize::new(0));
    let received: StdArc<StdMutex<Vec<Value>>> = StdArc::new(StdMutex::new(Vec::new()));

    let cc = call_count.clone();
    let rx = received.clone();

    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |body: axum::Json<Value>| {
            let cc = cc.clone();
            let rx = rx.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                rx.lock().unwrap().push(body.0);
                axum::http::StatusCode::NO_CONTENT
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    WebhookSink {
        url: format!("http://127.0.0.1:{}/hook", addr.port()),
        call_count,
        received,
    }
}

// ---------------------------------------------------------------------------
// N01: configured notifier fires on accepted arm action; payload is truthful
// ---------------------------------------------------------------------------

/// Proves:
/// - an accepted `arm-execution` ops/action triggers a Discord webhook POST
/// - the webhook body contains `action_key="control.arm"`, `disposition="applied"`,
///   and a non-empty `ts_utc`
/// - the primary HTTP response is still 200 / accepted=true
#[tokio::test]
async fn n01_configured_notifier_fires_on_accepted_action_and_payload_is_truthful() {
    let sink = start_webhook_sink().await;

    let mut state = AppState::new();
    state.discord_notifier = DiscordNotifier::from_url(&sink.url);
    let router = build_router(Arc::new(state));

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"action_key": "arm-execution"}"#))
        .unwrap();

    let (status, bytes) = call(router, req).await;
    assert_eq!(status, 200, "N01: arm-execution must return 200");

    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["accepted"], true, "N01: accepted must be true");
    assert_eq!(
        body["disposition"], "applied",
        "N01: disposition must be applied"
    );

    // Give the notifier time to complete the POST to our sink.
    // (notify_operator_action is awaited inline before the response returns,
    // but the sink's axum handler is async — a short wait ensures it's flushed.)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let count = sink.call_count.load(Ordering::SeqCst);
    assert_eq!(count, 1, "N01: webhook must have been called exactly once");

    let received = sink.received.lock().unwrap();
    assert_eq!(
        received.len(),
        1,
        "N01: exactly one payload must be received"
    );

    let payload = &received[0];

    // action_key must reflect the authoritative action
    assert_eq!(
        payload["action_key"].as_str().unwrap(),
        "control.arm",
        "N01: action_key must be control.arm"
    );

    // disposition must be applied
    assert_eq!(
        payload["disposition"].as_str().unwrap(),
        "applied",
        "N01: payload disposition must be applied"
    );

    // ts_utc must be a non-empty RFC 3339 string
    let ts = payload["ts_utc"].as_str().unwrap_or("");
    assert!(!ts.is_empty(), "N01: ts_utc must be present and non-empty");
    assert!(ts.contains('T'), "N01: ts_utc must look like RFC 3339");

    // content must include the action key and disposition (human-readable line)
    let content = payload["content"].as_str().unwrap_or("");
    assert!(
        content.contains("control.arm"),
        "N01: content must reference control.arm; got: {content}"
    );
    assert!(
        content.contains("applied"),
        "N01: content must reference applied disposition; got: {content}"
    );

    // provenance_ref must be null: arm-execution writes only sys_arm_state,
    // not audit_events, and there is no DB in this test — no durable write.
    assert!(
        payload["provenance_ref"].is_null(),
        "N01: provenance_ref must be null for arm-execution (no DB, no audit_events row); got: {:?}",
        payload["provenance_ref"]
    );
}

// ---------------------------------------------------------------------------
// N05: /control/arm provenance_ref matches exact durable audit_events UUID (DB-backed)
// ---------------------------------------------------------------------------

/// Proves:
/// - POST /control/arm with a real DB and a seeded run anchor fires the Discord notifier
/// - the payload `provenance_ref` is `"audit_events:<uuid>"` — not null, not a generic label
/// - the UUID in provenance_ref matches the `audit_event_id` returned in the HTTP response
/// - that UUID exists in the audit_events table (end-to-end durable linkage)
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn n05_control_arm_provenance_ref_matches_exact_durable_audit_events_uuid() {
    let db_url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => {
            eprintln!("N05: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("N05: connect failed");

    // Seed a run with engine_id='mqk-daemon' so write_control_operator_audit_event
    // can resolve a run_id anchor via fetch_latest_run_for_engine.
    let run_id = uuid::Uuid::parse_str("cc000005-0000-4000-8000-000000000099").unwrap();
    let started_at = chrono::DateTime::parse_from_rfc3339("2020-01-05T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Pre-test cleanup.
    sqlx::query("delete from audit_events where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("N05: pre-test audit_events cleanup failed");
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("N05: pre-test runs cleanup failed");

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: started_at,
            git_hash: "n05-test-hash".to_string(),
            config_hash: "n05-config-hash".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "n05-test-host".to_string(),
        },
    )
    .await
    .expect("N05: insert_run failed");

    // Build state with DB + a Discord sink.
    let sink = start_webhook_sink().await;
    let mut st = mqk_daemon::state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
    );
    st.discord_notifier = mqk_daemon::notify::DiscordNotifier::from_url(&sink.url);
    let router = mqk_daemon::routes::build_router(Arc::new(st));

    let req = Request::builder()
        .method("POST")
        .uri("/control/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body_bytes) = call(router, req).await;
    assert_eq!(
        status,
        200,
        "N05: /control/arm must return 200; body: {}",
        String::from_utf8_lossy(&body_bytes)
    );

    let arm_json: Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(arm_json["accepted"], true, "N05: arm must be accepted");

    // The response must have a non-null audit_event_id.
    let event_id_str = arm_json["audit"]["audit_event_id"]
        .as_str()
        .expect("N05: audit_event_id must be non-null when run anchor exists");

    // Give the async webhook sink time to flush.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let count = sink.call_count.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(count, 1, "N05: webhook must be called exactly once");

    let received = sink.received.lock().unwrap();
    let payload = &received[0];

    // provenance_ref must be "audit_events:<event_id_str>" — exact durable linkage.
    let expected_prov = format!("audit_events:{}", event_id_str);
    assert_eq!(
        payload["provenance_ref"].as_str().unwrap_or(""),
        expected_prov,
        "N05: provenance_ref must be exact audit_events UUID reference; got: {:?}",
        payload["provenance_ref"]
    );

    drop(received);

    // Verify the UUID actually exists in the audit_events table.
    let event_uuid =
        uuid::Uuid::parse_str(event_id_str).expect("N05: event_id is not a valid UUID");
    let row: (bool,) = sqlx::query_as("select count(*) > 0 from audit_events where event_id = $1")
        .bind(event_uuid)
        .fetch_one(&pool)
        .await
        .expect("N05: DB existence check failed");
    assert!(
        row.0,
        "N05: audit_events row with event_id={event_id_str} must exist in DB"
    );
}

// ---------------------------------------------------------------------------
// N02: missing webhook config — no delivery attempted, action result unchanged
// ---------------------------------------------------------------------------

/// Proves:
/// - `AppState::new()` without `DISCORD_WEBHOOK_URL` env produces a no-op notifier
/// - the arm-execution action still succeeds with the same 200/applied result
/// - no delivery is attempted (is_configured() == false is structural proof)
#[tokio::test]
async fn n02_missing_config_produces_noop_action_result_unchanged() {
    // AppState::new() reads DISCORD_WEBHOOK_URL from env; in CI/test it is absent.
    let state = AppState::new();

    // Structural proof: no delivery will be attempted.
    assert!(
        !state.discord_notifier.is_configured(),
        "N02: default AppState must have unconfigured notifier (DISCORD_WEBHOOK_URL absent)"
    );

    let router = build_router(Arc::new(state));

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"action_key": "arm-execution"}"#))
        .unwrap();

    let (status, bytes) = call(router, req).await;
    assert_eq!(status, 200, "N02: arm-execution must still return 200");

    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["accepted"], true, "N02: accepted must be true");
    assert_eq!(
        body["disposition"], "applied",
        "N02: disposition must be applied"
    );
}

// ---------------------------------------------------------------------------
// N03: delivery failure does not corrupt primary daemon result
// ---------------------------------------------------------------------------

/// Proves:
/// - a notifier configured with an unreachable URL (connection refused) does
///   not propagate an error to the HTTP handler
/// - the primary arm-execution result is still 200 / accepted=true / applied
#[tokio::test]
async fn n03_delivery_failure_does_not_corrupt_primary_result() {
    // Port 1 is reserved and will refuse connections on all platforms.
    let bad_url = "http://127.0.0.1:1/hook";
    let mut state = AppState::new();
    state.discord_notifier = DiscordNotifier::from_url(bad_url);

    assert!(
        state.discord_notifier.is_configured(),
        "N03: notifier must be configured to exercise failure path"
    );

    let router = build_router(Arc::new(state));

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"action_key": "arm-execution"}"#))
        .unwrap();

    let (status, bytes) = call(router, req).await;

    // The daemon action must still succeed despite delivery failure.
    assert_eq!(
        status, 200,
        "N03: arm-execution must still return 200 even when Discord delivery fails"
    );

    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["accepted"], true,
        "N03: accepted must still be true after delivery failure"
    );
    assert_eq!(
        body["disposition"], "applied",
        "N03: disposition must still be applied after delivery failure"
    );
}

// ---------------------------------------------------------------------------
// N04: alerts/active GET route does NOT trigger the notifier
// ---------------------------------------------------------------------------

/// Proves:
/// - the GET /api/v1/alerts/active route returns 200 with clean-state empty rows
/// - the notifier is NOT called from a read-only GET route
///   (notifications are only emitted from accepted POST control actions)
#[tokio::test]
async fn n04_alert_get_route_does_not_trigger_notifier() {
    let sink = start_webhook_sink().await;

    let mut state = AppState::new();
    state.discord_notifier = DiscordNotifier::from_url(&sink.url);
    let router = build_router(Arc::new(state));

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/alerts/active")
        .body(Body::empty())
        .unwrap();

    let (status, bytes) = call(router, req).await;
    assert_eq!(status, 200, "N04: alerts/active must return 200");

    let body: Value = serde_json::from_slice(&bytes).unwrap();

    // Confirm truth_state = active and empty rows (clean daemon state).
    assert_eq!(
        body["truth_state"].as_str().unwrap(),
        "active",
        "N04: truth_state must be active"
    );
    let rows = body["rows"].as_array().unwrap();
    assert!(
        rows.is_empty(),
        "N04: clean state must produce zero alert rows"
    );

    // Give async any pending work time to flush.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let count = sink.call_count.load(Ordering::SeqCst);
    assert_eq!(
        count, 0,
        "N04: webhook must NOT be called from a GET alerts/active request"
    );
}
