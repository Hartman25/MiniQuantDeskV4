//! Discord webhook notifier — best-effort outbound signal from authoritative daemon truth.
//!
//! Discord is an **OUTBOUND SIGNAL RAIL ONLY**.  It is NOT the source of truth.
//! Delivery failure must not affect primary daemon action results.
//!
//! # Configuration
//!
//! Set `DISCORD_WEBHOOK_URL` to a valid Discord webhook URL to enable delivery.
//! When the environment variable is absent or empty the notifier operates as a
//! silent no-op.  No delivery is attempted; no error is returned.
//!
//! # Delivery contract
//!
//! - Primary daemon action/result completes before any `notify_*` call.
//! - Delivery failure is logged as `warn!` and swallowed — it does not propagate.
//! - A 3-second timeout caps worst-case latency impact on the calling handler.
//! - All methods are no-ops when unconfigured.
//!
//! # Notification types
//!
//! - `notify_operator_action` — accepted operator control actions (arm/disarm/start/stop/halt).
//! - `notify_critical_alert` (DIS-01) — critical/warning fault conditions (halt, WS gap).
//! - `notify_run_status` (DIS-02) — paper-run lifecycle summaries (started/stopped/halted).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

/// Environment variable name for the Discord webhook URL.
pub const DISCORD_WEBHOOK_URL_ENV: &str = "DISCORD_WEBHOOK_URL";

// ---------------------------------------------------------------------------
// Payload types
// ---------------------------------------------------------------------------

/// Payload describing an accepted operator control action.
///
/// Every field comes only from authoritative daemon truth — no fabrication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorNotifyPayload {
    /// Normalised action key (e.g. `"control.arm"`, `"run.halt"`).
    pub action_key: String,
    /// Always `"applied"` for accepted control actions surfaced here.
    pub disposition: String,
    /// Daemon deployment mode label (e.g. `"paper"`, `"live-shadow"`).
    pub environment: Option<String>,
    /// RFC 3339 timestamp of the notification event.
    pub ts_utc: String,
    /// Durable audit provenance reference when a DB row was written.
    pub provenance_ref: Option<String>,
    /// Active run_id at time of action, if any.
    pub run_id: Option<String>,
}

/// Payload describing a critical or warning daemon fault condition (DIS-01).
///
/// Fired when a fault transitions to active:
/// - System halt (`runtime.halt.operator_or_safety`)
/// - Alpaca WS gap detected (`paper.ws_continuity.gap_detected`)
///
/// Distinct from `OperatorNotifyPayload`: alerts describe daemon fault state,
/// not accepted operator actions.  Every field is derived from authoritative
/// daemon truth — no fabrication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriticalAlertPayload {
    /// Fault signal class (mirrors `ActiveAlertRow.class`).
    pub alert_class: String,
    /// `"critical"` or `"warning"`.
    pub severity: String,
    /// Human-readable fault summary.
    pub summary: String,
    /// Optional additional detail (e.g. last WS message id on gap).
    pub detail: Option<String>,
    /// Daemon deployment mode label.
    pub environment: Option<String>,
    /// Active run_id at time of alert, if any.
    pub run_id: Option<String>,
    /// RFC 3339 timestamp of the alert event.
    pub ts_utc: String,
}

/// Payload for a paper-run lifecycle status summary notification (DIS-02).
///
/// Fired at run start, stop, and halt to give the operator a concise
/// structured record of the lifecycle transition.
///
/// Every field is derived from authoritative daemon truth — no fabrication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunStatusPayload {
    /// Lifecycle event: `"run.started"`, `"run.stopped"`, or `"run.halted"`.
    pub event: String,
    /// Active run_id at time of event, if any.
    pub run_id: Option<String>,
    /// Daemon deployment mode label.
    pub environment: Option<String>,
    /// Optional operator-facing note (e.g. `"dispatch fail-closed"`).
    pub note: Option<String>,
    /// RFC 3339 timestamp of the status event.
    pub ts_utc: String,
}

// ---------------------------------------------------------------------------
// Notifier
// ---------------------------------------------------------------------------

/// Best-effort Discord webhook notifier.
///
/// Cloneable — `reqwest::Client` wraps an `Arc` internally so cloning is cheap.
/// Constructed once at daemon startup and shared via `AppState`.
#[derive(Clone)]
pub struct DiscordNotifier {
    webhook_url: Option<String>,
    client: Option<reqwest::Client>,
}

impl DiscordNotifier {
    /// Construct from environment.  Silent no-op when `DISCORD_WEBHOOK_URL`
    /// is absent or empty.
    pub fn from_env() -> Self {
        let url = std::env::var(DISCORD_WEBHOOK_URL_ENV)
            .ok()
            .filter(|s| !s.is_empty());
        let client = url.as_ref().map(|_| reqwest::Client::new());
        Self {
            webhook_url: url,
            client,
        }
    }

    /// Construct with an explicit URL.  Used in tests and targeted wiring.
    pub fn from_url(url: impl Into<String>) -> Self {
        Self {
            webhook_url: Some(url.into()),
            client: Some(reqwest::Client::new()),
        }
    }

    /// Explicit no-op instance — never attempts delivery.
    pub fn noop() -> Self {
        Self {
            webhook_url: None,
            client: None,
        }
    }

    /// Returns `true` when a webhook URL is configured and delivery will be
    /// attempted on the next call.
    pub fn is_configured(&self) -> bool {
        self.webhook_url.is_some()
    }

    /// Best-effort delivery of an accepted operator action notification.
    ///
    /// Returns immediately (no-op) when the notifier is not configured.
    /// Delivery errors are logged as `warn!` and swallowed — the primary
    /// daemon action has already been applied before this is called.
    pub async fn notify_operator_action(&self, payload: &OperatorNotifyPayload) {
        let (Some(url), Some(client)) = (&self.webhook_url, &self.client) else {
            return;
        };

        // Discord webhook expects a JSON body. We include both a human-readable
        // `content` string and structured fields so downstream consumers can
        // parse either form.
        let content = format!(
            "[mqk-daemon] `{}` → `{}` | env: `{}` | ts: `{}` | ref: `{}`",
            payload.action_key,
            payload.disposition,
            payload.environment.as_deref().unwrap_or("unknown"),
            payload.ts_utc,
            payload.provenance_ref.as_deref().unwrap_or("none"),
        );

        let body = serde_json::json!({
            "content": content,
            "action_key": payload.action_key,
            "disposition": payload.disposition,
            "environment": payload.environment,
            "ts_utc": payload.ts_utc,
            "provenance_ref": payload.provenance_ref,
            "run_id": payload.run_id,
        });

        if let Err(err) = client
            .post(url.as_str())
            .json(&body)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            warn!(
                error = %err,
                action_key = %payload.action_key,
                "discord webhook delivery failed (best-effort; primary action truth unaffected)"
            );
        }
    }

    /// Best-effort delivery of a critical or warning fault alert (DIS-01).
    ///
    /// Distinct from `notify_operator_action`: this fires for daemon fault
    /// *conditions* (halt, WS gap), not accepted operator control actions.
    ///
    /// Same delivery contract: no-op when unconfigured, errors logged as
    /// `warn!` and swallowed, 3-second timeout.
    pub async fn notify_critical_alert(&self, payload: &CriticalAlertPayload) {
        let (Some(url), Some(client)) = (&self.webhook_url, &self.client) else {
            return;
        };

        let content = format!(
            "[mqk-daemon] ALERT `{}` | severity: `{}` | env: `{}` | ts: `{}` | {}",
            payload.alert_class,
            payload.severity,
            payload.environment.as_deref().unwrap_or("unknown"),
            payload.ts_utc,
            payload.summary,
        );

        let body = serde_json::json!({
            "content": content,
            "alert_class": payload.alert_class,
            "severity": payload.severity,
            "summary": payload.summary,
            "detail": payload.detail,
            "environment": payload.environment,
            "run_id": payload.run_id,
            "ts_utc": payload.ts_utc,
        });

        if let Err(err) = client
            .post(url.as_str())
            .json(&body)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            warn!(
                error = %err,
                alert_class = %payload.alert_class,
                "discord alert delivery failed (best-effort; daemon fault truth unaffected)"
            );
        }
    }

    /// Best-effort delivery of a paper-run lifecycle status summary (DIS-02).
    ///
    /// Fired at run start, stop, and halt.  Gives the operator a concise
    /// structured record of each lifecycle transition without polling.
    ///
    /// Same delivery contract: no-op when unconfigured, errors logged as
    /// `warn!` and swallowed, 3-second timeout.
    pub async fn notify_run_status(&self, payload: &RunStatusPayload) {
        let (Some(url), Some(client)) = (&self.webhook_url, &self.client) else {
            return;
        };

        let content = format!(
            "[mqk-daemon] `{}` | env: `{}` | run: `{}` | ts: `{}`{}",
            payload.event,
            payload.environment.as_deref().unwrap_or("unknown"),
            payload.run_id.as_deref().unwrap_or("none"),
            payload.ts_utc,
            payload
                .note
                .as_ref()
                .map(|n| format!(" | {n}"))
                .unwrap_or_default(),
        );

        let body = serde_json::json!({
            "content": content,
            "event": payload.event,
            "run_id": payload.run_id,
            "environment": payload.environment,
            "note": payload.note,
            "ts_utc": payload.ts_utc,
        });

        if let Err(err) = client
            .post(url.as_str())
            .json(&body)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            warn!(
                error = %err,
                event = %payload.event,
                "discord run-status delivery failed (best-effort; daemon lifecycle truth unaffected)"
            );
        }
    }
}
