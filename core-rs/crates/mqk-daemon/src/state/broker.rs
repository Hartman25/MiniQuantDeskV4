//! DaemonBroker enum-dispatch seam and broker construction helpers.
//!
//! Contains: DaemonBroker, alpaca_base_url_for_mode, build_daemon_broker,
//! DeploymentReadiness, RuntimeSelection, StrategyFleetEntry.

use std::fmt;

use mqk_broker_alpaca::{AlpacaBrokerAdapter, AlpacaConfig};
use mqk_broker_paper::LockedPaperBroker;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerInvokeToken,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
};

use super::types::{BrokerKind, DeploymentMode, RuntimeLifecycleError};
use super::{
    ALPACA_BASE_URL_PAPER_ENV, ALPACA_KEY_LIVE_ENV, ALPACA_KEY_PAPER_ENV, ALPACA_SECRET_LIVE_ENV,
    ALPACA_SECRET_PAPER_ENV,
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
        Some(BrokerKind::Paper) => Ok(DaemonBroker::Paper(LockedPaperBroker::new())),
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
