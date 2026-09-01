//! PATCH S1 — Secrets & Webhook Routing
//!
//! This module is the **single source of truth** for runtime secret resolution.
//!
//! # Contract
//! - Config YAML stores only **env var NAMES** (e.g., `"ALPACA_API_KEY_PAPER"`).
//! - At startup, callers invoke `resolve_secrets_for_mode()` once.
//! - The returned `ResolvedSecrets` is passed into constructors; never scatter
//!   `std::env::var` calls across the codebase.
//! - `Debug` impls on all secret-containing structs **redact** values.
//! - Error messages reference the env var **NAME**, never the value.
//!
//! # Mode-aware enforcement
//! - `LIVE`:     broker api_key + api_secret + TwelveData api_key are **required**.
//! - `PAPER`:    broker api_key + api_secret are **required**.
//! - `BACKTEST`: no keys required — all optional.
//!
//! Discord webhooks are always **optional** in every mode.
//!
//! # Extension points
//! - Add future broker adapters: extend `ResolvedSecrets` with `fmp_api_key`, etc.
//! - Add future Discord channels: extend `ResolvedDiscordWebhooks` and `base.yaml`.
//! - Add future providers: extend `base.yaml` under `data.providers.<name>` and
//!   add the corresponding field here.

use anyhow::{bail, Result};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Discord webhook URLs resolved from the environment.
///
/// Every channel is optional — a missing channel silently produces `None`.
/// **Values are redacted in `Debug` output.**
#[derive(Clone)]
pub struct ResolvedDiscordWebhooks {
    /// Paper-trading notifications.
    pub paper: Option<String>,
    /// Live-trading notifications.
    pub live: Option<String>,
    /// Backtest summary notifications.
    pub backtest: Option<String>,
    /// Risk / integrity alerts (kill-switch, disarm, reject-storm, …).
    pub alerts: Option<String>,
    /// Periodic heartbeat pings.
    pub heartbeat: Option<String>,
    /// C2 (command-and-control) operator messages.
    pub c2: Option<String>,
    // Extension point: add new channels here (e.g., `pub audit: Option<String>`).
    // Consumers that don't need the new channel are unaffected.
}

impl std::fmt::Debug for ResolvedDiscordWebhooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print webhook URLs — they carry auth in the URL itself.
        f.debug_struct("ResolvedDiscordWebhooks")
            .field("paper", &self.paper.as_ref().map(|_| "<REDACTED>"))
            .field("live", &self.live.as_ref().map(|_| "<REDACTED>"))
            .field("backtest", &self.backtest.as_ref().map(|_| "<REDACTED>"))
            .field("alerts", &self.alerts.as_ref().map(|_| "<REDACTED>"))
            .field("heartbeat", &self.heartbeat.as_ref().map(|_| "<REDACTED>"))
            .field("c2", &self.c2.as_ref().map(|_| "<REDACTED>"))
            .finish()
    }
}

/// All runtime-resolved secrets for one engine instantiation.
///
/// Built **once** at startup via [`resolve_secrets_for_mode`].
/// Pass to constructors. Do **not** scatter `std::env::var` calls elsewhere.
/// **Values are redacted in `Debug` output.**
#[derive(Clone)]
pub struct ResolvedSecrets {
    /// Broker (Alpaca) API key. `None` if the named env var was absent or empty.
    pub broker_api_key: Option<String>,
    /// Broker (Alpaca) API secret. `None` if the named env var was absent or empty.
    pub broker_api_secret: Option<String>,
    /// TwelveData API key. `None` if the named env var was absent or empty.
    pub twelvedata_api_key: Option<String>,
    /// Discord webhook URLs keyed by logical channel.
    pub discord: ResolvedDiscordWebhooks,
    // Extension point: add `pub fmp_api_key: Option<String>` here when FMP is wired.
}

impl std::fmt::Debug for ResolvedSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedSecrets")
            .field(
                "broker_api_key",
                &self.broker_api_key.as_ref().map(|_| "<REDACTED>"),
            )
            .field(
                "broker_api_secret",
                &self.broker_api_secret.as_ref().map(|_| "<REDACTED>"),
            )
            .field(
                "twelvedata_api_key",
                &self.twelvedata_api_key.as_ref().map(|_| "<REDACTED>"),
            )
            .field("discord", &self.discord)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Env var names extracted from the config JSON.
/// These are the NAMES stored in YAML — not values.
struct SecretEnvNames {
    broker_api_key_var: String,
    broker_api_secret_var: String,
    twelvedata_api_key_var: String,
}

/// Default Discord webhook-URL env var NAMES, matching
/// `config/defaults/base.yaml`'s `discord.channels.*` values exactly. Used
/// by [`resolve_discord_webhooks`] whenever a channel's pointer is absent
/// from `config_json` — the same fallback-default pattern already used for
/// `broker_api_key_var` / `broker_api_secret_var` / `twelvedata_api_key_var`
/// above, never a silent `None`.
const DISCORD_ENV_PAPER_DEFAULT: &str = "DISCORD_WEBHOOK_PAPER";
const DISCORD_ENV_LIVE_DEFAULT: &str = "DISCORD_WEBHOOK_LIVE";
const DISCORD_ENV_BACKTEST_DEFAULT: &str = "DISCORD_WEBHOOK_BACKTEST";
const DISCORD_ENV_ALERTS_DEFAULT: &str = "DISCORD_WEBHOOK_ALERTS";
const DISCORD_ENV_HEARTBEAT_DEFAULT: &str = "DISCORD_WEBHOOK_HEARTBEAT";
const DISCORD_ENV_C2_DEFAULT: &str = "DISCORD_WEBHOOK_C2";

/// Read a non-empty string value at `pointer` from a JSON config.
/// Returns `None` if the pointer is absent, the value is not a string, or it
/// is blank after trimming.
fn read_str_at(config: &Value, pointer: &str) -> Option<String> {
    let s = config.pointer(pointer)?.as_str()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Resolve a named environment variable.
/// Returns `None` if the variable is unset or its value is blank.
/// Never returns the value in an error path — callers report the NAME only.
fn resolve_env(var_name: &str) -> Option<String> {
    match std::env::var(var_name) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

/// Parse env var names from the loaded config JSON.
/// Falls back to well-known defaults if a pointer is absent.
fn parse_env_names(config_json: &Value) -> SecretEnvNames {
    SecretEnvNames {
        broker_api_key_var: read_str_at(config_json, "/broker/keys_env/api_key")
            .unwrap_or_else(|| "MQK_BROKER_API_KEY".to_string()),

        broker_api_secret_var: read_str_at(config_json, "/broker/keys_env/api_secret")
            .unwrap_or_else(|| "MQK_BROKER_API_SECRET".to_string()),

        // Extension point: add fmp: read_str_at(config_json, "/data/providers/fmp/api_key_env")
        twelvedata_api_key_var: read_str_at(config_json, "/data/providers/twelvedata/api_key_env")
            .unwrap_or_else(|| "TWELVEDATA_API_KEY".to_string()),
    }
}

/// Resolve Discord webhook URLs from the environment for every logical
/// channel — the narrowest Discord-only slice of the config/env-name
/// parsing authority.
///
/// Reads each channel's env var NAME from `config_json` (`/discord/channels/
/// <channel>`), falling back to the canonical default NAME
/// (`config/defaults/base.yaml`'s literal values, e.g. `DISCORD_WEBHOOK_PAPER`)
/// when the pointer is absent — exactly the same fallback-default pattern
/// [`parse_env_names`] already uses for broker/TwelveData. This means a
/// caller with no loaded config JSON at all (`&Value::Null`) still resolves
/// correctly against the canonical env var names.
///
/// [`resolve_secrets_for_mode`] calls this function for its own `discord`
/// field — there is exactly one Discord env-name-resolution implementation.
/// No mode is consulted here: Discord webhooks are always optional,
/// independent of LIVE/PAPER/BACKTEST enforcement.
pub fn resolve_discord_webhooks(config_json: &Value) -> ResolvedDiscordWebhooks {
    let paper_var = read_str_at(config_json, "/discord/channels/paper")
        .unwrap_or_else(|| DISCORD_ENV_PAPER_DEFAULT.to_string());
    let live_var = read_str_at(config_json, "/discord/channels/live")
        .unwrap_or_else(|| DISCORD_ENV_LIVE_DEFAULT.to_string());
    let backtest_var = read_str_at(config_json, "/discord/channels/backtest")
        .unwrap_or_else(|| DISCORD_ENV_BACKTEST_DEFAULT.to_string());
    let alerts_var = read_str_at(config_json, "/discord/channels/alerts")
        .unwrap_or_else(|| DISCORD_ENV_ALERTS_DEFAULT.to_string());
    let heartbeat_var = read_str_at(config_json, "/discord/channels/heartbeat")
        .unwrap_or_else(|| DISCORD_ENV_HEARTBEAT_DEFAULT.to_string());
    let c2_var = read_str_at(config_json, "/discord/channels/c2")
        .unwrap_or_else(|| DISCORD_ENV_C2_DEFAULT.to_string());

    ResolvedDiscordWebhooks {
        paper: resolve_env(&paper_var),
        live: resolve_env(&live_var),
        backtest: resolve_env(&backtest_var),
        alerts: resolve_env(&alerts_var),
        heartbeat: resolve_env(&heartbeat_var),
        c2: resolve_env(&c2_var),
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve all secrets from the environment for the given `mode` string.
///
/// `mode` is case-insensitive: `"LIVE"`, `"PAPER"`, or `"BACKTEST"`.
///
/// # Enforcement
/// | Mode      | Required                                   |
/// |-----------|--------------------------------------------|
/// | LIVE      | broker api_key, broker api_secret, TwelveData api_key |
/// | PAPER     | broker api_key, broker api_secret          |
/// | BACKTEST  | nothing (all optional)                     |
///
/// Discord webhooks are always optional in every mode.
///
/// # Errors
/// Returns `Err` with the **env var NAME** of the first missing required
/// variable. The actual value is never mentioned.
pub fn resolve_secrets_for_mode(config_json: &Value, mode: &str) -> Result<ResolvedSecrets> {
    let names = parse_env_names(config_json);
    let mode_upper = mode.trim().to_ascii_uppercase();

    let broker_api_key = resolve_env(&names.broker_api_key_var);
    let broker_api_secret = resolve_env(&names.broker_api_secret_var);
    let twelvedata_api_key = resolve_env(&names.twelvedata_api_key_var);

    match mode_upper.as_str() {
        "LIVE" => {
            if broker_api_key.is_none() {
                bail!(
                    "SECRETS_MISSING mode=LIVE: required env var '{}' \
                     (broker api_key) is not set or empty",
                    names.broker_api_key_var,
                );
            }
            if broker_api_secret.is_none() {
                bail!(
                    "SECRETS_MISSING mode=LIVE: required env var '{}' \
                     (broker api_secret) is not set or empty",
                    names.broker_api_secret_var,
                );
            }
            if twelvedata_api_key.is_none() {
                bail!(
                    "SECRETS_MISSING mode=LIVE: required env var '{}' \
                     (TwelveData api_key) is not set or empty",
                    names.twelvedata_api_key_var,
                );
            }
        }
        "PAPER" => {
            if broker_api_key.is_none() {
                bail!(
                    "SECRETS_MISSING mode=PAPER: required env var '{}' \
                     (broker api_key) is not set or empty",
                    names.broker_api_key_var,
                );
            }
            if broker_api_secret.is_none() {
                bail!(
                    "SECRETS_MISSING mode=PAPER: required env var '{}' \
                     (broker api_secret) is not set or empty",
                    names.broker_api_secret_var,
                );
            }
        }
        "BACKTEST" => {
            // No required secrets in BACKTEST — provider / broker keys are optional.
        }
        other => {
            bail!(
                "SECRETS_UNKNOWN_MODE: unrecognised mode '{}'; \
                 expected one of: LIVE | PAPER | BACKTEST",
                other,
            );
        }
    }

    let discord = resolve_discord_webhooks(config_json);

    Ok(ResolvedSecrets {
        broker_api_key,
        broker_api_secret,
        twelvedata_api_key,
        discord,
    })
}
