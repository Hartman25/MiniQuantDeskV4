//! OBS-SESSION-DISCORD-01 — Discord test-alert route proof.
//!
//! Proves that `POST /api/v1/ops/action {"action_key":"test-discord-alert"}` is:
//! - Safe: never mutates trading, arm, runtime, or integrity state.
//! - Redacted: webhook URL never appears in response body.
//! - Correct: accepted=true in all cases (configured or not).
//! - Separated: noop_unconfigured disposition when notifier is not set up.
//!
//! | Test   | Claim                                                                            |
//! |--------|----------------------------------------------------------------------------------|
//! | DT-01  | Unconfigured notifier → accepted=true, disposition=noop_unconfigured, warning    |
//! | DT-02  | Configured notifier + local sink → delivery received, payload has "[TEST]" marker |
//! | DT-03  | Webhook URL is never present in the route response body                          |
//! | DT-04  | Action does not mutate arm state or integrity                                    |
//! | DT-05  | Payload construction: TestAlertPayload serialises correctly (no network needed)  |
//!
//! DT-01/DT-03/DT-04/DT-05 are pure in-process tests (no network).
//! DT-02 uses an in-process HTTP sink (no real Discord call).

use std::sync::Arc;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc as StdArc, Mutex as StdMutex,
};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use bytes::Bytes;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use mqk_daemon::{
    notify::{DiscordNotifier, TestAlertPayload},
    routes::build_router,
    state::AppState,
};

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

fn parse_json(b: &Bytes) -> Value {
    serde_json::from_slice(b).expect("body is not valid JSON")
}

fn ops_action_req(action_key: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(Body::from(format!(r#"{{"action_key":"{}"}}"#, action_key)))
        .unwrap()
}

// In-process webhook sink — captures all JSON POSTs.
struct WebhookSink {
    url: String,
    received: StdArc<StdMutex<Vec<Value>>>,
}

async fn start_webhook_sink() -> WebhookSink {
    let received: StdArc<StdMutex<Vec<Value>>> = StdArc::new(StdMutex::new(Vec::new()));
    let rx = received.clone();
    let counter = StdArc::new(AtomicUsize::new(0));
    let cc = counter.clone();

    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |body: axum::Json<Value>| {
            let rx = rx.clone();
            let cc = cc.clone();
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
        received,
    }
}

// ---------------------------------------------------------------------------
// DT-01: Unconfigured notifier → noop_unconfigured
// ---------------------------------------------------------------------------

/// Proves:
/// - `test-discord-alert` with a noop notifier returns 200 + accepted=true.
/// - disposition is "noop_unconfigured".
/// - A warning is present telling the operator how to enable delivery.
/// - No panic; no state mutation.
#[tokio::test]
async fn dt01_unconfigured_notifier_returns_noop_unconfigured() {
    let mut state = AppState::new();
    state.discord_notifier = DiscordNotifier::noop();
    let router = build_router(Arc::new(state));

    let (status, body) = call(router, ops_action_req("test-discord-alert")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "DT-01: test-discord-alert must return 200 even when notifier is unconfigured"
    );
    let v = parse_json(&body);
    assert_eq!(
        v["accepted"], true,
        "DT-01: accepted must be true for test-discord-alert"
    );
    assert_eq!(
        v["disposition"], "noop_unconfigured",
        "DT-01: unconfigured notifier must yield disposition=noop_unconfigured; got: {}",
        v["disposition"]
    );
    let warnings = v["warnings"].as_array().expect("warnings must be array");
    assert!(
        !warnings.is_empty(),
        "DT-01: at least one warning must explain how to enable Discord delivery"
    );
    let first_warn = warnings[0].as_str().unwrap_or("");
    // DISCORD-CHANNEL-ROUTING-01: test-discord-alert routes to the `alerts`
    // channel specifically, so the warning names that channel's canonical
    // env var (DISCORD_WEBHOOK_ALERTS), not the retired flat DISCORD_WEBHOOK_URL.
    assert!(
        first_warn.contains("DISCORD_WEBHOOK_ALERTS"),
        "DT-01: warning must mention DISCORD_WEBHOOK_ALERTS; got: {first_warn}"
    );
}

// ---------------------------------------------------------------------------
// DT-02: Configured notifier + local sink → delivery received, "[TEST]" marker
// ---------------------------------------------------------------------------

/// Proves:
/// - With a local webhook sink wired in, `test-discord-alert` POSTs a payload.
/// - The payload's `content` field contains the "[TEST]" marker.
/// - The payload's `alert_type` field is "test".
/// - disposition is "delivery_attempted" (sink responded OK).
#[tokio::test]
async fn dt02_configured_notifier_delivers_test_payload_to_sink() {
    let sink = start_webhook_sink().await;

    let mut state = AppState::new();
    state.discord_notifier = DiscordNotifier::from_url(&sink.url);
    let router = build_router(Arc::new(state));

    let (status, body) = call(router, ops_action_req("test-discord-alert")).await;
    assert_eq!(status, StatusCode::OK, "DT-02: must return 200");
    let v = parse_json(&body);
    assert_eq!(v["accepted"], true, "DT-02: accepted must be true");
    assert_eq!(
        v["disposition"], "delivery_attempted",
        "DT-02: delivery to local sink must yield disposition=delivery_attempted; got: {}",
        v["disposition"]
    );

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let received = sink.received.lock().unwrap();
    assert_eq!(
        received.len(),
        1,
        "DT-02: exactly one payload must be delivered to the sink; got {}",
        received.len()
    );
    let payload = &received[0];

    let content = payload["content"].as_str().unwrap_or("");
    assert!(
        content.contains("[TEST]"),
        "DT-02: content must contain '[TEST]' marker; got: {content}"
    );
    assert_eq!(
        payload["alert_type"].as_str().unwrap_or(""),
        "test",
        "DT-02: alert_type must be 'test'"
    );
    let note = payload["note"].as_str().unwrap_or("");
    assert!(
        note.contains("no trading state affected"),
        "DT-02: note must clarify that no trading state is affected; got: {note}"
    );
}

// ---------------------------------------------------------------------------
// DT-03: Webhook URL is never present in the route response body
// ---------------------------------------------------------------------------

/// Proves that even when delivery is attempted, the webhook URL is not present
/// in any field of the JSON response (redaction enforcement).
#[tokio::test]
async fn dt03_webhook_url_is_redacted_from_response_body() {
    let sink = start_webhook_sink().await;

    let mut state = AppState::new();
    state.discord_notifier = DiscordNotifier::from_url(&sink.url);
    let router = build_router(Arc::new(state));

    let (status, body) = call(router, ops_action_req("test-discord-alert")).await;
    assert_eq!(status, StatusCode::OK);

    // The response body must not contain the URL or any fragment of it.
    let body_str = std::str::from_utf8(&body).expect("body must be valid UTF-8");
    assert!(
        !body_str.contains("127.0.0.1"),
        "DT-03: webhook host must not appear in response body; got: {body_str}"
    );
    assert!(
        !body_str.contains("/hook"),
        "DT-03: webhook path must not appear in response body; got: {body_str}"
    );
}

// ---------------------------------------------------------------------------
// DT-04: Action does not mutate arm state, run state, or integrity
// ---------------------------------------------------------------------------

/// Proves:
/// - After test-discord-alert, the arm state (disarmed flag) is unchanged.
/// - After test-discord-alert, locally_owned_run_id() is still None.
/// - No panic; consistent state before and after.
#[tokio::test]
async fn dt04_test_discord_alert_does_not_mutate_state() {
    let sink = start_webhook_sink().await;

    let mut state = AppState::new();
    state.discord_notifier = DiscordNotifier::from_url(&sink.url);
    let state = Arc::new(state);

    // Capture integrity state before.
    let arm_before = {
        let ig = state.integrity.read().await;
        ig.disarmed
    };
    let run_before = state.locally_owned_run_id().await;

    let router = build_router(Arc::clone(&state));
    let (status, _) = call(router, ops_action_req("test-discord-alert")).await;
    assert_eq!(status, StatusCode::OK);

    // State must be identical after.
    let arm_after = {
        let ig = state.integrity.read().await;
        ig.disarmed
    };
    let run_after = state.locally_owned_run_id().await;

    assert_eq!(
        arm_before, arm_after,
        "DT-04: integrity.disarmed must be unchanged after test-discord-alert"
    );
    assert_eq!(
        run_before, run_after,
        "DT-04: locally_owned_run_id must be unchanged after test-discord-alert"
    );
}

// ---------------------------------------------------------------------------
// DT-05: TestAlertPayload serialises correctly (pure, no network)
// ---------------------------------------------------------------------------

/// Proves that `TestAlertPayload` round-trips through serde without data loss.
/// Also verifies the alert_type field and content format of `notify_test_alert`
/// by inspecting the sink payload in isolation.
#[tokio::test]
async fn dt05_test_alert_payload_structure_is_correct() {
    let payload = TestAlertPayload {
        environment: Some("paper".to_string()),
        ts_utc: "2026-04-29T14:30:00Z".to_string(),
        note: "operator test via test-discord-alert route; no trading state affected".to_string(),
    };

    let json_str = serde_json::to_string(&payload).expect("must serialise");
    let back: TestAlertPayload = serde_json::from_str(&json_str).expect("must deserialise");

    assert_eq!(
        back.environment.as_deref(),
        Some("paper"),
        "DT-05: environment must round-trip"
    );
    assert_eq!(
        back.ts_utc, "2026-04-29T14:30:00Z",
        "DT-05: ts_utc must round-trip"
    );
    assert!(
        back.note.contains("no trading state affected"),
        "DT-05: note must contain the safety marker"
    );

    // Also verify via the notifier itself (with an in-process sink).
    let sink = start_webhook_sink().await;
    let notifier = DiscordNotifier::from_url(&sink.url);
    let delivered = notifier.notify_test_alert(&payload).await;
    assert!(delivered, "DT-05: delivery to local sink must succeed");

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    let received = sink.received.lock().unwrap();
    assert_eq!(received.len(), 1, "DT-05: exactly one payload delivered");
    let p = &received[0];
    assert_eq!(
        p["alert_type"].as_str().unwrap_or(""),
        "test",
        "DT-05: alert_type must be 'test' in delivered payload"
    );
    let content = p["content"].as_str().unwrap_or("");
    assert!(
        content.contains("[TEST]"),
        "DT-05: content must contain '[TEST]' marker"
    );
    assert!(
        content.contains("paper"),
        "DT-05: content must reference the environment"
    );
}
