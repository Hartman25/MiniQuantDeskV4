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
//! - HTTP 2xx (including 204 No Content) is delivery success.
//! - HTTP non-2xx is classified as a sanitized, best-effort delivery failure.
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

/// Payload for an operator-triggered test alert (OBS-SESSION-DISCORD-01).
///
/// Fires on `POST /api/v1/ops/action {"action_key":"test-discord-alert"}`.
/// The payload is clearly marked as a test so Discord operators can distinguish
/// it from real fault alerts.  No trading state is mutated by this path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestAlertPayload {
    /// Daemon deployment mode label (e.g. `"paper"`).
    pub environment: Option<String>,
    /// RFC 3339 timestamp of the test-alert event.
    pub ts_utc: String,
    /// Human-readable note; always includes "operator test" marker.
    pub note: String,
}

/// Status of the Discord notifier as surfaced on operator read surfaces.
///
/// No secrets (webhook URL) are included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordNotifierStatus {
    /// True when `DISCORD_WEBHOOK_URL` is set and non-empty.
    pub configured: bool,
    /// True when delivery will be attempted on the next notify call.
    /// Identical to `configured` — present for clarity on operator surfaces.
    pub delivery_enabled: bool,
}

/// URL-free summary of a Discord delivery failure.
///
/// This intentionally omits the raw `reqwest::Error` text because reqwest can
/// format request URL context, which would expose the webhook token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscordDeliveryErrorSummary {
    pub kind: &'static str,
    pub status_code: Option<u16>,
    pub is_timeout: bool,
    pub is_connect: bool,
}

pub fn discord_delivery_error_summary(err: &reqwest::Error) -> DiscordDeliveryErrorSummary {
    let status_code = err.status().map(|status| status.as_u16());
    let kind = if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connect"
    } else if status_code.is_some() {
        "status"
    } else if err.is_request() {
        "request"
    } else if err.is_body() {
        "body"
    } else if err.is_decode() {
        "decode"
    } else if err.is_builder() {
        "builder"
    } else if err.is_redirect() {
        "redirect"
    } else {
        "unknown"
    };

    DiscordDeliveryErrorSummary {
        kind,
        status_code,
        is_timeout: err.is_timeout(),
        is_connect: err.is_connect(),
    }
}

/// Broad classification of an HTTP response status code for sanitized
/// logging. Never derived from headers, body, or URL — status code only.
pub fn discord_status_class(status: reqwest::StatusCode) -> &'static str {
    if status.is_redirection() {
        "redirection"
    } else if status.is_client_error() {
        "client_error"
    } else if status.is_server_error() {
        "server_error"
    } else {
        "unknown"
    }
}

/// URL-free, body-free summary of a non-2xx Discord HTTP response, suitable
/// for sanitized logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscordDeliveryStatusSummary {
    pub status_code: u16,
    pub status_class: &'static str,
}

/// Build a [`DiscordDeliveryStatusSummary`] from a non-2xx response status.
pub fn discord_delivery_status_summary(
    status: reqwest::StatusCode,
) -> DiscordDeliveryStatusSummary {
    DiscordDeliveryStatusSummary {
        status_code: status.as_u16(),
        status_class: discord_status_class(status),
    }
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

    /// Returns a redacted status snapshot — never includes the webhook URL.
    pub fn status(&self) -> DiscordNotifierStatus {
        DiscordNotifierStatus {
            configured: self.webhook_url.is_some(),
            delivery_enabled: self.webhook_url.is_some(),
        }
    }

    /// Best-effort delivery of an operator test alert (OBS-SESSION-DISCORD-01).
    ///
    /// Used by `POST /api/v1/ops/action {"action_key":"test-discord-alert"}`.
    /// Returns `true` when the HTTP response status was 2xx (including 204 No
    /// Content), `false` when unconfigured, the response was non-2xx, or a
    /// transport error prevented delivery.  Never panics.
    ///
    /// The payload is clearly labelled `"[TEST]"` so operators can distinguish
    /// it from production fault alerts.  No trading state is mutated.
    pub async fn notify_test_alert(&self, payload: &TestAlertPayload) -> bool {
        let (Some(url), Some(client)) = (&self.webhook_url, &self.client) else {
            return false;
        };

        let content = format!(
            "[mqk-daemon] [TEST] test-discord-alert | env: `{}` | ts: `{}` | {}",
            payload.environment.as_deref().unwrap_or("unknown"),
            payload.ts_utc,
            payload.note,
        );

        let body = serde_json::json!({
            "content": content,
            "alert_type": "test",
            "environment": payload.environment,
            "note": payload.note,
            "ts_utc": payload.ts_utc,
        });

        match client
            .post(url.as_str())
            .json(&body)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => true,
            Ok(resp) => {
                let summary = discord_delivery_status_summary(resp.status());
                warn!(
                    status_code = summary.status_code,
                    status_class = summary.status_class,
                    "discord test-alert delivery failed (best-effort; operator action truth unaffected)"
                );
                false
            }
            Err(err) => {
                let summary = discord_delivery_error_summary(&err);
                warn!(
                    error_kind = summary.kind,
                    is_timeout = summary.is_timeout,
                    is_connect = summary.is_connect,
                    has_status = summary.status_code.is_some(),
                    status_code = summary.status_code.unwrap_or(0),
                    "discord test-alert delivery failed (best-effort; operator action truth unaffected)"
                );
                false
            }
        }
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

        match client
            .post(url.as_str())
            .json(&body)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => {
                let summary = discord_delivery_status_summary(resp.status());
                warn!(
                    status_code = summary.status_code,
                    status_class = summary.status_class,
                    action_key = %payload.action_key,
                    "discord webhook delivery failed (best-effort; primary action truth unaffected)"
                );
            }
            Err(err) => {
                let summary = discord_delivery_error_summary(&err);
                warn!(
                    error_kind = summary.kind,
                    is_timeout = summary.is_timeout,
                    is_connect = summary.is_connect,
                    has_status = summary.status_code.is_some(),
                    status_code = summary.status_code.unwrap_or(0),
                    action_key = %payload.action_key,
                    "discord webhook delivery failed (best-effort; primary action truth unaffected)"
                );
            }
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

        let detail_suffix = payload
            .detail
            .as_deref()
            .map(|d| format!(" | reason: {d}"))
            .unwrap_or_default();
        let content = format!(
            "[mqk-daemon] ALERT `{}` | severity: `{}` | env: `{}` | ts: `{}` | {}{}",
            payload.alert_class,
            payload.severity,
            payload.environment.as_deref().unwrap_or("unknown"),
            payload.ts_utc,
            payload.summary,
            detail_suffix,
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

        match client
            .post(url.as_str())
            .json(&body)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => {
                let summary = discord_delivery_status_summary(resp.status());
                warn!(
                    status_code = summary.status_code,
                    status_class = summary.status_class,
                    alert_class = %payload.alert_class,
                    "discord alert delivery failed (best-effort; daemon fault truth unaffected)"
                );
            }
            Err(err) => {
                let summary = discord_delivery_error_summary(&err);
                warn!(
                    error_kind = summary.kind,
                    is_timeout = summary.is_timeout,
                    is_connect = summary.is_connect,
                    has_status = summary.status_code.is_some(),
                    status_code = summary.status_code.unwrap_or(0),
                    alert_class = %payload.alert_class,
                    "discord alert delivery failed (best-effort; daemon fault truth unaffected)"
                );
            }
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

        match client
            .post(url.as_str())
            .json(&body)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => {
                let summary = discord_delivery_status_summary(resp.status());
                warn!(
                    status_code = summary.status_code,
                    status_class = summary.status_class,
                    event = %payload.event,
                    "discord run-status delivery failed (best-effort; daemon lifecycle truth unaffected)"
                );
            }
            Err(err) => {
                let summary = discord_delivery_error_summary(&err);
                warn!(
                    error_kind = summary.kind,
                    is_timeout = summary.is_timeout,
                    is_connect = summary.is_connect,
                    has_status = summary.status_code.is_some(),
                    status_code = summary.status_code.unwrap_or(0),
                    event = %payload.event,
                    "discord run-status delivery failed (best-effort; daemon lifecycle truth unaffected)"
                );
            }
        }
    }

    /// Best-effort delivery of a trade lifecycle event (DISCORD-TRADE-LIFECYCLE-ALERTS-01).
    ///
    /// Covers: order submitted, order ACKed, fill applied (partial/terminal),
    /// reconcile drift halt, recovery quarantine.  Called from the orchestrator
    /// alert sink after each durable DB write — primary execution path has
    /// already completed before this is invoked.
    ///
    /// Same delivery contract: no-op when unconfigured, errors logged as
    /// `warn!` and swallowed, 3-second timeout.
    pub async fn notify_trade_event(&self, payload: &TradeEventPayload) {
        let (Some(url), Some(client)) = (&self.webhook_url, &self.client) else {
            return;
        };

        let content = format!(
            "[mqk-daemon] `{}` | env: `{}` | run: `{}` | {}",
            payload.stage,
            payload.environment.as_deref().unwrap_or("unknown"),
            payload.run_id.as_deref().unwrap_or("none"),
            payload.summary,
        );

        let body = serde_json::json!({
            "content": content,
            "stage": payload.stage,
            "run_id": payload.run_id,
            "symbol": payload.symbol,
            "side": payload.side,
            "qty": payload.qty,
            "price_micros": payload.price_micros,
            "order_id": payload.order_id,
            "detail": payload.detail,
            "environment": payload.environment,
            "ts_utc": payload.ts_utc,
        });

        match client
            .post(url.as_str())
            .json(&body)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => {
                let summary = discord_delivery_status_summary(resp.status());
                warn!(
                    status_code = summary.status_code,
                    status_class = summary.status_class,
                    stage = %payload.stage,
                    "discord trade-event delivery failed (best-effort; trading path unaffected)"
                );
            }
            Err(err) => {
                let summary = discord_delivery_error_summary(&err);
                warn!(
                    error_kind = summary.kind,
                    is_timeout = summary.is_timeout,
                    is_connect = summary.is_connect,
                    has_status = summary.status_code.is_some(),
                    status_code = summary.status_code.unwrap_or(0),
                    stage = %payload.stage,
                    "discord trade-event delivery failed (best-effort; trading path unaffected)"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Trade lifecycle payload (DISCORD-TRADE-LIFECYCLE-ALERTS-01)
// ---------------------------------------------------------------------------

/// Payload for a trade lifecycle event notification.
///
/// Carries enough context for an operator to identify the affected order
/// without reading daemon logs.  No secrets or internal auth state included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeEventPayload {
    /// Lifecycle stage (e.g. `"order.submitted"`, `"order.acked"`, `"fill.terminal"`).
    pub stage: String,
    /// Short run_id (first 8 hex chars) for operator readability.
    pub run_id: Option<String>,
    /// Affected symbol, if applicable.
    pub symbol: Option<String>,
    /// Order side (`"Buy"` / `"Sell"`), if applicable.
    pub side: Option<String>,
    /// Quantity (shares), if applicable.
    pub qty: Option<i64>,
    /// Fill price in micros (price × 1_000_000), if applicable.
    pub price_micros: Option<i64>,
    /// Internal order ID, if applicable.
    pub order_id: Option<String>,
    /// Additional operator-facing detail (reason, blocker, etc.).
    pub detail: Option<String>,
    /// Daemon deployment mode label.
    pub environment: Option<String>,
    /// Human-readable summary line (included in Discord `content`).
    pub summary: String,
    /// RFC 3339 timestamp of the event.
    pub ts_utc: String,
}
