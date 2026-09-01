//! DISCORD-CHANNEL-ROUTING-01: proof that each `DiscordNotifier::notify_*`
//! method routes to its documented canonical channel (`notify.rs` module
//! docs), and only that channel — never the flat single-webhook behavior
//! this patch replaces.
//!
//! Each test stands up one in-process webhook sink per channel under test
//! and constructs a `DiscordNotifier::from_resolved_webhooks` pointing
//! different channels at different sinks, so a hit on the wrong sink is a
//! real, observable routing defect — not an assumption.
//!
//! No real Discord webhook is contacted. No Live daemon mode is engaged
//! anywhere in this file — the live-channel routing proof is entirely at
//! the `DiscordNotifier` level, hermetic per the mission's requirement that
//! it be provable "without enabling Live."
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                             |
//! |------|-----------------------------------------------------------------------------|
//! | CR01 | `notify_critical_alert` delivers to `alerts` only                         |
//! | CR02 | `notify_operator_action` delivers to `c2` only                            |
//! | CR03 | `notify_trade_event` with environment="paper" delivers to `paper` only    |
//! | CR04 | `notify_trade_event` with environment="live-shadow"/"live-capital" delivers to `live` only |
//! | CR05 | `notify_run_status` follows the same paper/live routing as trade events   |
//! | CR06 | `notify_test_alert` delivers to `alerts` (same channel as critical alert) |
//! | CR07 | Unrecognized/absent environment routes to no channel, even if all are configured |
//! | CR08 | An unconfigured channel no-ops even when every other channel is fully configured |
//! | CR09 | `DiscordNotifier::from_env()` reads the canonical per-channel env vars     |
//! | CR10 | `from_env()` honors a custom channel env-var NAME from `MQK_DISCORD_CONFIG_PATH` |
//! | CR11 | Same config load: a channel absent from the file still uses its canonical default |
//! | CR12 | `is_paper_channel_configured` is Paper-specific, not "any channel" (repair) |

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use mqk_daemon::notify::{
    CriticalAlertPayload, DiscordNotifier, OperatorNotifyPayload, RunStatusPayload,
    TestAlertPayload, TradeEventPayload,
};
use mqk_config::secrets::ResolvedDiscordWebhooks;

// ---------------------------------------------------------------------------
// In-process webhook sink
// ---------------------------------------------------------------------------

struct Sink {
    url: String,
    hits: Arc<AtomicUsize>,
}

async fn start_sink() -> Sink {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();

    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |_body: axum::body::Bytes| {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::NO_CONTENT
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    Sink {
        url: format!("http://127.0.0.1:{}/hook", addr.port()),
        hits,
    }
}

fn ts() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ---------------------------------------------------------------------------
// CR01 — notify_critical_alert delivers to alerts only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr01_critical_alert_delivers_to_alerts_only() {
    let alerts = start_sink().await;
    let c2 = start_sink().await;
    let paper = start_sink().await;
    let live = start_sink().await;

    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: Some(paper.url.clone()),
        live: Some(live.url.clone()),
        backtest: None,
        alerts: Some(alerts.url.clone()),
        heartbeat: None,
        c2: Some(c2.url.clone()),
    });

    notifier
        .notify_critical_alert(&CriticalAlertPayload {
            alert_class: "runtime.halt.operator_or_safety".to_string(),
            severity: "critical".to_string(),
            summary: "test".to_string(),
            detail: None,
            environment: Some("paper".to_string()),
            run_id: None,
            ts_utc: ts(),
        })
        .await;

    assert_eq!(alerts.hits.load(Ordering::SeqCst), 1, "alerts channel must receive the alert");
    assert_eq!(c2.hits.load(Ordering::SeqCst), 0, "c2 channel must not receive it");
    assert_eq!(paper.hits.load(Ordering::SeqCst), 0, "paper channel must not receive it");
    assert_eq!(live.hits.load(Ordering::SeqCst), 0, "live channel must not receive it");
}

// ---------------------------------------------------------------------------
// CR02 — notify_operator_action delivers to c2 only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr02_operator_action_delivers_to_c2_only() {
    let alerts = start_sink().await;
    let c2 = start_sink().await;

    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: None,
        live: None,
        backtest: None,
        alerts: Some(alerts.url.clone()),
        heartbeat: None,
        c2: Some(c2.url.clone()),
    });

    notifier
        .notify_operator_action(&OperatorNotifyPayload {
            action_key: "control.arm".to_string(),
            disposition: "applied".to_string(),
            environment: Some("paper".to_string()),
            ts_utc: ts(),
            provenance_ref: None,
            run_id: None,
        })
        .await;

    assert_eq!(c2.hits.load(Ordering::SeqCst), 1, "c2 channel must receive the operator action");
    assert_eq!(alerts.hits.load(Ordering::SeqCst), 0, "alerts channel must not receive it");
}

// ---------------------------------------------------------------------------
// CR03 — trade_event(environment="paper") delivers to paper only
// ---------------------------------------------------------------------------

fn trade_event_payload(environment: Option<&str>) -> TradeEventPayload {
    TradeEventPayload {
        stage: "fill.terminal".to_string(),
        run_id: Some("run-test".to_string()),
        symbol: Some("AAPL".to_string()),
        side: Some("Buy".to_string()),
        qty: Some(1),
        price_micros: Some(100_000_000),
        order_id: Some("order-test".to_string()),
        detail: None,
        environment: environment.map(|s| s.to_string()),
        summary: "terminal fill applied".to_string(),
        ts_utc: ts(),
    }
}

#[tokio::test]
async fn cr03_trade_event_paper_delivers_to_paper_only() {
    let paper = start_sink().await;
    let live = start_sink().await;

    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: Some(paper.url.clone()),
        live: Some(live.url.clone()),
        backtest: None,
        alerts: None,
        heartbeat: None,
        c2: None,
    });

    notifier
        .notify_trade_event(&trade_event_payload(Some("paper")))
        .await;

    assert_eq!(paper.hits.load(Ordering::SeqCst), 1, "paper channel must receive the trade event");
    assert_eq!(live.hits.load(Ordering::SeqCst), 0, "live channel must not receive it");
}

// ---------------------------------------------------------------------------
// CR04 — trade_event(environment="live-shadow"/"live-capital") delivers to
// live only. Proves the live-channel mapping hermetically: no Live daemon
// mode is engaged anywhere here, only the notifier's own routing logic.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr04_trade_event_live_shadow_delivers_to_live_only() {
    let paper = start_sink().await;
    let live = start_sink().await;

    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: Some(paper.url.clone()),
        live: Some(live.url.clone()),
        backtest: None,
        alerts: None,
        heartbeat: None,
        c2: None,
    });

    notifier
        .notify_trade_event(&trade_event_payload(Some("live-shadow")))
        .await;

    assert_eq!(live.hits.load(Ordering::SeqCst), 1, "live channel must receive live-shadow trade events");
    assert_eq!(paper.hits.load(Ordering::SeqCst), 0, "paper channel must not receive it");
}

#[tokio::test]
async fn cr04b_trade_event_live_capital_delivers_to_live_only() {
    let paper = start_sink().await;
    let live = start_sink().await;

    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: Some(paper.url.clone()),
        live: Some(live.url.clone()),
        backtest: None,
        alerts: None,
        heartbeat: None,
        c2: None,
    });

    notifier
        .notify_trade_event(&trade_event_payload(Some("live-capital")))
        .await;

    assert_eq!(live.hits.load(Ordering::SeqCst), 1, "live channel must receive live-capital trade events");
    assert_eq!(paper.hits.load(Ordering::SeqCst), 0, "paper channel must not receive it");
}

// ---------------------------------------------------------------------------
// CR05 — notify_run_status follows the same paper/live routing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr05_run_status_paper_delivers_to_paper_only() {
    let paper = start_sink().await;
    let live = start_sink().await;

    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: Some(paper.url.clone()),
        live: Some(live.url.clone()),
        backtest: None,
        alerts: None,
        heartbeat: None,
        c2: None,
    });

    notifier
        .notify_run_status(&RunStatusPayload {
            event: "run.started".to_string(),
            run_id: Some("run-test".to_string()),
            environment: Some("paper".to_string()),
            note: None,
            ts_utc: ts(),
        })
        .await;

    assert_eq!(paper.hits.load(Ordering::SeqCst), 1);
    assert_eq!(live.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cr05b_run_status_live_shadow_delivers_to_live_only() {
    let paper = start_sink().await;
    let live = start_sink().await;

    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: Some(paper.url.clone()),
        live: Some(live.url.clone()),
        backtest: None,
        alerts: None,
        heartbeat: None,
        c2: None,
    });

    notifier
        .notify_run_status(&RunStatusPayload {
            event: "run.halted".to_string(),
            run_id: Some("run-test".to_string()),
            environment: Some("live-shadow".to_string()),
            note: None,
            ts_utc: ts(),
        })
        .await;

    assert_eq!(live.hits.load(Ordering::SeqCst), 1);
    assert_eq!(paper.hits.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// CR06 — notify_test_alert delivers to alerts (same channel as critical alert)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr06_test_alert_delivers_to_alerts_only() {
    let alerts = start_sink().await;
    let c2 = start_sink().await;

    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: None,
        live: None,
        backtest: None,
        alerts: Some(alerts.url.clone()),
        heartbeat: None,
        c2: Some(c2.url.clone()),
    });

    let delivered = notifier
        .notify_test_alert(&TestAlertPayload {
            environment: Some("paper".to_string()),
            ts_utc: ts(),
            note: "operator test".to_string(),
        })
        .await;

    assert!(delivered, "alerts channel is configured; delivery must succeed");
    assert_eq!(alerts.hits.load(Ordering::SeqCst), 1);
    assert_eq!(c2.hits.load(Ordering::SeqCst), 0, "test alert must not land on c2");
}

// ---------------------------------------------------------------------------
// CR07 — unrecognized/absent environment routes to no channel, even when
// every channel is configured (fail closed on channel selection)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr07_unrecognized_environment_routes_nowhere() {
    let paper = start_sink().await;
    let live = start_sink().await;
    let backtest = start_sink().await;

    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: Some(paper.url.clone()),
        live: Some(live.url.clone()),
        backtest: Some(backtest.url.clone()),
        alerts: None,
        heartbeat: None,
        c2: None,
    });

    notifier
        .notify_trade_event(&trade_event_payload(Some("staging"))) // not a real deployment mode label
        .await;
    notifier.notify_trade_event(&trade_event_payload(None)).await;

    assert_eq!(paper.hits.load(Ordering::SeqCst), 0);
    assert_eq!(live.hits.load(Ordering::SeqCst), 0);
    assert_eq!(backtest.hits.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// CR08 — an unconfigured channel no-ops even when every other channel is
// fully configured
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr08_unconfigured_channel_noops_with_others_fully_configured() {
    let paper = start_sink().await;
    let live = start_sink().await;
    let c2 = start_sink().await;
    // alerts intentionally left unconfigured.

    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: Some(paper.url.clone()),
        live: Some(live.url.clone()),
        backtest: None,
        alerts: None,
        heartbeat: None,
        c2: Some(c2.url.clone()),
    });

    assert!(
        notifier.is_configured(),
        "notifier must report configured (other channels are set)"
    );
    assert!(
        !notifier.is_alerts_channel_configured(),
        "alerts specifically must report unconfigured"
    );

    notifier
        .notify_critical_alert(&CriticalAlertPayload {
            alert_class: "runtime.halt.operator_or_safety".to_string(),
            severity: "critical".to_string(),
            summary: "test".to_string(),
            detail: None,
            environment: Some("paper".to_string()),
            run_id: None,
            ts_utc: ts(),
        })
        .await;
    let delivered = notifier
        .notify_test_alert(&TestAlertPayload {
            environment: Some("paper".to_string()),
            ts_utc: ts(),
            note: "operator test".to_string(),
        })
        .await;

    assert!(!delivered, "test alert must report non-delivery when alerts is unconfigured");
    assert_eq!(paper.hits.load(Ordering::SeqCst), 0, "unrelated channels must not receive alerts traffic");
    assert_eq!(live.hits.load(Ordering::SeqCst), 0);
    assert_eq!(c2.hits.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// CR09 — DiscordNotifier::from_env() reads the canonical per-channel env
// vars (config/defaults/base.yaml's discord.channels.* NAMES)
// ---------------------------------------------------------------------------

static ENV_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

fn env_lock() -> &'static tokio::sync::Mutex<()> {
    ENV_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

struct EnvGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prior = std::env::var(key).ok();
        #[allow(deprecated)]
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, prior }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        #[allow(deprecated)]
        unsafe {
            match &self.prior {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[tokio::test]
async fn cr09_from_env_reads_canonical_per_channel_names() {
    let _lock = env_lock().lock().await;
    let alerts = start_sink().await;

    // Only DISCORD_WEBHOOK_ALERTS is set; every other canonical name stays
    // whatever the ambient test process has (almost always absent).
    let _g = EnvGuard::set("DISCORD_WEBHOOK_ALERTS", &alerts.url);

    let notifier = DiscordNotifier::from_env();
    assert!(
        notifier.is_alerts_channel_configured(),
        "from_env() must read DISCORD_WEBHOOK_ALERTS for the alerts channel"
    );

    notifier
        .notify_critical_alert(&CriticalAlertPayload {
            alert_class: "runtime.halt.operator_or_safety".to_string(),
            severity: "critical".to_string(),
            summary: "test".to_string(),
            detail: None,
            environment: Some("paper".to_string()),
            run_id: None,
            ts_utc: ts(),
        })
        .await;

    assert_eq!(alerts.hits.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// CR10/CR11 — DiscordNotifier::from_env() honors the operator's configured
// /discord/channels/* env-var-NAME mapping loaded from
// MQK_DISCORD_CONFIG_PATH (DISCORD-CHANNEL-ROUTING-01 repair: production
// construction must not substitute Value::Null for real configuration).
// ---------------------------------------------------------------------------

static NEXT_CONFIG_ID: AtomicUsize = AtomicUsize::new(0);

fn write_temp_discord_config(tag: &str, contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_discord_config_{tag}_{}_{}.yaml",
        std::process::id(),
        NEXT_CONFIG_ID.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&path, contents).expect("write temp discord config yaml");
    path
}

#[tokio::test]
async fn cr10_from_env_honors_custom_channel_name_from_config_path() {
    let _lock = env_lock().lock().await;
    let custom_sink = start_sink().await;
    let decoy_sink = start_sink().await;

    // The operator's config points the `alerts` channel at a non-default
    // env var NAME.
    let config_path = write_temp_discord_config(
        "cr10",
        "discord:\n  channels:\n    alerts: TEST_CUSTOM_ALERT_WEBHOOK_NAME\n",
    );

    let _g_path = EnvGuard::set(
        "MQK_DISCORD_CONFIG_PATH",
        config_path.to_str().expect("utf8 temp path"),
    );
    // The custom-named env var carries the real webhook URL under test.
    let _g_custom = EnvGuard::set("TEST_CUSTOM_ALERT_WEBHOOK_NAME", &custom_sink.url);
    // A decoy under the CANONICAL default name must be ignored — if
    // from_env() were still substituting Value::Null for the loaded config,
    // it would resolve `alerts` against DISCORD_WEBHOOK_ALERTS instead and
    // hit this sink rather than the custom one.
    let _g_decoy = EnvGuard::set("DISCORD_WEBHOOK_ALERTS", &decoy_sink.url);

    let notifier = DiscordNotifier::from_env();
    assert!(
        notifier.is_alerts_channel_configured(),
        "from_env() must resolve the alerts channel via the custom NAME"
    );

    notifier
        .notify_critical_alert(&CriticalAlertPayload {
            alert_class: "runtime.halt.operator_or_safety".to_string(),
            severity: "critical".to_string(),
            summary: "test".to_string(),
            detail: None,
            environment: Some("paper".to_string()),
            run_id: None,
            ts_utc: ts(),
        })
        .await;

    let _ = std::fs::remove_file(&config_path);

    assert_eq!(
        custom_sink.hits.load(Ordering::SeqCst),
        1,
        "delivery must go to the custom-named env var's URL"
    );
    assert_eq!(
        decoy_sink.hits.load(Ordering::SeqCst),
        0,
        "the canonical default env var NAME must be ignored once the config \
         file names a different one for this channel"
    );
}

#[tokio::test]
async fn cr11_channel_absent_from_config_file_still_uses_canonical_default() {
    let _lock = env_lock().lock().await;
    let c2_sink = start_sink().await;

    // Same load-bearing config file shape as CR10, but this time it only
    // names the `alerts` channel — `c2` is absent from the file entirely.
    let config_path = write_temp_discord_config(
        "cr11",
        "discord:\n  channels:\n    alerts: TEST_CUSTOM_ALERT_WEBHOOK_NAME_CR11\n",
    );
    let _g_path = EnvGuard::set(
        "MQK_DISCORD_CONFIG_PATH",
        config_path.to_str().expect("utf8 temp path"),
    );
    // c2 is resolved via the canonical default name, DISCORD_WEBHOOK_C2,
    // even though a config file IS loaded — a config load must not turn
    // every unmentioned channel silently unconfigured.
    let _g_c2 = EnvGuard::set("DISCORD_WEBHOOK_C2", &c2_sink.url);

    let notifier = DiscordNotifier::from_env();

    notifier
        .notify_operator_action(&OperatorNotifyPayload {
            action_key: "control.arm".to_string(),
            disposition: "applied".to_string(),
            environment: Some("paper".to_string()),
            ts_utc: ts(),
            provenance_ref: None,
            run_id: None,
        })
        .await;

    let _ = std::fs::remove_file(&config_path);

    assert_eq!(
        c2_sink.hits.load(Ordering::SeqCst),
        1,
        "a channel not named in the loaded config file must still resolve \
         via its canonical default env var NAME"
    );
}

// ---------------------------------------------------------------------------
// CR12 — is_paper_channel_configured is Paper-channel-specific, not "any
// channel configured" (observability truth repair)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr12_is_paper_channel_configured_is_channel_specific() {
    let c2 = start_sink().await;

    // Only c2 is configured — no paper channel.
    let notifier = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: None,
        live: None,
        backtest: None,
        alerts: None,
        heartbeat: None,
        c2: Some(c2.url.clone()),
    });

    assert!(
        notifier.is_configured(),
        "is_configured() (any channel) must still be true — c2 is configured"
    );
    assert!(
        !notifier.is_paper_channel_configured(),
        "is_paper_channel_configured() must be false when only c2 is \
         configured — Paper lifecycle notifications would still be a silent \
         no-op"
    );

    let paper = start_sink().await;
    let notifier_with_paper = DiscordNotifier::from_resolved_webhooks(ResolvedDiscordWebhooks {
        paper: Some(paper.url.clone()),
        live: None,
        backtest: None,
        alerts: None,
        heartbeat: None,
        c2: None,
    });
    assert!(
        notifier_with_paper.is_paper_channel_configured(),
        "is_paper_channel_configured() must be true when paper is configured"
    );
}
