//! Alpaca account entitlement evidence and admission.
//!
//! Source: `GET /v2/account` (Alpaca Trading API). The provider documents
//! `id` and `status` as the only required fields; every other entitlement
//! field below is optional on the wire, so an absent or wrongly typed field is
//! *unknown*, never an implicit grant.
//!
//! Provider contract used (field -> meaning):
//! - `id`                      account UUID (provider account identity)
//! - `status`                  account status; only `ACTIVE` is accepted here
//! - `trading_blocked`         `true` => account cannot place orders
//! - `account_blocked`         `true` => account activity is prohibited
//! - `trade_suspended_by_user` `true` => user setting blocks order placement
//! - `crypto_status`           crypto account status; only `ACTIVE` is accepted
//! - `options_trading_level`   effective options level, 0 = disabled
//! - `shorting_enabled`        whether the account may short
//!
//! Evidence is one input to admission; it never widens what the adapter
//! supports (`supports_asset_class`) or what the operator enabled. Admission
//! is the conjunction of all of them, evaluated at the gateway.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use mqk_execution::{AccountEntitlementRefusal, AssetClass};
use serde::Serialize;
use serde_json::Value;

pub const ACCOUNT_ENTITLEMENT_SCHEMA_VERSION: u32 = 1;

/// The only `status` / `crypto_status` value accepted as "usable".
const ACTIVE: &str = "ACTIVE";

/// Minimum effective `options_trading_level` for a multi-leg spread (provider
/// level 3 = spreads/straddles; 2 = long call/put only).
pub const OPTIONS_SPREAD_MIN_LEVEL: i64 = 3;

/// Typed projection of the entitlement-relevant fields of `GET /v2/account`.
/// `None` means absent or wrongly typed on the wire (unknown).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountEntitlementEvidence {
    pub schema_version: u32,
    pub provider_account_id: Option<String>,
    pub status: Option<String>,
    pub trading_blocked: Option<bool>,
    pub account_blocked: Option<bool>,
    pub trade_suspended_by_user: Option<bool>,
    pub crypto_status: Option<String>,
    pub options_trading_level: Option<i64>,
    pub shorting_enabled: Option<bool>,
}

fn str_field(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

impl AccountEntitlementEvidence {
    /// Pure projection of a `GET /v2/account` JSON body. A field that is
    /// absent or has the wrong JSON type becomes `None`.
    pub fn from_account_json(account: &Value) -> Self {
        Self {
            schema_version: ACCOUNT_ENTITLEMENT_SCHEMA_VERSION,
            provider_account_id: str_field(account, "id").map(|s| s.to_ascii_lowercase()),
            status: str_field(account, "status"),
            trading_blocked: account.get("trading_blocked").and_then(Value::as_bool),
            account_blocked: account.get("account_blocked").and_then(Value::as_bool),
            trade_suspended_by_user: account
                .get("trade_suspended_by_user")
                .and_then(Value::as_bool),
            crypto_status: str_field(account, "crypto_status"),
            options_trading_level: account.get("options_trading_level").and_then(Value::as_i64),
            shorting_enabled: account.get("shorting_enabled").and_then(Value::as_bool),
        }
    }
}

fn refuse(code: &'static str, detail: impl Into<String>) -> AccountEntitlementRefusal {
    AccountEntitlementRefusal::new(code, detail)
}

/// Attach the evidence provenance a refusal was decided on: the observed
/// provider account id and observation time, and the run's bound account
/// (`none` when no binding exists). Ids only; never credentials or balances.
fn with_provenance(
    refusal: AccountEntitlementRefusal,
    obs: &AccountEvidenceObservation,
    bound_account: Option<&str>,
) -> AccountEntitlementRefusal {
    refusal
        .with_context(
            "observed_provider_account_id",
            obs.evidence
                .provider_account_id
                .as_deref()
                .unwrap_or("unavailable"),
        )
        .with_context("observed_at_utc", obs.observed_at_utc.to_rfc3339())
        .with_context("bound_provider_account_id", bound_account.unwrap_or("none"))
}

fn require_flag_clear(
    flag: Option<bool>,
    name: &'static str,
    blocked_code: &'static str,
) -> Result<(), AccountEntitlementRefusal> {
    match flag {
        None => Err(refuse(
            "account_entitlement_field_unavailable",
            format!("GET /v2/account did not carry a boolean `{name}`"),
        )),
        Some(true) => Err(refuse(
            blocked_code,
            format!("provider reports {name}=true"),
        )),
        Some(false) => Ok(()),
    }
}

/// Provider-level admission of a NEW order for `asset_class` (`None` = no
/// class information, e.g. replace: base account permission only).
///
/// No exemption exists for risk-reducing orders: the provider refuses every
/// order from a blocked account, so MQD refuses it before sending.
pub fn evaluate_account_entitlement(
    evidence: &AccountEntitlementEvidence,
    asset_class: Option<AssetClass>,
) -> Result<(), AccountEntitlementRefusal> {
    if evidence.provider_account_id.is_none() {
        return Err(refuse(
            "account_identity_unavailable",
            "GET /v2/account did not carry a usable account id",
        ));
    }
    match evidence.status.as_deref() {
        None => {
            return Err(refuse(
                "account_entitlement_field_unavailable",
                "GET /v2/account did not carry a string `status`",
            ))
        }
        Some(ACTIVE) => {}
        Some(other) => {
            return Err(refuse(
                "account_status_not_active",
                format!("provider account status is {other:?}, not {ACTIVE:?}"),
            ))
        }
    }
    require_flag_clear(
        evidence.trading_blocked,
        "trading_blocked",
        "account_trading_blocked",
    )?;
    require_flag_clear(
        evidence.account_blocked,
        "account_blocked",
        "account_blocked",
    )?;
    require_flag_clear(
        evidence.trade_suspended_by_user,
        "trade_suspended_by_user",
        "account_trade_suspended_by_user",
    )?;
    match asset_class {
        None | Some(AssetClass::Equity) => Ok(()),
        Some(AssetClass::Crypto) => match evidence.crypto_status.as_deref() {
            Some(ACTIVE) => Ok(()),
            Some(other) => Err(refuse(
                "account_crypto_not_active",
                format!("provider crypto_status is {other:?}, not {ACTIVE:?}"),
            )),
            None => Err(refuse(
                "account_entitlement_field_unavailable",
                "GET /v2/account did not carry a string `crypto_status`",
            )),
        },
        Some(AssetClass::Option) => match evidence.options_trading_level {
            Some(level) if level >= 1 => Ok(()),
            Some(level) => Err(refuse(
                "account_options_not_enabled",
                format!("provider options_trading_level is {level}"),
            )),
            None => Err(refuse(
                "account_entitlement_field_unavailable",
                "GET /v2/account did not carry an integer `options_trading_level`",
            )),
        },
        Some(other) => Err(refuse(
            "account_entitlement_unsupported_asset_class",
            format!("no Alpaca account entitlement rule exists for {other:?}"),
        )),
    }
}

/// One observation of the account entitlement fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountEvidenceObservation {
    pub evidence: AccountEntitlementEvidence,
    pub observed_at_utc: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct CellState {
    observation: Option<AccountEvidenceObservation>,
    pinned_provider_account_id: Option<String>,
}

/// Shared, process-local holder of the latest account observation and the
/// provider account identity this run is bound to. Cloning shares state.
/// Nothing here survives a restart: a restarted process has no evidence, so
/// it cannot admit orders until a fresh `GET /v2/account` is observed.
#[derive(Debug, Clone, Default)]
pub struct AccountEvidenceCell(Arc<Mutex<CellState>>);

impl AccountEvidenceCell {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a fresh observation. `observed_at_utc` is caller-injected.
    pub fn observe(&self, evidence: AccountEntitlementEvidence, observed_at_utc: DateTime<Utc>) {
        if let Ok(mut s) = self.0.lock() {
            s.observation = Some(AccountEvidenceObservation {
                evidence,
                observed_at_utc,
            });
        }
    }

    /// Bind admission to one provider account id (lowercased). Admission
    /// REQUIRES this binding and refuses any observation naming a different
    /// account. It is established only by the run-start binding step after the
    /// execution adapter's own probe proved the account (and, for Paper, the
    /// durable registry accepted it); an observation by any other fetcher never
    /// binds. A blank id clears the binding.
    pub fn pin_provider_account_id(&self, provider_account_id: &str) {
        let id = provider_account_id.trim().to_ascii_lowercase();
        if let Ok(mut s) = self.0.lock() {
            s.pinned_provider_account_id = (!id.is_empty()).then_some(id);
        }
    }

    /// Drop the run/account binding (called at the start of every run, before
    /// the probe, so a binding never survives into a run that failed to prove
    /// its own).
    pub fn clear_run_binding(&self) {
        if let Ok(mut s) = self.0.lock() {
            s.pinned_provider_account_id = None;
        }
    }

    pub fn latest(&self) -> Option<AccountEvidenceObservation> {
        self.0.lock().ok().and_then(|s| s.observation.clone())
    }

    pub fn pinned_provider_account_id(&self) -> Option<String> {
        self.0
            .lock()
            .ok()
            .and_then(|s| s.pinned_provider_account_id.clone())
    }

    /// Admission against the latest observation: unavailable, stale (older
    /// than `freshness_bound` or timestamped in the future), unbound (no
    /// validated run/account binding), identity-drifted or non-entitled
    /// evidence refuses. An absent binding is never permission.
    pub fn admit(
        &self,
        now: DateTime<Utc>,
        freshness_bound: Duration,
        asset_class: Option<AssetClass>,
    ) -> Result<(), AccountEntitlementRefusal> {
        let (obs, pinned) = {
            let s = self.0.lock().map_err(|_| {
                refuse(
                    "account_evidence_unavailable",
                    "account evidence state is poisoned",
                )
            })?;
            (s.observation.clone(), s.pinned_provider_account_id.clone())
        };
        let obs = obs.ok_or_else(|| {
            refuse(
                "account_evidence_unavailable",
                "no GET /v2/account observation has been recorded in this process",
            )
        })?;
        let provenance = |r: AccountEntitlementRefusal| with_provenance(r, &obs, pinned.as_deref());
        let age = now.signed_duration_since(obs.observed_at_utc);
        if age < Duration::zero() || age > freshness_bound {
            return Err(provenance(refuse(
                "account_evidence_stale",
                format!(
                    "account observation at {} is {}s old (bound {}s)",
                    obs.observed_at_utc.to_rfc3339(),
                    age.num_seconds(),
                    freshness_bound.num_seconds()
                ),
            )));
        }
        if let Some(pinned) = pinned.as_deref() {
            if obs.evidence.provider_account_id.as_deref() != Some(pinned) {
                return Err(provenance(refuse(
                    "account_identity_drift",
                    format!(
                        "observed provider account {:?} differs from the account this run is bound to ({pinned:?})",
                        obs.evidence.provider_account_id
                    ),
                )));
            }
        }
        // Report the provider's own denial even when no binding exists yet
        // (the most informative refusal); an entitled account with no binding
        // is still refused below.
        evaluate_account_entitlement(&obs.evidence, asset_class).map_err(provenance)?;
        if pinned.is_none() {
            return Err(provenance(refuse(
                "account_binding_absent",
                "no validated run/account binding exists; fresh account evidence alone does not authorize orders",
            )));
        }
        Ok(())
    }
}

/// `true` for refusal codes that are a definite provider/account denial (the
/// provider reported a blocked, non-active or non-entitled account), as
/// opposed to evidence that is merely unavailable, stale or malformed.
pub fn is_provider_denial_code(code: &str) -> bool {
    matches!(
        code,
        "account_status_not_active"
            | "account_trading_blocked"
            | "account_blocked"
            | "account_trade_suspended_by_user"
            | "account_crypto_not_active"
            | "account_options_not_enabled"
            | "account_identity_drift"
    )
}

/// Operator-readable readiness of the broker-account entitlement for one
/// asset class, derived from the same `admit` logic the gateway enforces.
///
/// `state`: `not_observed` (no observation yet in this process), `entitled`,
/// `denied` (definite provider/identity denial), `stale`, `unbound` (fresh
/// evidence but no validated run/account binding), `unknown` (observation
/// present but fields unavailable/malformed). Only `entitled` means orders
/// would be admitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AccountEntitlementReadiness {
    pub state: String,
    pub asset_class: String,
    pub code: Option<String>,
    pub detail: Option<String>,
    pub provider_account_id: Option<String>,
    pub observed_at_utc: Option<DateTime<Utc>>,
}

impl AccountEvidenceCell {
    pub fn readiness(
        &self,
        now: DateTime<Utc>,
        freshness_bound: Duration,
        asset_class: AssetClass,
    ) -> AccountEntitlementReadiness {
        let latest = self.latest();
        let (state, code, detail) = match self.admit(now, freshness_bound, Some(asset_class)) {
            Ok(()) => ("entitled", None, None),
            Err(r) => {
                let state = match r.code.as_str() {
                    "account_evidence_unavailable" if latest.is_none() => "not_observed",
                    "account_evidence_stale" => "stale",
                    "account_binding_absent" => "unbound",
                    c if is_provider_denial_code(c) => "denied",
                    _ => "unknown",
                };
                (state, Some(r.code), Some(r.detail))
            }
        };
        AccountEntitlementReadiness {
            state: state.to_string(),
            asset_class: format!("{asset_class:?}").to_ascii_lowercase(),
            code,
            detail,
            provider_account_id: latest
                .as_ref()
                .and_then(|o| o.evidence.provider_account_id.clone()),
            observed_at_utc: latest.map(|o| o.observed_at_utc),
        }
    }
}
