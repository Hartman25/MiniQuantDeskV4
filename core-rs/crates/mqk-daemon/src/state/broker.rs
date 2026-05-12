//! DaemonBroker enum-dispatch seam and broker construction helpers.
//!
//! Contains: DaemonBroker, alpaca_base_url_for_mode, build_daemon_broker,
//! AlpacaFillActivityFetcher, build_fill_activity_fetcher_from_env,
//! DeploymentReadiness, RuntimeSelection, StrategyFleetEntry.

use std::fmt;
use std::sync::Arc;

use mqk_broker_alpaca::{AlpacaBrokerAdapter, AlpacaConfig};
use mqk_broker_paper::LockedPaperBroker;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerInvokeToken,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
};

use super::types::{BrokerKind, DeploymentMode, RuntimeLifecycleError};
use super::{
    BrokerFillActivityFetcher, BrokerSnapshotFetcher, WsGapFillFetcher, ALPACA_BASE_URL_PAPER_ENV,
    ALPACA_KEY_LIVE_ENV, ALPACA_KEY_PAPER_ENV, ALPACA_SECRET_LIVE_ENV, ALPACA_SECRET_PAPER_ENV,
};

// ---------------------------------------------------------------------------
// DaemonBroker — enum-dispatch seam (AP-02)
// ---------------------------------------------------------------------------

/// Broker dispatch seam for the daemon execution orchestrator.
pub(crate) enum DaemonBroker {
    Paper(LockedPaperBroker),
    /// AP-06/AP-07/AP-08: Alpaca v2 REST broker.
    Alpaca(AlpacaBrokerAdapter),
}

impl fmt::Debug for DaemonBroker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Paper(_) => f.write_str("DaemonBroker::Paper"),
            Self::Alpaca(_) => f.write_str("DaemonBroker::Alpaca"),
        }
    }
}

impl BrokerAdapter for DaemonBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        match self {
            Self::Paper(b) => b.submit_order(req, token),
            Self::Alpaca(b) => b.submit_order(req, token),
        }
    }

    fn cancel_order(
        &self,
        order_id: &str,
        token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
        match self {
            Self::Paper(b) => b.cancel_order(order_id, token),
            Self::Alpaca(b) => b.cancel_order(order_id, token),
        }
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
        match self {
            Self::Paper(b) => b.replace_order(req, token),
            Self::Alpaca(b) => b.replace_order(req, token),
        }
    }

    fn fetch_events(
        &self,
        cursor: Option<&str>,
        token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
        match self {
            Self::Paper(b) => b.fetch_events(cursor, token),
            Self::Alpaca(b) => b.fetch_events(cursor, token),
        }
    }
}

pub(crate) fn alpaca_base_url_for_mode(
    deployment_mode: DeploymentMode,
    paper_base_url_override: Option<&str>,
) -> Result<String, RuntimeLifecycleError> {
    match deployment_mode {
        DeploymentMode::Paper => Ok(paper_base_url_override
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "https://paper-api.alpaca.markets".to_string())),
        DeploymentMode::LiveShadow | DeploymentMode::LiveCapital => {
            Ok("https://api.alpaca.markets".to_string())
        }
        DeploymentMode::Backtest => Err(RuntimeLifecycleError::service_unavailable(
            "runtime.start_refused.alpaca_mode_not_wired",
            format!(
                "broker 'alpaca' is not wired for deployment mode '{}'; refusing start fail-closed",
                deployment_mode.as_api_label()
            ),
        )),
    }
}

pub(crate) fn build_daemon_broker(
    broker_kind: Option<BrokerKind>,
    deployment_mode: DeploymentMode,
) -> Result<DaemonBroker, RuntimeLifecycleError> {
    match broker_kind {
        // BRK-10: LockedPaperBroker is not the canonical paper-trading execution path.
        // It is a bar-driven in-process fill engine (testkit only).  No bar-feed is
        // wired in the daemon runtime; orders would be accepted but never filled.
        // The authoritative paper path is Paper+Alpaca (MQK_DAEMON_ADAPTER_ID=alpaca),
        // which routes through paper-api.alpaca.markets.
        // This arm is fail-closed so the daemon cannot accidentally construct a broker
        // that looks live but silently drops all fills.
        Some(BrokerKind::Paper) => Err(RuntimeLifecycleError::service_unavailable(
            "runtime.start_refused.paper_broker_not_execution_path",
            "broker 'paper' (LockedPaperBroker) is not the canonical paper-trading execution \
             path: the in-process fill engine has no market-data source wired in the daemon \
             runtime and cannot produce real fills — \
             set MQK_DAEMON_ADAPTER_ID=alpaca to route through the Alpaca paper-trading \
             endpoint (paper-api.alpaca.markets)",
        )),
        Some(BrokerKind::Alpaca) => {
            // ENV-TRUTH-01: credentials are mode-specific to match .env.local.example.
            // Paper path: ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER (paper-api.alpaca.markets)
            // Live path:  ALPACA_API_KEY_LIVE  / ALPACA_API_SECRET_LIVE  (api.alpaca.markets)
            let (key_env, secret_env) = match deployment_mode {
                DeploymentMode::Paper => (ALPACA_KEY_PAPER_ENV, ALPACA_SECRET_PAPER_ENV),
                _ => (ALPACA_KEY_LIVE_ENV, ALPACA_SECRET_LIVE_ENV),
            };
            let paper_base_url_override = match deployment_mode {
                DeploymentMode::Paper => std::env::var(ALPACA_BASE_URL_PAPER_ENV).ok(),
                _ => None,
            };
            let base_url =
                alpaca_base_url_for_mode(deployment_mode, paper_base_url_override.as_deref())?;
            let key_id = std::env::var(key_env).map_err(|_| {
                RuntimeLifecycleError::service_unavailable(
                    "runtime.start_refused.alpaca_creds_missing",
                    format!("broker 'alpaca' requires {key_env} environment variable"),
                )
            })?;
            let secret = std::env::var(secret_env).map_err(|_| {
                RuntimeLifecycleError::service_unavailable(
                    "runtime.start_refused.alpaca_creds_missing",
                    format!("broker 'alpaca' requires {secret_env} environment variable"),
                )
            })?;
            Ok(DaemonBroker::Alpaca(AlpacaBrokerAdapter::new(
                AlpacaConfig {
                    base_url,
                    api_key_id: key_id,
                    api_secret_key: secret,
                },
            )))
        }
        None => Err(RuntimeLifecycleError::service_unavailable(
            "runtime.start_refused.broker_unrecognised",
            "unrecognised broker adapter; cannot construct execution broker",
        )),
    }
}

#[derive(Clone, Debug)]
pub struct DeploymentReadiness {
    pub start_allowed: bool,
    pub blocker: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RuntimeSelection {
    pub deployment_mode: DeploymentMode,
    /// Parsed broker implementation kind; `None` when the adapter-id string is
    /// unrecognised (treated as fail-closed by `deployment_mode_readiness`).
    pub broker_kind: Option<BrokerKind>,
    /// Raw adapter identifier string (e.g. `"paper"`, `"alpaca"`).
    pub adapter_id: String,
    pub run_config_hash: String,
    pub readiness: DeploymentReadiness,
}

// ---------------------------------------------------------------------------
// StrategyFleetEntry
// ---------------------------------------------------------------------------

/// A single strategy entry in the daemon's configured fleet.
#[derive(Debug, Clone)]
pub struct StrategyFleetEntry {
    pub strategy_id: String,
}

// ---------------------------------------------------------------------------
// BROKER-FILL-REST-PRODUCTION-WIRING-01: production fill-activity fetcher
// ---------------------------------------------------------------------------

/// Thin newtype wrapping `AlpacaBrokerAdapter` that implements `BrokerFillActivityFetcher`.
///
/// Only the read-only fill-activity fetch path is reachable through this type.
/// Order submission, cancel, replace, and `fetch_events` are not exposed.
struct AlpacaFillActivityFetcher(AlpacaBrokerAdapter);

impl BrokerFillActivityFetcher for AlpacaFillActivityFetcher {
    fn fetch_fill_activities_for_order(
        &self,
        broker_order_id: &str,
    ) -> Result<Vec<mqk_broker_alpaca::types::AlpacaOrderActivity>, String> {
        self.0
            .fetch_fill_activities_for_order(broker_order_id)
            .map_err(|e| e.to_string())
    }
}

/// BROKER-FILL-REST-PRODUCTION-WIRING-01: construct a production fill-activity fetcher.
///
/// Returns `Some(fetcher)` only when `broker_kind == Some(BrokerKind::Alpaca)` and
/// the matching credentials are present in the environment.  Returns `None` for all
/// other broker kinds or when any credential env var is absent — the repair route
/// will respond with `recovery_unavailable` rather than panicking or guessing.
///
/// Credential and base-URL selection mirrors `build_daemon_broker` exactly so
/// the fetcher targets the same Alpaca endpoint as the execution broker.
pub(super) fn build_fill_activity_fetcher_from_env(
    broker_kind: Option<BrokerKind>,
    deployment_mode: DeploymentMode,
) -> Option<Arc<dyn BrokerFillActivityFetcher>> {
    match broker_kind {
        Some(BrokerKind::Alpaca) => {}
        _ => return None,
    }

    let (key_env, secret_env) = match deployment_mode {
        DeploymentMode::Paper => (ALPACA_KEY_PAPER_ENV, ALPACA_SECRET_PAPER_ENV),
        _ => (ALPACA_KEY_LIVE_ENV, ALPACA_SECRET_LIVE_ENV),
    };
    let paper_override = match deployment_mode {
        DeploymentMode::Paper => std::env::var(ALPACA_BASE_URL_PAPER_ENV).ok(),
        _ => None,
    };
    let base_url = match alpaca_base_url_for_mode(deployment_mode, paper_override.as_deref()) {
        Ok(u) => u,
        Err(_) => return None, // e.g. Backtest mode — not wired for Alpaca
    };
    let key_id = match std::env::var(key_env) {
        Ok(v) => v,
        Err(_) => return None, // credentials absent — fail-closed
    };
    let secret = match std::env::var(secret_env) {
        Ok(v) => v,
        Err(_) => return None,
    };

    Some(Arc::new(AlpacaFillActivityFetcher(
        AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url,
            api_key_id: key_id,
            api_secret_key: secret,
        }),
    )))
}

// ---------------------------------------------------------------------------
// BRK-GAP-REST-RECOVERY-01: production account-wide gap fill fetcher
// ---------------------------------------------------------------------------

/// Thin newtype wrapping `AlpacaBrokerAdapter` that implements `WsGapFillFetcher`.
///
/// Only the read-only account-wide fill-activities-since path is reachable
/// through this type.  Order submission, cancel, replace, and `fetch_events`
/// are not exposed.
struct AlpacaWsGapFillFetcher(AlpacaBrokerAdapter);

impl WsGapFillFetcher for AlpacaWsGapFillFetcher {
    fn fetch_fills_since(
        &self,
        since_activity_id: Option<&str>,
    ) -> Result<Vec<mqk_broker_alpaca::types::AlpacaOrderActivity>, String> {
        self.0
            .fetch_fill_activities_since(since_activity_id)
            .map_err(|e| e.to_string())
    }
}

/// BRK-GAP-REST-RECOVERY-01: construct a production account-wide gap fill fetcher.
///
/// Returns `Some(fetcher)` only when `broker_kind == Some(BrokerKind::Alpaca)` and
/// the matching credentials are present in the environment.  Returns `None` for all
/// other broker kinds or when any credential env var is absent — the repair route
/// will respond with `recovery_unavailable` rather than panicking or guessing.
///
/// Credential and base-URL selection mirrors `build_fill_activity_fetcher_from_env`
/// exactly so the fetcher targets the same Alpaca endpoint as the execution broker.
pub(super) fn build_ws_gap_fill_fetcher_from_env(
    broker_kind: Option<BrokerKind>,
    deployment_mode: DeploymentMode,
) -> Option<Arc<dyn WsGapFillFetcher>> {
    match broker_kind {
        Some(BrokerKind::Alpaca) => {}
        _ => return None,
    }

    let (key_env, secret_env) = match deployment_mode {
        DeploymentMode::Paper => (ALPACA_KEY_PAPER_ENV, ALPACA_SECRET_PAPER_ENV),
        _ => (ALPACA_KEY_LIVE_ENV, ALPACA_SECRET_LIVE_ENV),
    };
    let paper_override = match deployment_mode {
        DeploymentMode::Paper => std::env::var(ALPACA_BASE_URL_PAPER_ENV).ok(),
        _ => None,
    };
    let base_url = match alpaca_base_url_for_mode(deployment_mode, paper_override.as_deref()) {
        Ok(u) => u,
        Err(_) => return None,
    };
    let key_id = match std::env::var(key_env) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let secret = match std::env::var(secret_env) {
        Ok(v) => v,
        Err(_) => return None,
    };

    Some(Arc::new(AlpacaWsGapFillFetcher(AlpacaBrokerAdapter::new(
        AlpacaConfig {
            base_url,
            api_key_id: key_id,
            api_secret_key: secret,
        },
    ))))
}

// ---------------------------------------------------------------------------
// BROKER-SNAPSHOT-REFRESH-FOR-BASELINE-01: on-demand broker snapshot fetcher
// ---------------------------------------------------------------------------

/// Thin newtype wrapping `AlpacaBrokerAdapter` that implements `BrokerSnapshotFetcher`.
///
/// Only the read-only broker snapshot fetch path is reachable through this type.
/// Order submission, cancel, replace, and `fetch_events` are not exposed.
struct AlpacaSnapshotFetcher(AlpacaBrokerAdapter);

impl BrokerSnapshotFetcher for AlpacaSnapshotFetcher {
    fn fetch_snapshot(&self) -> Result<mqk_schemas::BrokerSnapshot, String> {
        self.0
            .fetch_broker_snapshot(chrono::Utc::now())
            .map_err(|e| e.to_string())
    }
}

/// BROKER-SNAPSHOT-REFRESH-FOR-BASELINE-01: construct a production on-demand snapshot fetcher.
///
/// Returns `Some(fetcher)` only when `broker_kind == Some(BrokerKind::Alpaca)` and
/// the matching credentials are present in the environment.  Returns `None` for all
/// other broker kinds or when any credential env var is absent — the adoption route
/// will respond with `repair.broker_snapshot_refresh_unavailable` rather than panicking.
///
/// Credential and base-URL selection mirrors `build_fill_activity_fetcher_from_env`
/// exactly so the fetcher targets the same Alpaca endpoint as the execution broker.
pub(super) fn build_snapshot_fetcher_from_env(
    broker_kind: Option<BrokerKind>,
    deployment_mode: DeploymentMode,
) -> Option<Arc<dyn BrokerSnapshotFetcher>> {
    match broker_kind {
        Some(BrokerKind::Alpaca) => {}
        _ => return None,
    }

    let (key_env, secret_env) = match deployment_mode {
        DeploymentMode::Paper => (ALPACA_KEY_PAPER_ENV, ALPACA_SECRET_PAPER_ENV),
        _ => (ALPACA_KEY_LIVE_ENV, ALPACA_SECRET_LIVE_ENV),
    };
    let paper_override = match deployment_mode {
        DeploymentMode::Paper => std::env::var(ALPACA_BASE_URL_PAPER_ENV).ok(),
        _ => None,
    };
    let base_url = match alpaca_base_url_for_mode(deployment_mode, paper_override.as_deref()) {
        Ok(u) => u,
        Err(_) => return None,
    };
    let key_id = match std::env::var(key_env) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let secret = match std::env::var(secret_env) {
        Ok(v) => v,
        Err(_) => return None,
    };

    Some(Arc::new(AlpacaSnapshotFetcher(AlpacaBrokerAdapter::new(
        AlpacaConfig {
            base_url,
            api_key_id: key_id,
            api_secret_key: secret,
        },
    ))))
}
