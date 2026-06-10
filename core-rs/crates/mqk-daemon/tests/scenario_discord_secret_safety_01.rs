//! Discord secret-safety regression tests.
//!
//! Proves delivery failure reporting never exposes webhook URL material. All
//! webhook strings in this file are fake fixtures; no real Discord endpoint is
//! contacted.

use std::time::Duration;

use mqk_daemon::notify::{
    discord_delivery_error_summary, CriticalAlertPayload, DiscordNotifier, OperatorNotifyPayload,
    RunStatusPayload, TestAlertPayload, TradeEventPayload,
};

const FAKE_LOCAL_WEBHOOK_URL: &str =
    "http://127.0.0.1:1/discord.com/api/webhooks/fake-webhook-id/fake-webhook-token";
const FAKE_DISCORD_WEBHOOK_URL: &str =
    "https://discord.com/api/webhooks/fake-webhook-id/fake-webhook-token";
const FAKE_WEBHOOK_TOKEN: &str = "fake-webhook-token";
const DISCORD_WEBHOOK_PATH: &str = "discord.com/api/webhooks";

fn assert_no_webhook_material(rendered: &str) {
    assert!(
        !rendered.contains(FAKE_LOCAL_WEBHOOK_URL),
        "rendered output must not contain the full local webhook fixture"
    );
    assert!(
        !rendered.contains(FAKE_DISCORD_WEBHOOK_URL),
        "rendered output must not contain the full Discord webhook fixture"
    );
    assert!(
        !rendered.contains(FAKE_WEBHOOK_TOKEN),
        "rendered output must not contain the webhook token fixture"
    );
    assert!(
        !rendered.contains(DISCORD_WEBHOOK_PATH),
        "rendered output must not contain the Discord webhook path"
    );
}

fn test_alert_payload() -> TestAlertPayload {
    TestAlertPayload {
        environment: Some("paper".to_string()),
        ts_utc: "2026-06-09T00:00:00Z".to_string(),
        note: "operator test; no trading state affected".to_string(),
    }
}

#[tokio::test]
async fn ds01_delivery_error_summary_omits_url_token_and_raw_error_text() {
    let err = reqwest::Client::new()
        .post(FAKE_LOCAL_WEBHOOK_URL)
        .timeout(Duration::from_millis(200))
        .send()
        .await
        .expect_err("fake local webhook fixture must be unreachable");

    let raw_error_text = err.to_string();
    assert!(
        raw_error_text.contains(FAKE_WEBHOOK_TOKEN) || raw_error_text.contains("127.0.0.1"),
        "fixture must exercise an error that can carry request URL context"
    );

    let summary = discord_delivery_error_summary(&err);
    let rendered_summary = format!("{summary:?}");
    assert_no_webhook_material(&rendered_summary);
    assert!(
        matches!(
            summary.kind,
            "timeout"
                | "connect"
                | "status"
                | "request"
                | "body"
                | "decode"
                | "builder"
                | "redirect"
                | "unknown"
        ),
        "summary kind must be one of the sanitized categories"
    );
}

#[tokio::test]
async fn ds02_delivery_failure_stays_best_effort_and_non_fatal() {
    let notifier = DiscordNotifier::from_url(FAKE_LOCAL_WEBHOOK_URL);
    let delivered = notifier.notify_test_alert(&test_alert_payload()).await;

    assert!(
        !delivered,
        "failed Discord test-alert delivery must return false, not panic"
    );

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
}

#[tokio::test]
async fn ds03_missing_webhook_is_silent_noop() {
    let notifier = DiscordNotifier::noop();

    assert!(
        !notifier.is_configured(),
        "noop notifier must be structurally unconfigured"
    );
    assert!(
        !notifier.notify_test_alert(&test_alert_payload()).await,
        "unconfigured test alert must return false without delivery"
    );

    notifier
        .notify_run_status(&RunStatusPayload {
            event: "run.stopped".to_string(),
            run_id: None,
            environment: None,
            note: None,
            ts_utc: "2026-06-09T00:00:05Z".to_string(),
        })
        .await;
}

#[test]
fn ds04_payloads_and_status_do_not_serialize_webhook_material() {
    let notifier = DiscordNotifier::from_url(FAKE_DISCORD_WEBHOOK_URL);

    let payloads = [
        serde_json::to_string(&test_alert_payload()).expect("test alert payload serializes"),
        serde_json::to_string(&OperatorNotifyPayload {
            action_key: "control.arm".to_string(),
            disposition: "applied".to_string(),
            environment: Some("paper".to_string()),
            ts_utc: "2026-06-09T00:00:06Z".to_string(),
            provenance_ref: Some("audit:test".to_string()),
            run_id: None,
        })
        .expect("operator payload serializes"),
        serde_json::to_string(&CriticalAlertPayload {
            alert_class: "runtime.halt.operator_or_safety".to_string(),
            severity: "critical".to_string(),
            summary: "halted".to_string(),
            detail: None,
            environment: Some("paper".to_string()),
            run_id: None,
            ts_utc: "2026-06-09T00:00:07Z".to_string(),
        })
        .expect("critical alert payload serializes"),
        serde_json::to_string(&RunStatusPayload {
            event: "run.started".to_string(),
            run_id: Some("run-test".to_string()),
            environment: Some("paper".to_string()),
            note: None,
            ts_utc: "2026-06-09T00:00:08Z".to_string(),
        })
        .expect("run status payload serializes"),
        serde_json::to_string(&TradeEventPayload {
            stage: "fill.terminal".to_string(),
            run_id: Some("run-test".to_string()),
            symbol: Some("AAPL".to_string()),
            side: Some("Buy".to_string()),
            qty: Some(1),
            price_micros: Some(100_000_000),
            order_id: Some("order-test".to_string()),
            detail: Some("terminal fill".to_string()),
            environment: Some("paper".to_string()),
            summary: "terminal fill applied".to_string(),
            ts_utc: "2026-06-09T00:00:09Z".to_string(),
        })
        .expect("trade event payload serializes"),
        serde_json::to_string(&notifier.status()).expect("status serializes"),
    ];

    for rendered in payloads {
        assert_no_webhook_material(&rendered);
    }
}
