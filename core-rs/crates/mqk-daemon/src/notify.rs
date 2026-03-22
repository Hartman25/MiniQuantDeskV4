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
//! - Primary daemon action/result completes before `notify_operator_action` is called.
//! - Delivery failure is logged as `warn!` and swallowed — it does not propagate.
//! - A 3-second timeout caps worst-case latency impact on the calling handler.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

/// Environment variable name for the Discord webhook URL.
pub const DISCORD_WEBHOOK_URL_ENV: &str = "DISCORD_WEBHOOK_URL";

// ---------------------------------------------------------------------------
// Payload type
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
}
