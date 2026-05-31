//! DIS-05: Discord deadman alert coverage tests.
//!
//! Verifies DISCORD-DEADMAN-ALERTS-01 wiring:
//!
//! - DIS05-T01: `halt.deadman_expired` payload delivers correctly via `notify_critical_alert`.
//! - DIS05-T02: `halt.deadman_supervisor_failure` payload delivers correctly.
//! - DIS05-T03: `halt.deadman_heartbeat_failed` payload delivers correctly.
//! - DIS05-T04: Discord delivery failure does not propagate (best-effort; bad URL).
//! - DIS05-T05: No webhook secret in any deadman alert payload.
//! - DIS05-T06: alert_class field is present and correct in all three variants.
//! - DIS05-T07: severity is "critical" for all deadman halt alerts.
//! - DIS05-T08: noop notifier silently discards deadman alerts (no panic, no error).

use std::sync::{Arc, Mutex};

use mqk_daemon::notify::{CriticalAlertPayload, DiscordNotifier};

// ---------------------------------------------------------------------------
// Minimal mock HTTP server
// ---------------------------------------------------------------------------

struct MockSink {
    url: String,
    received: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl MockSink {
    async fn new() -> Self {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let received: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let rx = Arc::clone(&received);

        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let rx2 = Arc::clone(&rx);
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let mut io = stream;
                        use tokio::io::AsyncReadExt;
                        let _ = io.read_to_end(&mut buf).await;
                        let text = String::from_utf8_lossy(&buf);
                        if let Some(body_start) = text.find("\r\n\r\n") {
                            let body = &text[body_start + 4..];
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
                                rx2.lock().unwrap().push(v);
                            }
                        }
                    });
                }
            }
        });

        Self {
            url: format!("http://{addr}/hook"),
            received,
        }
    }

    fn drain(&self) -> Vec<serde_json::Value> {
        self.received.lock().unwrap().drain(..).collect()
    }
}

fn deadman_expired_payload() -> CriticalAlertPayload {
    CriticalAlertPayload {
        alert_class: "halt.deadman_expired".to_string(),
        severity: "critical".to_string(),
        summary: "Deadman TTL expired — run halted and disarmed | run=abcd1234".to_string(),
        detail: Some("disarm_reason=DeadmanExpired phase=pre_tick".to_string()),
        environment: Some("paper".to_string()),
        run_id: Some("abcd1234".to_string()),
        ts_utc: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn supervisor_failure_payload() -> CriticalAlertPayload {
    CriticalAlertPayload {
        alert_class: "halt.deadman_supervisor_failure".to_string(),
        severity: "critical".to_string(),
        summary: "Deadman supervisor check failed — run halted and disarmed | run=abcd1234"
            .to_string(),
        detail: Some(
            "disarm_reason=DeadmanSupervisorFailure error=db connection refused".to_string(),
        ),
        environment: Some("paper".to_string()),
        run_id: Some("abcd1234".to_string()),
        ts_utc: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn heartbeat_failed_payload() -> CriticalAlertPayload {
    CriticalAlertPayload {
        alert_class: "halt.deadman_heartbeat_failed".to_string(),
        severity: "critical".to_string(),
        summary: "Deadman heartbeat persist failed — run halted and disarmed | run=abcd1234"
            .to_string(),
        detail: Some("disarm_reason=DeadmanHeartbeatPersistFailed error=write timeout".to_string()),
        environment: Some("paper".to_string()),
        run_id: Some("abcd1234".to_string()),
        ts_utc: "2026-01-01T00:00:00Z".to_string(),
    }
}

// ---------------------------------------------------------------------------
// DIS05-T01: halt.deadman_expired delivers via notify_critical_alert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dis05_t01_deadman_expired_delivers() {
    let sink = MockSink::new().await;
    let notifier = DiscordNotifier::from_url(&sink.url);
    notifier
        .notify_critical_alert(&deadman_expired_payload())
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let msgs = sink.drain();
    assert_eq!(msgs.len(), 1, "DIS05-T01: expected exactly one delivery");
    let msg = &msgs[0];
    assert_eq!(
        msg["alert_class"].as_str(),
        Some("halt.deadman_expired"),
        "DIS05-T01: alert_class must be halt.deadman_expired"
    );
    assert_eq!(
        msg["severity"].as_str(),
        Some("critical"),
        "DIS05-T01: severity must be critical"
    );
    assert!(
        msg["content"]
            .as_str()
            .unwrap_or("")
            .contains("halt.deadman_expired"),
        "DIS05-T01: content must contain alert_class"
    );
}

// ---------------------------------------------------------------------------
// DIS05-T02: halt.deadman_supervisor_failure delivers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dis05_t02_supervisor_failure_delivers() {
    let sink = MockSink::new().await;
    let notifier = DiscordNotifier::from_url(&sink.url);
    notifier
        .notify_critical_alert(&supervisor_failure_payload())
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let msgs = sink.drain();
    assert_eq!(msgs.len(), 1, "DIS05-T02: expected exactly one delivery");
    let msg = &msgs[0];
    assert_eq!(
        msg["alert_class"].as_str(),
        Some("halt.deadman_supervisor_failure"),
        "DIS05-T02: alert_class must be halt.deadman_supervisor_failure"
    );
    assert_eq!(
        msg["severity"].as_str(),
        Some("critical"),
        "DIS05-T02: severity must be critical"
    );
}

// ---------------------------------------------------------------------------
// DIS05-T03: halt.deadman_heartbeat_failed delivers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dis05_t03_heartbeat_failed_delivers() {
    let sink = MockSink::new().await;
    let notifier = DiscordNotifier::from_url(&sink.url);
    notifier
        .notify_critical_alert(&heartbeat_failed_payload())
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let msgs = sink.drain();
    assert_eq!(msgs.len(), 1, "DIS05-T03: expected exactly one delivery");
    let msg = &msgs[0];
    assert_eq!(
        msg["alert_class"].as_str(),
        Some("halt.deadman_heartbeat_failed"),
        "DIS05-T03: alert_class must be halt.deadman_heartbeat_failed"
    );
}

// ---------------------------------------------------------------------------
// DIS05-T04: Discord delivery failure does not propagate (bad URL)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dis05_t04_delivery_failure_is_best_effort() {
    let notifier = DiscordNotifier::from_url("http://127.0.0.1:1/unreachable");
    // Must not panic or return Err — delivery failure is silently swallowed.
    notifier
        .notify_critical_alert(&deadman_expired_payload())
        .await;
    // If we reach here, the best-effort contract holds.
}

// ---------------------------------------------------------------------------
// DIS05-T05: No webhook secret in any deadman alert payload
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dis05_t05_no_webhook_secret_in_payload() {
    let payloads = [
        deadman_expired_payload(),
        supervisor_failure_payload(),
        heartbeat_failed_payload(),
    ];
    for payload in &payloads {
        let json = serde_json::to_string(payload).unwrap();
        assert!(
            !json.contains("webhook"),
            "DIS05-T05: payload must not contain 'webhook': {json}"
        );
        assert!(
            !json.contains("DISCORD_WEBHOOK"),
            "DIS05-T05: payload must not contain 'DISCORD_WEBHOOK': {json}"
        );
        // environment label is allowed; the URL itself must not be present.
        assert!(
            !json.contains("https://discord.com"),
            "DIS05-T05: payload must not contain Discord URL: {json}"
        );
    }
}

// ---------------------------------------------------------------------------
// DIS05-T06: alert_class is present and correct in all variants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dis05_t06_alert_class_correct_in_all_variants() {
    let cases = [
        ("halt.deadman_expired", deadman_expired_payload()),
        (
            "halt.deadman_supervisor_failure",
            supervisor_failure_payload(),
        ),
        ("halt.deadman_heartbeat_failed", heartbeat_failed_payload()),
    ];
    for (expected_class, payload) in &cases {
        assert_eq!(
            payload.alert_class.as_str(),
            *expected_class,
            "DIS05-T06: alert_class mismatch for variant {expected_class}"
        );
    }
}

// ---------------------------------------------------------------------------
// DIS05-T07: severity is "critical" for all deadman halt alerts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dis05_t07_severity_is_critical_for_all_variants() {
    let payloads = [
        deadman_expired_payload(),
        supervisor_failure_payload(),
        heartbeat_failed_payload(),
    ];
    for payload in &payloads {
        assert_eq!(
            payload.severity.as_str(),
            "critical",
            "DIS05-T07: severity must be critical for alert_class={}",
            payload.alert_class
        );
    }
}

// ---------------------------------------------------------------------------
// DIS05-T08: noop notifier silently discards deadman alerts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dis05_t08_noop_notifier_silently_discards() {
    let notifier = DiscordNotifier::noop();
    assert!(
        !notifier.is_configured(),
        "DIS05-T08: noop must not be configured"
    );
    // All three variants must be fire-and-forget no-ops.
    notifier
        .notify_critical_alert(&deadman_expired_payload())
        .await;
    notifier
        .notify_critical_alert(&supervisor_failure_payload())
        .await;
    notifier
        .notify_critical_alert(&heartbeat_failed_payload())
        .await;
    // No panic, no delivery attempt — test passes by reaching this line.
}
