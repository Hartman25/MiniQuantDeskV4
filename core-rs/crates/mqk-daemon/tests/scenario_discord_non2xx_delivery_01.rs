//! DISCORD-NON2XX-DELIVERY-CLASSIFICATION-01 — non-2xx Discord delivery proof.
//!
//! Proves:
//! ND01: HTTP 200 OK is treated as delivery success.
//! ND02: HTTP 204 No Content is treated as delivery success.
//! ND03-ND08: HTTP 400/401/403/404/429/500 are each classified as a
//!            non-fatal, sanitized delivery failure (no panic, no block).
//! ND09: `discord_status_class` buckets status codes into sanitized classes.
//! ND10: every `notify_*` method completes quickly without panicking when the
//!       webhook responds non-2xx.
//! ND11: the sanitized status summary cannot carry webhook URL, token,
//!       Discord webhook path, or response body content.
//! ND12: a missing webhook remains a silent no-op.
//!
//! All webhook strings in this file are fake fixtures pointing at an
//! in-process axum sink on 127.0.0.1; no real Discord endpoint is contacted.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::StatusCode;

use mqk_daemon::notify::{
    discord_delivery_status_summary, discord_status_class, CriticalAlertPayload, DiscordNotifier,
    OperatorNotifyPayload, RunStatusPayload, TestAlertPayload, TradeEventPayload,
};

const FAKE_WEBHOOK_PATH: &str = "/discord.com/api/webhooks/fake-webhook-id/fake-webhook-token";
const FAKE_WEBHOOK_TOKEN: &str = "fake-webhook-token";
const DISCORD_WEBHOOK_PATH: &str = "discord.com/api/webhooks";
const SECRET_RESPONSE_BODY: &str = "super-secret-response-body-do-not-log";

// ---------------------------------------------------------------------------
// In-process configurable sink — responds with a fixed status + body.
// ---------------------------------------------------------------------------

struct ConfigurableSink {
    url: String,
    request_count: Arc<AtomicUsize>,
}

async fn start_configurable_sink(status: StatusCode, body: &'static str) -> ConfigurableSink {
    let request_count = Arc::new(AtomicUsize::new(0));
    let rc = request_count.clone();

    let app = axum::Router::new().route(
        FAKE_WEBHOOK_PATH,
        axum::routing::post(move |_body: axum::body::Bytes| {
            let rc = rc.clone();
            async move {
                rc.fetch_add(1, Ordering::SeqCst);
                (status, body)
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    ConfigurableSink {
        url: format!("http://127.0.0.1:{}{FAKE_WEBHOOK_PATH}", addr.port()),
        request_count,
    }
}

fn test_alert_payload() -> TestAlertPayload {
    TestAlertPayload {
        environment: Some("paper".to_string()),
        ts_utc: "2026-06-09T00:00:00Z".to_string(),
        note: "operator test; no trading state affected".to_string(),
    }
}

// ---------------------------------------------------------------------------
// ND01/ND02: 2xx is success
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd01_200_ok_is_treated_as_success() {
    let sink = start_configurable_sink(StatusCode::OK, "ok").await;
    let notifier = DiscordNotifier::from_url(&sink.url);

    let delivered = notifier.notify_test_alert(&test_alert_payload()).await;
    assert!(
        delivered,
        "ND01: 200 OK must be treated as delivery success"
    );
    assert_eq!(sink.request_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nd02_204_no_content_is_treated_as_success() {
    let sink = start_configurable_sink(StatusCode::NO_CONTENT, "").await;
    let notifier = DiscordNotifier::from_url(&sink.url);

    let delivered = notifier.notify_test_alert(&test_alert_payload()).await;
    assert!(
        delivered,
        "ND02: 204 No Content must be treated as delivery success"
    );
    assert_eq!(sink.request_count.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// ND03-ND08: non-2xx is a non-fatal, sanitized failure
// ---------------------------------------------------------------------------

async fn assert_non2xx_is_non_fatal_failure(status: StatusCode) {
    let sink = start_configurable_sink(status, SECRET_RESPONSE_BODY).await;
    let notifier = DiscordNotifier::from_url(&sink.url);

    let start = Instant::now();
    let delivered = notifier.notify_test_alert(&test_alert_payload()).await;
    let elapsed = start.elapsed();

    assert!(
        !delivered,
        "non-2xx status {status} must be classified as a delivery failure"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "non-2xx handling must not block the caller; took {elapsed:?}"
    );
    assert_eq!(sink.request_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nd03_400_bad_request_is_non_fatal_failure() {
    assert_non2xx_is_non_fatal_failure(StatusCode::BAD_REQUEST).await;
}

#[tokio::test]
async fn nd04_401_unauthorized_is_non_fatal_failure() {
    assert_non2xx_is_non_fatal_failure(StatusCode::UNAUTHORIZED).await;
}

#[tokio::test]
async fn nd05_403_forbidden_is_non_fatal_failure() {
    assert_non2xx_is_non_fatal_failure(StatusCode::FORBIDDEN).await;
}

#[tokio::test]
async fn nd06_404_not_found_is_non_fatal_failure() {
    assert_non2xx_is_non_fatal_failure(StatusCode::NOT_FOUND).await;
}

#[tokio::test]
async fn nd07_429_too_many_requests_is_non_fatal_failure() {
    assert_non2xx_is_non_fatal_failure(StatusCode::TOO_MANY_REQUESTS).await;
}

#[tokio::test]
async fn nd08_500_internal_server_error_is_non_fatal_failure() {
    assert_non2xx_is_non_fatal_failure(StatusCode::INTERNAL_SERVER_ERROR).await;
}

// ---------------------------------------------------------------------------
// ND09: discord_status_class buckets
// ---------------------------------------------------------------------------

#[test]
fn nd09_status_class_buckets_are_sanitized_and_correct() {
    assert_eq!(
        discord_status_class(StatusCode::MULTIPLE_CHOICES),
        "redirection"
    );
    assert_eq!(
        discord_status_class(StatusCode::BAD_REQUEST),
        "client_error"
    );
    assert_eq!(
        discord_status_class(StatusCode::UNAUTHORIZED),
        "client_error"
    );
    assert_eq!(discord_status_class(StatusCode::FORBIDDEN), "client_error");
    assert_eq!(discord_status_class(StatusCode::NOT_FOUND), "client_error");
    assert_eq!(
        discord_status_class(StatusCode::TOO_MANY_REQUESTS),
        "client_error"
    );
    assert_eq!(
        discord_status_class(StatusCode::INTERNAL_SERVER_ERROR),
        "server_error"
    );

    let summary = discord_delivery_status_summary(StatusCode::BAD_REQUEST);
    assert_eq!(summary.status_code, 400);
    assert_eq!(summary.status_class, "client_error");
}

// ---------------------------------------------------------------------------
// ND10: every notify_* method survives a non-2xx response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd10_all_notify_methods_survive_non2xx_without_blocking() {
    let sink =
        start_configurable_sink(StatusCode::INTERNAL_SERVER_ERROR, SECRET_RESPONSE_BODY).await;
    let notifier = DiscordNotifier::from_url(&sink.url);

    let start = Instant::now();

    notifier
        .notify_operator_action(&OperatorNotifyPayload {
            action_key: "control.arm".to_string(),
            disposition: "applied".to_string(),
            environment: Some("paper".to_string()),
            ts_utc: "2026-06-09T00:00:01Z".to_string(),
            provenance_ref: Some("audit:test".to_string()),
            run_id: None,
        })
        .await;

    notifier
        .notify_critical_alert(&CriticalAlertPayload {
            alert_class: "paper.ws_continuity.gap_detected".to_string(),
            severity: "critical".to_string(),
            summary: "gap detected".to_string(),
            detail: None,
            environment: Some("paper".to_string()),
            run_id: None,
            ts_utc: "2026-06-09T00:00:02Z".to_string(),
        })
        .await;

    notifier
        .notify_run_status(&RunStatusPayload {
            event: "run.halted".to_string(),
            run_id: None,
            environment: Some("paper".to_string()),
            note: Some("test halt".to_string()),
            ts_utc: "2026-06-09T00:00:03Z".to_string(),
        })
        .await;

    notifier
        .notify_trade_event(&TradeEventPayload {
            stage: "order.submitted".to_string(),
            run_id: Some("run-test".to_string()),
            symbol: Some("AAPL".to_string()),
            side: Some("Buy".to_string()),
            qty: Some(1),
            price_micros: None,
            order_id: Some("order-test".to_string()),
            detail: None,
            environment: Some("paper".to_string()),
            summary: "order submitted".to_string(),
            ts_utc: "2026-06-09T00:00:04Z".to_string(),
        })
        .await;

    let delivered = notifier.notify_test_alert(&test_alert_payload()).await;

    let elapsed = start.elapsed();

    assert!(
        !delivered,
        "ND10: 500 response must not be treated as delivery success"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "ND10: all five notify_* calls against a 500 response must complete quickly; took {elapsed:?}"
    );
    assert_eq!(
        sink.request_count.load(Ordering::SeqCst),
        5,
        "ND10: all five notify_* calls must have reached the sink"
    );
}

// ---------------------------------------------------------------------------
// ND11: sanitized summary cannot carry webhook/body material
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd11_non2xx_summary_omits_webhook_url_token_path_and_body() {
    let sink = start_configurable_sink(StatusCode::FORBIDDEN, SECRET_RESPONSE_BODY).await;

    // Sanity: the fixture URL really does carry webhook-shaped path/token, and
    // the sink really does respond with a body that must never be logged.
    assert!(sink.url.contains(DISCORD_WEBHOOK_PATH));
    assert!(sink.url.contains(FAKE_WEBHOOK_TOKEN));

    let notifier = DiscordNotifier::from_url(&sink.url);
    let delivered = notifier.notify_test_alert(&test_alert_payload()).await;
    assert!(!delivered, "ND11: 403 must be classified as a failure");

    // The sanitized summary used for logging is derived from the status code
    // alone and structurally cannot carry URL, token, path, or body content.
    let summary = discord_delivery_status_summary(StatusCode::FORBIDDEN);
    let rendered = format!("{summary:?}");

    assert!(
        !rendered.contains(&sink.url),
        "ND11: summary must not contain the webhook URL"
    );
    assert!(
        !rendered.contains(FAKE_WEBHOOK_TOKEN),
        "ND11: summary must not contain the webhook token"
    );
    assert!(
        !rendered.contains(DISCORD_WEBHOOK_PATH),
        "ND11: summary must not contain the Discord webhook path"
    );
    assert!(
        !rendered.contains(SECRET_RESPONSE_BODY),
        "ND11: summary must not contain the response body"
    );
}

// ---------------------------------------------------------------------------
// ND12: missing webhook remains a silent no-op
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd12_missing_webhook_remains_silent_noop() {
    let notifier = DiscordNotifier::noop();
    assert!(
        !notifier.is_configured(),
        "ND12: noop notifier must be structurally unconfigured"
    );
    assert!(
        !notifier.notify_test_alert(&test_alert_payload()).await,
        "ND12: missing webhook must not be treated as delivery success"
    );
}
