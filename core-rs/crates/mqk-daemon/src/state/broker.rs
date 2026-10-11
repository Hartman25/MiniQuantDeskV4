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
    BrokerAssetShortablePreflight, BrokerAssetShortablePreflightFetcher, BrokerFillActivityFetcher,
    BrokerSnapshotFetcher, CryptoFeeActivityFetcher, OptionLifecycleActivityFetcher,
    WsGapFillFetcher, ALPACA_BASE_URL_PAPER_ENV, ALPACA_KEY_LIVE_ENV, ALPACA_KEY_PAPER_ENV,
    ALPACA_SECRET_LIVE_ENV, ALPACA_SECRET_PAPER_ENV,
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

impl DaemonBroker {
    /// Bind an Alpaca adapter to the shared account-evidence cell (records
    /// every `GET /v2/account`, and admits orders only against fresh
    /// evidence). The Paper variant has no provider account: unchanged.
    pub(crate) fn with_account_evidence(
        self,
        cell: &mqk_broker_alpaca::AccountEvidenceCell,
    ) -> Self {
        match self {
            Self::Alpaca(adapter) => Self::Alpaca(adapter.with_account_evidence(
                cell.clone(),
                super::account_entitlement_freshness_bound(),
            )),
            other => other,
        }
    }
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
    /// M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01: forwards to the wrapped
    /// adapter's declared capability. This override is load-bearing, not
    /// cosmetic — without it, `DaemonBroker`'s own trait impl would fall
    /// back to `BrokerAdapter`'s Equity-only default and silently negate
    /// whatever capability the inner adapter declares (today: Alpaca is
    /// Equity-only pending IR-B1-03 / the Alpaca-crypto completion commit),
    /// since `DaemonBroker` (not the inner adapter) is the concrete type
    /// `BrokerGateway<B, ..>` is instantiated with in the real daemon.
    fn supports_asset_class(&self, asset_class: mqk_execution::AssetClass) -> bool {
        match self {
            Self::Paper(b) => b.supports_asset_class(asset_class),
            Self::Alpaca(b) => b.supports_asset_class(asset_class),
        }
    }

    /// Load-bearing forward: without it the wrapper would fall back to the
    /// trait's admit-all default and silently drop the Alpaca account
    /// entitlement authority at the real gateway seam.
    fn admit_account_entitlement(
        &self,
        asset_class: Option<mqk_execution::AssetClass>,
    ) -> Result<(), mqk_execution::AccountEntitlementRefusal> {
        match self {
            Self::Paper(b) => b.admit_account_entitlement(asset_class),
            Self::Alpaca(b) => b.admit_account_entitlement(asset_class),
        }
    }

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

#[cfg(test)]
mod daemon_broker_capability_tests {
    use super::*;
    use mqk_execution::AssetClass;

    /// M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01: proves `DaemonBroker`'s
    /// `supports_asset_class` override genuinely forwards to the wrapped
    /// adapter's declared capability set (today Equity-only for Alpaca,
    /// pending IR-B1-03) rather than silently falling back to the trait's
    /// Equity-only default for an unrelated reason — the specific defect
    /// this override exists to prevent is masking the forwarding bug behind
    /// a coincidentally-identical capability set, so this also proves the
    /// non-Equity refusals forward correctly.
    #[test]
    fn alpaca_variant_forwards_declared_capability() {
        let broker = DaemonBroker::Alpaca(AlpacaBrokerAdapter::paper(
            "test-key".to_string(),
            "test-secret".to_string(),
        ));
        assert!(broker.supports_asset_class(AssetClass::Equity));
        // IR-B1-03: Alpaca does not yet advertise Crypto — see
        // AlpacaBrokerAdapter::supports_asset_class.
        assert!(!broker.supports_asset_class(AssetClass::Crypto));
        assert!(!broker.supports_asset_class(AssetClass::Future));
        assert!(!broker.supports_asset_class(AssetClass::Option));
        assert!(!broker.supports_asset_class(AssetClass::Forex));
    }

    fn account(extra: serde_json::Value) -> mqk_broker_alpaca::AccountEntitlementEvidence {
        let mut a = serde_json::json!({
            "id": "904837e3-3b76-47ec-b432-046db621571b",
            "status": "ACTIVE",
            "trading_blocked": false,
            "account_blocked": false,
            "trade_suspended_by_user": false
        });
        for (k, v) in extra.as_object().unwrap() {
            a[k] = v.clone();
        }
        mqk_broker_alpaca::AccountEntitlementEvidence::from_account_json(&a)
    }

    /// The enum wrapper must forward the account-entitlement authority: if
    /// it fell back to the trait's admit-all default, an unbound adapter
    /// would be admitted here.
    #[test]
    fn alpaca_variant_forwards_account_entitlement_authority() {
        use mqk_execution::AssetClass;
        let paper = || AlpacaBrokerAdapter::paper("k".to_string(), "s".to_string());
        let code = |b: &DaemonBroker| {
            b.admit_account_entitlement(Some(AssetClass::Equity))
                .map_err(|r| r.code)
        };
        assert_eq!(
            code(&DaemonBroker::Alpaca(paper())),
            Err("account_evidence_not_bound".to_string())
        );
        let cell = mqk_broker_alpaca::AccountEvidenceCell::new();
        let bound = DaemonBroker::Alpaca(paper()).with_account_evidence(&cell);
        assert_eq!(
            code(&bound),
            Err("account_evidence_unavailable".to_string())
        );
        cell.observe(account(serde_json::json!({})), chrono::Utc::now());
        // fresh + entitled but no run/account binding: refused
        assert_eq!(code(&bound), Err("account_binding_absent".to_string()));
        cell.pin_provider_account_id("904837e3-3b76-47ec-b432-046db621571b");
        assert_eq!(code(&bound), Ok(()));
        cell.observe(
            account(serde_json::json!({"trading_blocked": true})),
            chrono::Utc::now(),
        );
        assert_eq!(code(&bound), Err("account_trading_blocked".to_string()));
        // The in-process Paper broker has no provider account.
        assert_eq!(
            code(&DaemonBroker::Paper(LockedPaperBroker::new())),
            Ok(())
        );
    }

    struct Armed;
    impl mqk_execution::IntegrityGate for Armed {
        fn is_armed(&self) -> bool {
            true
        }
    }
    struct Allow;
    impl mqk_execution::RiskGate for Allow {
        fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
            mqk_execution::RiskDecision::Allow
        }
    }
    struct Clean;
    impl mqk_execution::ReconcileGate for Clean {
        fn is_clean(&self) -> bool {
            true
        }
    }

    fn order() -> mqk_execution::BrokerSubmitRequest {
        mqk_execution::BrokerSubmitRequest {
            order_id: "ord-acct-01".to_string(),
            symbol: "AAPL".to_string(),
            side: mqk_execution::Side::Buy,
            quantity: mqk_execution::QtyMicros::from_whole_units(1).unwrap(),
            order_type: "market".to_string(),
            limit_price: None,
            time_in_force: "day".to_string(),
            asset_class: mqk_execution::AssetClass::Equity,
        }
    }

    /// Real gateway over the real DaemonBroker::Alpaca: an unentitled account
    /// is refused before any HTTP; an entitled one passes every gate and
    /// reaches the adapter (nothing listens on the port, so the adapter
    /// reports a connection-level Transport error, proving it was invoked).
    #[test]
    fn real_gateway_refuses_unentitled_account_before_any_http() {
        use mqk_execution::{
            wiring::build_gateway, BrokerError, GateRefusal, OutboxClaimToken, SubmitError,
        };
        let cell = mqk_broker_alpaca::AccountEvidenceCell::new();
        let adapter = AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            api_key_id: "k".to_string(),
            api_secret_key: "s".to_string(),
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        });
        let gw = build_gateway(
            DaemonBroker::Alpaca(adapter).with_account_evidence(&cell),
            Armed,
            Allow,
            Clean,
        );
        let claim = OutboxClaimToken::for_test(1, "ord-acct-01");
        cell.pin_provider_account_id("904837e3-3b76-47ec-b432-046db621571b");

        for (label, evidence, want) in [
            ("never observed", None, "account_evidence_unavailable"),
            (
                "blocked",
                Some(account(serde_json::json!({"account_blocked": true}))),
                "account_blocked",
            ),
        ] {
            if let Some(e) = evidence {
                cell.observe(e, chrono::Utc::now());
            }
            match gw.submit(&claim, order()) {
                Err(SubmitError::Gate(GateRefusal::AccountEntitlementRefused(r))) => {
                    assert_eq!(r.code, want, "{label}");
                }
                other => panic!("{label}: expected entitlement refusal, got {other:?}"),
            }
        }

        cell.observe(account(serde_json::json!({})), chrono::Utc::now());
        match gw.submit(&claim, order()) {
            Err(SubmitError::Broker(BrokerError::Transport { .. })) => {}
            other => panic!("entitled order must reach the adapter, got {other:?}"),
        }
    }

    /// LockedPaperBroker does not override the trait default; the Paper
    /// variant must forward to that unchanged Equity-only behavior.
    #[test]
    fn paper_variant_stays_equity_only() {
        let broker = DaemonBroker::Paper(LockedPaperBroker::new());
        assert!(broker.supports_asset_class(AssetClass::Equity));
        assert!(!broker.supports_asset_class(AssetClass::Crypto));
    }
}

pub(crate) fn alpaca_base_url_for_mode(
    deployment_mode: DeploymentMode,
    paper_base_url_override: Option<&str>,
) -> Result<String, RuntimeLifecycleError> {
    match deployment_mode {
        DeploymentMode::Paper => {
            let Some(value) = paper_base_url_override
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(format!("https://{}", mqk_broker_alpaca::ALPACA_PAPER_API_HOST));
            };
            // Paper credentials must never be pointed at a non-Paper host
            // (e.g. the live API) by an environment override: the Paper
            // deployment label would then be attached to a Live account.
            // Loopback is admitted solely for hermetic in-process mocks.
            if mqk_broker_alpaca::is_alpaca_paper_base_url(value)
                || mqk_broker_alpaca::is_loopback_base_url(value)
            {
                Ok(value.to_owned())
            } else {
                Err(RuntimeLifecycleError::service_unavailable(
                    "runtime.start_refused.alpaca_paper_base_url_not_paper",
                    "ALPACA_PAPER_BASE_URL does not target the Alpaca Paper API host; \
                     refusing to bind a Paper deployment to a non-Paper endpoint",
                ))
            }
        }
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
            // LIVE-SECRETS-CONSOLIDATION-01: resolve through mqk_config::secrets
            // (the documented single source of truth for env-var-named secret
            // resolution) instead of a bare std::env::var — never the broader
            // resolve_secrets_for_mode() bundle, which also requires a
            // TwelveData key in LIVE mode, unrelated to broker construction.
            let key_id = mqk_config::secrets::resolve_env(key_env).ok_or_else(|| {
                RuntimeLifecycleError::service_unavailable(
                    "runtime.start_refused.alpaca_creds_missing",
                    format!("broker 'alpaca' requires {key_env} environment variable"),
                )
            })?;
            let secret = mqk_config::secrets::resolve_env(secret_env).ok_or_else(|| {
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
                    crypto_capability_enabled: false,
                    options_mleg_capability_enabled: false,
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
    /// Deployment-level Crypto time-in-force policy, resolved once at daemon
    /// start (never re-read from the environment). The strategy->decision
    /// construction seam emits this value directly; see
    /// `crypto_execution_policy`.
    pub crypto_time_in_force: super::crypto_execution_policy::CryptoTimeInForceConfig,
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
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        }),
    )))
}

// ---------------------------------------------------------------------------
// B6 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): production
// crypto fee-activity fetcher
// ---------------------------------------------------------------------------

/// Provider-account authority for one Alpaca credential set: resolved from the
/// authenticated `GET /v2/account` id (never the API key id), cached after the
/// first success for the process lifetime (an account id does not change under
/// a fixed credential), and never cached on failure.
struct AlpacaAccountAuthoritySource {
    deployment_mode: DeploymentMode,
    cached: std::sync::Mutex<Option<mqk_db::BrokerAccountAuthority>>,
}

impl AlpacaAccountAuthoritySource {
    fn new(deployment_mode: DeploymentMode) -> Self {
        Self {
            deployment_mode,
            cached: std::sync::Mutex::new(None),
        }
    }

    fn resolve(
        &self,
        adapter: &AlpacaBrokerAdapter,
    ) -> Result<mqk_db::BrokerAccountAuthority, String> {
        let mut guard = self
            .cached
            .lock()
            .map_err(|_| "alpaca account authority cache poisoned".to_string())?;
        if let Some(authority) = guard.as_ref() {
            return Ok(authority.clone());
        }
        let provider_account_id = adapter
            .fetch_provider_account_id()
            .map_err(|e| e.to_string())?;
        let authority = mqk_db::BrokerAccountAuthority::new(
            "alpaca",
            &provider_account_id,
            self.deployment_mode.as_api_label(),
        )
        .map_err(|e| e.to_string())?;
        *guard = Some(authority.clone());
        Ok(authority)
    }
}

/// Thin newtype wrapping `AlpacaBrokerAdapter` that implements
/// `CryptoFeeActivityFetcher`. Only the read-only day-end fee-activity fetch
/// path is reachable through this type. Order submission, cancel, replace,
/// and `fetch_events` are not exposed.
struct AlpacaCryptoFeeActivityFetcher(AlpacaBrokerAdapter, AlpacaAccountAuthoritySource);

impl CryptoFeeActivityFetcher for AlpacaCryptoFeeActivityFetcher {
    fn fetch_fee_activities_since(
        &self,
        activity_type: &str,
        after_id: Option<&str>,
    ) -> Result<Vec<mqk_broker_alpaca::types::AlpacaFeeActivity>, String> {
        self.0
            .fetch_fee_activities_since(activity_type, after_id)
            .map_err(|e| e.to_string())
    }

    fn broker_account_authority(&self) -> Result<mqk_db::BrokerAccountAuthority, String> {
        self.1.resolve(&self.0)
    }
}

/// B6: construct a production crypto fee-activity fetcher.
///
/// Returns `Some(fetcher)` only when `broker_kind == Some(BrokerKind::Alpaca)`
/// and the matching credentials are present in the environment — same
/// fail-closed shape as `build_fill_activity_fetcher_from_env`. This fetches
/// read-only account-activity history (never an order), so it is
/// constructed regardless of the separate Crypto trading-capability flag
/// (D2/B4); nothing in the daemon calls it automatically today (no
/// scheduled ingestion loop is wired by this patch) — it exists as a real,
/// callable, tested production path, not a live consumer.
pub(super) fn build_crypto_fee_activity_fetcher_from_env(
    broker_kind: Option<BrokerKind>,
    deployment_mode: DeploymentMode,
) -> Option<Arc<dyn CryptoFeeActivityFetcher>> {
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

    Some(Arc::new(AlpacaCryptoFeeActivityFetcher(
        AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url,
            api_key_id: key_id,
            api_secret_key: secret,
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        }),
        AlpacaAccountAuthoritySource::new(deployment_mode),
    )))
}

// ---------------------------------------------------------------------------
// D1 correction (V4-M5-M8-INDEPENDENT-REVIEW-CORRECTION-01): production
// options-lifecycle activity fetcher
// ---------------------------------------------------------------------------

/// Thin newtype wrapping `AlpacaBrokerAdapter` that implements
/// `OptionLifecycleActivityFetcher`. Only the read-only options-lifecycle
/// account-activity fetch path is reachable through this type. Order
/// submission, cancel, replace, and `fetch_events` are not exposed.
struct AlpacaOptionLifecycleActivityFetcher(AlpacaBrokerAdapter, AlpacaAccountAuthoritySource);

impl OptionLifecycleActivityFetcher for AlpacaOptionLifecycleActivityFetcher {
    fn fetch_option_lifecycle_activities_since(
        &self,
        activity_type: &str,
        after_id: Option<&str>,
    ) -> Result<Vec<mqk_broker_alpaca::types::AlpacaOptionLifecycleActivity>, String> {
        self.0
            .fetch_option_lifecycle_activities_since(activity_type, after_id)
            .map_err(|e| e.to_string())
    }

    fn broker_account_authority(&self) -> Result<mqk_db::BrokerAccountAuthority, String> {
        self.1.resolve(&self.0)
    }
}

/// D1 correction: construct a production options-lifecycle activity
/// fetcher.
///
/// Returns `Some(fetcher)` only when `broker_kind == Some(BrokerKind::Alpaca)`
/// and the matching credentials are present in the environment — same
/// fail-closed shape as `build_crypto_fee_activity_fetcher_from_env`. This
/// fetches read-only account-activity history (never an order); Alpaca
/// options trading capability does not exist in this codebase yet, so this
/// fetcher exists independently of any such flag. It is driven only by the
/// default-off `option_lifecycle_poll` task (`MQK_OPTION_LIFECYCLE_POLL_INTERVAL_SECS`)
/// and reads through the order-admission fences.
pub(super) fn build_option_lifecycle_activity_fetcher_from_env(
    broker_kind: Option<BrokerKind>,
    deployment_mode: DeploymentMode,
) -> Option<Arc<dyn OptionLifecycleActivityFetcher>> {
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

    Some(Arc::new(AlpacaOptionLifecycleActivityFetcher(
        AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url,
            api_key_id: key_id,
            api_secret_key: secret,
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        }),
        AlpacaAccountAuthoritySource::new(deployment_mode),
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
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
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
    account_evidence: &mqk_broker_alpaca::AccountEvidenceCell,
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

    Some(Arc::new(AlpacaSnapshotFetcher(
        AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url,
            api_key_id: key_id,
            api_secret_key: secret,
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        })
        .with_account_evidence(
            account_evidence.clone(),
            super::account_entitlement_freshness_bound(),
        ),
    )))
}

// ---------------------------------------------------------------------------
// SHORT-SIDE-EXTERNAL-SIGNAL-WIRING-01: read-only asset shortability preflight
// ---------------------------------------------------------------------------

/// Thin newtype wrapping `AlpacaBrokerAdapter` for read-only asset metadata.
///
/// Only `GET /v2/assets/{symbol}` is exposed through this type. Order submit,
/// cancel, replace, and event fetch methods are not reachable.
struct AlpacaAssetShortablePreflightFetcher {
    adapter: AlpacaBrokerAdapter,
    source: &'static str,
}

impl BrokerAssetShortablePreflightFetcher for AlpacaAssetShortablePreflightFetcher {
    fn fetch_asset_shortable_preflight(
        &self,
        symbol: &str,
    ) -> Result<Option<BrokerAssetShortablePreflight>, String> {
        match self.adapter.fetch_asset(symbol) {
            Ok(asset) => Ok(Some(BrokerAssetShortablePreflight {
                symbol: asset.symbol.to_ascii_uppercase(),
                asset_class: "equity".to_string(),
                tradable: asset.tradable,
                shortable: asset.shortable,
                marginable: asset.marginable,
                easy_to_borrow: asset.easy_to_borrow,
                source: self.source.to_string(),
            })),
            Err(mqk_execution::BrokerError::Reject { code, .. }) if code == "404" => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }
}

pub(super) fn build_asset_shortable_preflight_fetcher_from_env(
    broker_kind: Option<BrokerKind>,
    deployment_mode: DeploymentMode,
) -> Option<Arc<dyn BrokerAssetShortablePreflightFetcher>> {
    match broker_kind {
        Some(BrokerKind::Alpaca) => {}
        _ => return None,
    }

    let (key_env, secret_env, source) = match deployment_mode {
        DeploymentMode::Paper => (
            ALPACA_KEY_PAPER_ENV,
            ALPACA_SECRET_PAPER_ENV,
            "alpaca_paper_asset",
        ),
        _ => (
            ALPACA_KEY_LIVE_ENV,
            ALPACA_SECRET_LIVE_ENV,
            "alpaca_live_asset",
        ),
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

    Some(Arc::new(AlpacaAssetShortablePreflightFetcher {
        adapter: AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url,
            api_key_id: key_id,
            api_secret_key: secret,
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        }),
        source,
    }))
}
