//! Discord webhook notifier — best-effort outbound signal from authoritative daemon truth.
//!
//! Discord is an **OUTBOUND SIGNAL RAIL ONLY**.  It is NOT the source of truth.
//! Delivery failure must not affect primary daemon action results.
//!
//! # Configuration (DISCORD-CHANNEL-ROUTING-01)
//!
//! Webhook URLs are resolved per logical channel via the canonical
//! `mqk_config::secrets::resolve_discord_webhooks` authority — the same
//! config/env-name parsing `ResolvedSecrets`/`resolve_secrets_for_mode` uses,
//! never a second parser. `config/defaults/base.yaml`'s `discord.channels.*`
//! names the env var per channel; the canonical defaults are
//! `DISCORD_WEBHOOK_PAPER`, `DISCORD_WEBHOOK_LIVE`, `DISCORD_WEBHOOK_BACKTEST`,
//! `DISCORD_WEBHOOK_ALERTS`, `DISCORD_WEBHOOK_HEARTBEAT`, `DISCORD_WEBHOOK_C2`.
//! Each channel is independently optional — an unconfigured channel silently
//! no-ops for any notification routed to it. No delivery is attempted; no
//! error is returned.
//!
//! `DiscordNotifier::from_env()` (the production constructor — see
//! [`load_discord_config_json_from_env`]) additionally loads the operator's
//! own `/discord/channels/*` env-var-NAME mapping from the layered YAML
//! file(s) named by `MQK_DISCORD_CONFIG_PATH` (comma-separated, merge
//! order), when set — a custom channel NAME in that config is honored, not
//! silently discarded. `MQK_DISCORD_CONFIG_PATH` unset, unreadable, or
//! malformed is not an error: the canonical default env var NAMES apply,
//! same as before this bridge existed.
//!
//! # Channel routing
//!
//! - `notify_critical_alert` (DIS-01) and `notify_test_alert` → `alerts`
//!   channel (a test alert is meant to land where real fault alerts do, so
//!   an operator can confirm delivery works).
//! - `notify_operator_action` → `c2` channel (accepted operator control
//!   actions — arm/disarm/start/stop/halt — are exactly the
//!   command-and-control messages the `c2` channel exists for).
//! - `notify_trade_event` (DISCORD-TRADE-LIFECYCLE-ALERTS-01) and
//!   `notify_run_status` (DIS-02) → `paper`/`live`/`backtest` channel,
//!   selected by the payload's own `environment` label
//!   (`AppState::deployment_mode().as_api_label()`: `"paper"` →  paper,
//!   `"live-shadow"`/`"live-capital"` → live, `"backtest"` → backtest). An
//!   absent or unrecognized `environment` label routes nowhere (fail closed
//!   on channel selection — never guessed).
//! - `heartbeat` has no current caller in this daemon; the channel resolves
//!   but nothing routes to it yet (unwired, not ambiguous — there is no
//!   heartbeat notification to route).
//!
//! # Delivery contract
//!
//! - Primary daemon action/result completes before any `notify_*` call.
//! - HTTP 2xx (including 204 No Content) is delivery success.
//! - HTTP non-2xx is classified as a sanitized, best-effort delivery failure.
//! - Delivery failure is logged as `warn!` and swallowed — it does not propagate.
//! - A 3-second timeout caps worst-case latency impact on the calling handler.
//! - All methods are no-ops when their resolved channel is unconfigured.
//!
//! # Notification types
//!
//! - `notify_operator_action` — accepted operator control actions (arm/disarm/start/stop/halt).
//! - `notify_critical_alert` (DIS-01) — critical/warning fault conditions (halt, WS gap).
//! - `notify_run_status` (DIS-02) — paper-run lifecycle summaries (started/stopped/halted).

use std::time::Duration;

use mqk_config::secrets::ResolvedDiscordWebhooks;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Layered YAML config path(s) (comma-separated, merge order) carrying the
/// operator's `/discord/channels/*` env-var-NAME mapping — DISCORD-CHANNEL-
/// ROUTING-01's narrowest Discord-only startup bridge. This does NOT load
/// broker credentials or any other secret: `resolve_discord_webhooks` only
/// ever reads the `/discord/channels/*` pointers out of the resulting JSON,
/// and env var NAMES are not themselves secrets (the env vars they point at
/// hold the actual webhook URLs, read separately by `resolve_env`).
pub const DISCORD_CONFIG_PATH_ENV: &str = "MQK_DISCORD_CONFIG_PATH";

/// Load the config JSON `resolve_discord_webhooks` should resolve
/// `/discord/channels/*` against, from `MQK_DISCORD_CONFIG_PATH`.
///
/// Reuses the canonical `mqk_config::load_layered_yaml` loader — never a
/// second YAML parser. Absent, empty, unreadable, or malformed config
/// resolves to `Value::Null`, which `resolve_discord_webhooks` already
/// treats as "use the canonical default env var NAMES": Discord channel
/// configuration is always optional, so a missing or bad config path is
/// never a hard failure, only a fallback to defaults (logged at `warn!` when
/// the path was set but failed to load, so a operator typo is visible).
pub fn load_discord_config_json_from_env() -> serde_json::Value {
    let raw = match std::env::var(DISCORD_CONFIG_PATH_ENV) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return serde_json::Value::Null,
    };
    let paths: Vec<&str> = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if paths.is_empty() {
        return serde_json::Value::Null;
    }
    match mqk_config::load_layered_yaml(&paths) {
        Ok(loaded) => loaded.config_json,
        Err(error) => {
            warn!(
                %error,
                path_env = DISCORD_CONFIG_PATH_ENV,
                "discord_config: failed to load MQK_DISCORD_CONFIG_PATH; falling back to \
                 canonical default Discord channel env var NAMES"
            );
            serde_json::Value::Null
        }
    }
}

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
    /// True when at least one Discord channel is configured.
    pub configured: bool,
    /// True when delivery will be attempted on the next notify call whose
    /// resolved channel is configured. Identical to `configured` — present
    /// for clarity on operator surfaces.
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

/// Best-effort Discord webhook notifier (DISCORD-CHANNEL-ROUTING-01).
///
/// Holds one independently-optional webhook URL per canonical channel (see
/// module docs for routing). Cloneable — `reqwest::Client` wraps an `Arc`
/// internally so cloning is cheap. Constructed once at daemon startup and
/// shared via `AppState`.
#[derive(Clone)]
pub struct DiscordNotifier {
    channels: ResolvedDiscordWebhooks,
    client: Option<reqwest::Client>,
}

/// Which canonical Discord channel a notification is routed to. See module
/// docs for the routing table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Paper,
    Live,
    Backtest,
    Alerts,
    #[allow(dead_code)] // No current caller routes here — see module docs.
    Heartbeat,
    C2,
}

impl DiscordNotifier {
    /// Construct directly from an already-resolved per-channel webhook set
    /// (e.g. `mqk_config::secrets::resolve_discord_webhooks`'s output). The
    /// one real constructor every other constructor on this type delegates
    /// to — never a second, independent way to build a `DiscordNotifier`.
    pub fn from_resolved_webhooks(channels: ResolvedDiscordWebhooks) -> Self {
        let any_configured = channels.paper.is_some()
            || channels.live.is_some()
            || channels.backtest.is_some()
            || channels.alerts.is_some()
            || channels.heartbeat.is_some()
            || channels.c2.is_some();
        let client = any_configured.then(reqwest::Client::new);
        Self { channels, client }
    }

    /// Construct from environment via the canonical
    /// `mqk_config::secrets::resolve_discord_webhooks` authority — the same
    /// config/env-name parsing `ResolvedSecrets` uses, never a second
    /// parser. When `MQK_DISCORD_CONFIG_PATH` names one or more layered YAML
    /// config file(s) (see [`load_discord_config_json_from_env`]), the
    /// operator's actually-configured `/discord/channels/*` env-var-NAME
    /// mapping is loaded and honored; otherwise (unset, unreadable, or
    /// malformed) the resolver's own fallback-default env var NAMES
    /// (matching `config/defaults/base.yaml` exactly) apply, so the
    /// canonical `DISCORD_WEBHOOK_*` env vars are honored either way. Each
    /// channel independently silently no-ops when its env var is absent or
    /// empty.
    pub fn from_env() -> Self {
        Self::from_resolved_webhooks(mqk_config::secrets::resolve_discord_webhooks(
            &load_discord_config_json_from_env(),
        ))
    }

    /// Construct with a single URL applied to every channel. Used in tests
    /// and targeted wiring where per-channel routing is not the concern.
    pub fn from_url(url: impl Into<String>) -> Self {
        let url = url.into();
        Self::from_resolved_webhooks(ResolvedDiscordWebhooks {
            paper: Some(url.clone()),
            live: Some(url.clone()),
            backtest: Some(url.clone()),
            alerts: Some(url.clone()),
            heartbeat: Some(url.clone()),
            c2: Some(url),
        })
    }

    /// Explicit no-op instance — never attempts delivery on any channel.
    pub fn noop() -> Self {
        Self::from_resolved_webhooks(ResolvedDiscordWebhooks {
            paper: None,
            live: None,
            backtest: None,
            alerts: None,
            heartbeat: None,
            c2: None,
        })
    }

    /// Returns `true` when at least one channel is configured.
    pub fn is_configured(&self) -> bool {
        self.client.is_some()
    }

    /// Returns `true` when the `alerts` channel specifically is configured
    /// — the channel `notify_critical_alert` and `notify_test_alert` route
    /// to. Distinct from [`Self::is_configured`] (any channel) because a
    /// caller that specifically triggers a test-alert or critical-alert
    /// delivery needs to know whether *that* channel, not some other one,
    /// will actually deliver.
    pub fn is_alerts_channel_configured(&self) -> bool {
        self.channels.alerts.is_some()
    }

    /// Returns `true` when the `paper` channel specifically is configured
    /// — the channel `notify_trade_event`/`notify_run_status` route to for
    /// `environment == "paper"` (see module docs). Distinct from
    /// [`Self::is_configured`] (any channel): a caller asking whether Paper
    /// lifecycle notifications will actually be delivered must check the
    /// Paper channel specifically — e.g. `c2`/`live`-only configuration
    /// makes `is_configured()` true but Paper visibility is still not ready.
    pub fn is_paper_channel_configured(&self) -> bool {
        self.channels.paper.is_some()
    }

    /// Returns a redacted status snapshot — never includes any webhook URL.
    pub fn status(&self) -> DiscordNotifierStatus {
        let configured = self.is_configured();
        DiscordNotifierStatus {
            configured,
            delivery_enabled: configured,
        }
    }

    /// Resolve the webhook URL for `channel`, or `None` if that channel is
    /// unconfigured (or the notifier has no client at all).
    fn url_for(&self, channel: Channel) -> Option<(&String, &reqwest::Client)> {
        let client = self.client.as_ref()?;
        let url = match channel {
            Channel::Paper => self.channels.paper.as_ref(),
            Channel::Live => self.channels.live.as_ref(),
            Channel::Backtest => self.channels.backtest.as_ref(),
            Channel::Alerts => self.channels.alerts.as_ref(),
            Channel::Heartbeat => self.channels.heartbeat.as_ref(),
            Channel::C2 => self.channels.c2.as_ref(),
        }?;
        Some((url, client))
    }

    /// Select the `paper`/`live`/`backtest` channel from a payload's
    /// `environment` label (`AppState::deployment_mode().as_api_label()`).
    /// An absent or unrecognized label routes to no channel — fail closed
    /// on channel selection rather than guessing.
    fn channel_for_environment(environment: Option<&str>) -> Option<Channel> {
        match environment {
            Some("paper") => Some(Channel::Paper),
            Some("live-shadow") | Some("live-capital") => Some(Channel::Live),
            Some("backtest") => Some(Channel::Backtest),
            _ => None,
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
        let Some((url, client)) = self.url_for(Channel::Alerts) else {
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
        let Some((url, client)) = self.url_for(Channel::C2) else {
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
        let Some((url, client)) = self.url_for(Channel::Alerts) else {
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
        let Some(channel) = Self::channel_for_environment(payload.environment.as_deref()) else {
            return;
        };
        let Some((url, client)) = self.url_for(channel) else {
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
        let Some(channel) = Self::channel_for_environment(payload.environment.as_deref()) else {
            return;
        };
        let Some((url, client)) = self.url_for(channel) else {
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
