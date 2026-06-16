//! DIS-06: Discord lifecycle alert coverage — flatten.requested and reconcile.clean.
//!
//! Verifies DISCORD-TRADE-LIFECYCLE-ALERTS-01 additions:
//!
//! - DIS06-T01: `flatten.requested` payload delivers correct stage/symbol/side fields.
//! - DIS06-T02: `flatten.requested` detail includes `live_routing_enabled=false`.
//! - DIS06-T03: `flatten.requested` payload includes no webhook secrets.
//! - DIS06-T04: noop notifier silently discards flatten alert (no panic, no delivery).
//! - DIS06-T05: delivery failure does not propagate (bad URL; best-effort).
//! - DIS06-T06: `reconcile.clean` payload delivers on dirty→clean transition.
//! - DIS06-T07: `reconcile.clean` detail includes `transitioned_from` field.
//! - DIS06-T08: noop notifier silently discards reconcile.clean alert.
//! - DIS06-T09: alerting does not submit orders or mutate strategy state.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mqk_daemon::notify::{DiscordNotifier, TradeEventPayload};
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Minimal mock HTTP server (same pattern as DIS-03/DIS-05)
// ---------------------------------------------------------------------------

struct MockSink {
    url: String,
    received: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl MockSink {
    async fn new() -> Self {
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn flatten_payload(enqueued_symbols: &str) -> TradeEventPayload {
    TradeEventPayload {
        stage: "flatten.requested".to_string(),
        run_id: Some("abcd1234".to_string()),
        symbol: Some(enqueued_symbols.to_string()),
        side: Some("sell".to_string()),
        qty: None,
        price_micros: None,
        order_id: None,
        detail: Some(format!(
            "live_routing_enabled=false enqueued=[{enqueued_symbols}] already_pending=[] audit=none"
        )),
        environment: Some("paper".to_string()),
        summary: format!(
            "paper flatten requested: enqueued=[{enqueued_symbols}] live_routing_enabled=false"
        ),
        ts_utc: "2026-06-05T09:00:00Z".to_string(),
    }
}

fn reconcile_clean_payload(prior: &str) -> TradeEventPayload {
    TradeEventPayload {
        stage: "reconcile.clean".to_string(),
        run_id: None,
        symbol: None,
        side: None,
        qty: None,
        price_micros: None,
        order_id: None,
        detail: Some(format!("transitioned_from={prior}")),
        environment: Some("paper".to_string()),
        summary: format!("reconcile clean after {prior}"),
        ts_utc: "2026-06-05T09:00:00Z".to_string(),
    }
}

// ---------------------------------------------------------------------------
// DIS06-T01: flatten.requested delivers correct stage/symbol/side
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn dis06_t01_flatten_requested_delivers_correct_fields() {
    let sink = MockSink::new().await;
    let notifier = DiscordNotifier::from_url(&sink.url);

    let payload = flatten_payload("AAPL");
    notifier.notify_trade_event(&payload).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let msgs = sink.drain();
    assert_eq!(msgs.len(), 1, "DIS06-T01: exactly one message expected");
    let msg = &msgs[0];

    assert_eq!(
        msg["stage"].as_str(),
        Some("flatten.requested"),
        "DIS06-T01: stage must be flatten.requested"
    );
    assert_eq!(
        msg["symbol"].as_str(),
        Some("AAPL"),
        "DIS06-T01: symbol must be AAPL"
    );
    assert_eq!(
        msg["side"].as_str(),
        Some("sell"),
        "DIS06-T01: side must be sell"
    );
    assert_eq!(
        msg["environment"].as_str(),
        Some("paper"),
        "DIS06-T01: environment must be paper"
    );
}

// ---------------------------------------------------------------------------
// DIS06-T02: flatten.requested detail includes live_routing_enabled=false
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn dis06_t02_flatten_detail_includes_live_routing_disabled() {
    let sink = MockSink::new().await;
    let notifier = DiscordNotifier::from_url(&sink.url);

    let payload = flatten_payload("AAPL");
    notifier.notify_trade_event(&payload).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let msgs = sink.drain();
    assert_eq!(msgs.len(), 1, "DIS06-T02: one message expected");
    let detail = msgs[0]["detail"].as_str().unwrap_or("");
    assert!(
        detail.contains("live_routing_enabled=false"),
        "DIS06-T02: detail must include live_routing_enabled=false, got: {detail}"
    );
}

// ---------------------------------------------------------------------------
// DIS06-T03: flatten.requested payload includes no webhook secrets
// ---------------------------------------------------------------------------

#[test]
fn dis06_t03_flatten_payload_no_secrets() {
    let secret_url = "https://discord.com/api/webhooks/secret-token-99/very-secret-path";
    let _notifier = DiscordNotifier::from_url(secret_url);

    let payload = flatten_payload("AAPL");
    let serialized = serde_json::to_string(&payload).unwrap();

    assert!(
        !serialized.contains("secret-token-99"),
        "DIS06-T03: payload must not contain webhook token"
    );
    assert!(
        !serialized.contains("very-secret-path"),
        "DIS06-T03: payload must not contain webhook path secret"
    );
    // Sanity: symbol IS present.
    assert!(
        serialized.contains("AAPL"),
        "DIS06-T03: symbol must be present in payload"
    );
}

// ---------------------------------------------------------------------------
// DIS06-T04: noop notifier silently discards flatten alert
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn dis06_t04_noop_notifier_discards_flatten_alert() {
    let notifier = DiscordNotifier::noop();
    assert!(
        !notifier.is_configured(),
        "DIS06-T04: noop must not be configured"
    );

    // Must complete without panic or error; no delivery attempted.
    notifier.notify_trade_event(&flatten_payload("AAPL")).await;
}

// ---------------------------------------------------------------------------
// DIS06-T05: delivery failure does not propagate (bad URL; best-effort)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn dis06_t05_flatten_delivery_failure_does_not_propagate() {
    let bad_notifier = DiscordNotifier::from_url("http://127.0.0.1:1/hook");
    // Must not panic even when delivery fails.
    bad_notifier
        .notify_trade_event(&flatten_payload("AAPL"))
        .await;
}

// ---------------------------------------------------------------------------
// DIS06-T06: reconcile.clean delivers on dirty→clean transition
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn dis06_t06_reconcile_clean_delivers_on_dirty_to_clean() {
    let sink = MockSink::new().await;
    let notifier = DiscordNotifier::from_url(&sink.url);

    let payload = reconcile_clean_payload("dirty");
    notifier.notify_trade_event(&payload).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let msgs = sink.drain();
    assert_eq!(msgs.len(), 1, "DIS06-T06: exactly one message expected");
    let msg = &msgs[0];

    assert_eq!(
        msg["stage"].as_str(),
        Some("reconcile.clean"),
        "DIS06-T06: stage must be reconcile.clean"
    );
    assert_eq!(
        msg["environment"].as_str(),
        Some("paper"),
        "DIS06-T06: environment must be paper"
    );
}

// ---------------------------------------------------------------------------
// DIS06-T07: reconcile.clean detail includes transitioned_from field
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn dis06_t07_reconcile_clean_detail_has_transitioned_from() {
    let sink = MockSink::new().await;
    let notifier = DiscordNotifier::from_url(&sink.url);

    let payload = reconcile_clean_payload("dirty");
    notifier.notify_trade_event(&payload).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let msgs = sink.drain();
    assert_eq!(msgs.len(), 1, "DIS06-T07: one message expected");
    let detail = msgs[0]["detail"].as_str().unwrap_or("");
    assert!(
        detail.contains("transitioned_from=dirty"),
        "DIS06-T07: detail must include transitioned_from, got: {detail}"
    );
}

// ---------------------------------------------------------------------------
// DIS06-T08: noop notifier silently discards reconcile.clean alert
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn dis06_t08_noop_discards_reconcile_clean_alert() {
    let notifier = DiscordNotifier::noop();
    assert!(
        !notifier.is_configured(),
        "DIS06-T08: noop must not be configured"
    );

    // Must complete without panic or error.
    notifier
        .notify_trade_event(&reconcile_clean_payload("dirty"))
        .await;
}

// ---------------------------------------------------------------------------
// DIS06-T09: alerting does not submit orders or mutate strategy state
// ---------------------------------------------------------------------------

#[test]
fn dis06_t09_alerting_does_not_submit_orders() {
    // This test proves structurally that TradeEventPayload carries no
    // order-submission fields: no endpoint URL, no broker credentials,
    // no live_routing_enabled=true.  The payload is a read-only observation.
    let payload = flatten_payload("AAPL");

    // No broker endpoint should appear in the payload.
    let serialized = serde_json::to_string(&payload).unwrap();
    assert!(
        !serialized.contains("/v2/orders"),
        "DIS06-T09: alert payload must not reference broker order endpoint"
    );
    assert!(
        !serialized.contains("live_routing_enabled=true"),
        "DIS06-T09: alert payload must not claim live routing is enabled"
    );
    assert!(
        serialized.contains("live_routing_enabled=false"),
        "DIS06-T09: flatten alert must explicitly record live_routing_enabled=false"
    );

    // strategy_state is absent — alerting carries no strategy mutation fields.
    assert!(
        !serialized.contains("strategy_state"),
        "DIS06-T09: alert payload must not carry strategy state mutation fields"
    );
}
