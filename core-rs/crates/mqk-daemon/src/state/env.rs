//! Environment/config helpers for mqk-daemon state initialization.
//!
//! Contains: default_node_id, operator_auth_mode_from_env_values,
//! operator_auth_mode_from_env, runtime_selection_from_env_values,
//! runtime_selection_from_env, parse_deployment_mode,
//! deployment_mode_readiness, uptime_secs, spawn_heartbeat,
//! initial_reconcile_status.

use std::time::Duration;

use chrono::Utc;
use tokio::sync::broadcast;

use super::broker::{DeploymentReadiness, RuntimeSelection};
use super::types::{
    AlpacaWsContinuityState, BrokerKind, BusMsg, DeploymentMode, OperatorAuthMode,
    ReconcileStatusSnapshot,
};
use super::{
    DAEMON_ADAPTER_ID_ENV, DAEMON_DEPLOYMENT_MODE_ENV, DAEMON_RUN_CONFIG_HASH_PREFIX,
    DEFAULT_DAEMON_ADAPTER_ID, DEFAULT_DAEMON_DEPLOYMENT_MODE, DEV_ALLOW_NO_OPERATOR_TOKEN_ENV,
};

// ---------------------------------------------------------------------------
// Node ID
// ---------------------------------------------------------------------------

pub(crate) fn default_node_id(service: &str) -> String {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "UNKNOWN_HOST".to_string());
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "UNKNOWN_USER".to_string());
    format!("{service}|{host}|{user}|pid={}", std::process::id())
}

// ---------------------------------------------------------------------------
// Operator auth
// ---------------------------------------------------------------------------

pub fn operator_auth_mode_from_env_values(
    operator_token: Option<&str>,
    dev_allow_no_token: Option<&str>,
) -> OperatorAuthMode {
    if let Some(token) = operator_token
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        return OperatorAuthMode::TokenRequired(token.to_string());
    }

    #[cfg(debug_assertions)]
    {
        if dev_allow_no_token
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            return OperatorAuthMode::ExplicitDevNoToken;
        }
    }

    #[cfg(not(debug_assertions))]
    {
        let _ = dev_allow_no_token;
    }

    OperatorAuthMode::MissingTokenFailClosed
}

pub(crate) fn operator_auth_mode_from_env() -> OperatorAuthMode {
    let operator_token = std::env::var("MQK_OPERATOR_TOKEN").ok();
    let dev_allow_no_token = std::env::var(DEV_ALLOW_NO_OPERATOR_TOKEN_ENV).ok();
    operator_auth_mode_from_env_values(operator_token.as_deref(), dev_allow_no_token.as_deref())
}

// ---------------------------------------------------------------------------
// Runtime selection
// ---------------------------------------------------------------------------

pub(crate) fn runtime_selection_from_env_values(
    mode: Option<&str>,
    adapter_id: Option<&str>,
) -> RuntimeSelection {
    let deployment_mode = parse_deployment_mode(mode).unwrap_or_else(|| {
        parse_deployment_mode(Some(DEFAULT_DAEMON_DEPLOYMENT_MODE))
            .expect("default deployment mode must be valid")
    });
    let adapter = adapter_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DAEMON_ADAPTER_ID)
        .to_ascii_lowercase();

    let broker_kind = BrokerKind::parse(&adapter);

    let readiness = deployment_mode_readiness(deployment_mode, broker_kind);

    RuntimeSelection {
        deployment_mode,
        broker_kind,
        adapter_id: adapter,
        run_config_hash: format!(
            "{}-{}-{}-v1",
            DAEMON_RUN_CONFIG_HASH_PREFIX,
            deployment_mode.as_api_label(),
            if readiness.start_allowed {
                "ready"
            } else {
                "blocked"
            }
        ),
        readiness,
    }
}

pub(crate) fn runtime_selection_from_env() -> RuntimeSelection {
    let mode = std::env::var(DAEMON_DEPLOYMENT_MODE_ENV).ok();
    let adapter_id = std::env::var(DAEMON_ADAPTER_ID_ENV).ok();
    runtime_selection_from_env_values(mode.as_deref(), adapter_id.as_deref())
}

pub(crate) fn parse_deployment_mode(raw: Option<&str>) -> Option<DeploymentMode> {
    let value = raw?.trim().to_ascii_lowercase();
    match value.as_str() {
        "paper" => Some(DeploymentMode::Paper),
        "backtest" => Some(DeploymentMode::Backtest),
        "live-shadow" | "live_shadow" | "liveshadow" => Some(DeploymentMode::LiveShadow),
        "live-capital" | "live_capital" | "livecapital" | "live" => {
            Some(DeploymentMode::LiveCapital)
        }
        _ => None,
    }
}

/// Evaluate whether the (policy, broker) combination may be started.
pub(crate) fn deployment_mode_readiness(
    mode: DeploymentMode,
    broker_kind: Option<BrokerKind>,
) -> DeploymentReadiness {
    match (mode, broker_kind) {
        // ── Paper + Paper ─────────────────────────────────────────────────
        (DeploymentMode::Paper, Some(BrokerKind::Paper)) => DeploymentReadiness {
            start_allowed: true,
            blocker: None,
        },
        // ── Paper + Alpaca (AP-06) ────────────────────────────────────────
        (DeploymentMode::Paper, Some(BrokerKind::Alpaca)) => DeploymentReadiness {
            start_allowed: true,
            blocker: None,
        },
        // ── Paper + unrecognised adapter ──────────────────────────────────
        (DeploymentMode::Paper, None) => DeploymentReadiness {
            start_allowed: false,
            blocker: Some(
                "deployment mode 'paper' requires broker 'paper' or 'alpaca'; \
                 set MQK_DAEMON_ADAPTER_ID to a recognised broker adapter"
                    .to_string(),
            ),
        },
        // ── LiveShadow + Alpaca (AP-07) ───────────────────────────────────
        (DeploymentMode::LiveShadow, Some(BrokerKind::Alpaca)) => DeploymentReadiness {
            start_allowed: true,
            blocker: None,
        },
        // ── LiveShadow + Paper: explicitly blocked ────────────────────────
        (DeploymentMode::LiveShadow, Some(BrokerKind::Paper)) => DeploymentReadiness {
            start_allowed: false,
            blocker: Some(
                "deployment mode 'live-shadow' requires an external broker adapter; \
                 the paper fill engine cannot provide real market truth for shadow mode — \
                 set MQK_DAEMON_ADAPTER_ID=alpaca"
                    .to_string(),
            ),
        },
        // ── LiveShadow + unrecognised adapter ─────────────────────────────
        (DeploymentMode::LiveShadow, None) => DeploymentReadiness {
            start_allowed: false,
            blocker: Some(
                "deployment mode 'live-shadow' requires an external broker adapter; \
                 set MQK_DAEMON_ADAPTER_ID=alpaca"
                    .to_string(),
            ),
        },
        // ── LiveCapital + Alpaca (AP-08) ──────────────────────────────────
        (DeploymentMode::LiveCapital, Some(BrokerKind::Alpaca)) => DeploymentReadiness {
            start_allowed: true,
            blocker: None,
        },
        // ── LiveCapital + Paper: explicitly blocked ───────────────────────
        (DeploymentMode::LiveCapital, Some(BrokerKind::Paper)) => DeploymentReadiness {
            start_allowed: false,
            blocker: Some(
                "live-capital requires an external broker adapter; \
                 the paper fill engine cannot provide real market truth for capital execution — \
                 set MQK_DAEMON_ADAPTER_ID=alpaca"
                    .to_string(),
            ),
        },
        // ── LiveCapital + unrecognised adapter ────────────────────────────
        (DeploymentMode::LiveCapital, None) => DeploymentReadiness {
            start_allowed: false,
            blocker: Some(
                "live-capital requires an external broker adapter; \
                 set MQK_DAEMON_ADAPTER_ID=alpaca"
                    .to_string(),
            ),
        },
        // ── Backtest: unconditionally blocked ─────────────────────────────
        (DeploymentMode::Backtest, _) => DeploymentReadiness {
            start_allowed: false,
            blocker: Some(
                "deployment mode 'backtest' is not yet supported in the daemon runtime; \
                 refusing start fail-closed"
                    .to_string(),
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// Uptime / heartbeat / initial reconcile
// ---------------------------------------------------------------------------

/// Monotonically increasing uptime since first call (process lifetime).
pub fn uptime_secs() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

/// Spawn a background task that emits a heartbeat SSE every `interval`.
pub fn spawn_heartbeat(bus: broadcast::Sender<BusMsg>, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let ts = Utc::now().timestamp_millis(); // allow: ops-metadata — SSE heartbeat UI timestamp
            let _ = bus.send(BusMsg::Heartbeat { ts_millis: ts });
        }
    });
}

pub(crate) fn initial_reconcile_status() -> ReconcileStatusSnapshot {
    ReconcileStatusSnapshot {
        status: "unknown".to_string(),
        last_run_at: None,
        snapshot_watermark_ms: None,
        mismatched_positions: 0,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some(
            "reconcile truth not yet proven; fail closed until a fresh broker snapshot is accepted"
                .to_string(),
        ),
    }
}

// ── AP-05: derive initial WS continuity from broker kind ──────────────────
pub(crate) fn initial_ws_continuity_for_broker(
    broker_kind: Option<BrokerKind>,
) -> AlpacaWsContinuityState {
    match broker_kind {
        Some(BrokerKind::Alpaca) => AlpacaWsContinuityState::ColdStartUnproven,
        _ => AlpacaWsContinuityState::NotApplicable,
    }
}
