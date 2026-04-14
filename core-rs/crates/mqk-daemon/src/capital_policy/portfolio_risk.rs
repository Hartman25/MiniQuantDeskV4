//! TV-04E — Portfolio risk outcome and evaluator (exposure / exhaustion / drift).

use std::path::Path;

// ---------------------------------------------------------------------------
// TV-04E — Portfolio risk outcome (exposure / exhaustion / drift)
// ---------------------------------------------------------------------------

/// Result of evaluating portfolio-level risk controls at the **signal
/// ingestion boundary**.
///
/// Called after Gate 1f (position sizing, TV-04C).  Adds per-signal portfolio
/// risk controls drawn from the per-strategy policy entry:
///
/// - **Exposure** (`max_order_exposure_pct_of_portfolio`): single-order
///   implied notional as a fraction of the portfolio cap.
/// - **Exhaustion** (`capital_exhaustion_reserve_usd`): order notional vs.
///   the required capital reserve floor.
/// - **Drift**: portfolio weight drift is never measurable at signal time
///   without runtime portfolio state; surfaced honestly as
///   [`PortfolioRiskOutcome::RiskUnverifiable`] when a risk cap is present
///   but the order is a market order.
///
/// # New per-strategy policy fields (`policy-v1` schema extension)
///
/// ```json
/// {
///   "strategy_id": "strat-momentum-001",
///   "max_order_exposure_pct_of_portfolio": 0.05,
///   "capital_exhaustion_reserve_usd": 2000
/// }
/// ```
///
/// Both fields are optional.  Absent fields mean the control is not enforced
/// for that strategy.
///
/// # Signal-safe variants
///
/// | Variant             | Meaning                                                          |
/// |---------------------|------------------------------------------------------------------|
/// | `NotConfigured`     | No policy path; gate not applicable.                             |
/// | `NoRiskConstraints` | Policy present but no risk fields for this strategy or no cap.  |
/// | `Authorized`        | All applicable risk checks passed.                               |
/// | `RiskUnverifiable`  | Market order or drift; implied notional uncomputable; honest pass.|
///
/// # Fail-closed variants
///
/// | Variant           | Meaning                                                          |
/// |-------------------|------------------------------------------------------------------|
/// | `ExposureDenied`  | Implied notional exceeds portfolio exposure cap.                 |
/// | `ExhaustionDenied`| Order notional exceeds (portfolio cap − exhaustion reserve).     |
/// | `PolicyInvalid`   | Policy configured but structurally invalid.                      |
/// | `Unavailable`     | Evaluator could not run (reserved).                              |
#[derive(Debug, Clone, PartialEq)]
pub enum PortfolioRiskOutcome {
    /// No policy path was configured (env var absent or empty).
    NotConfigured,

    /// Policy present but no portfolio-level cap or no risk fields for this
    /// strategy.  Gate not applicable; callers pass through.
    NoRiskConstraints,

    /// All applicable portfolio risk checks passed.
    Authorized {
        /// The strategy that was authorized.
        strategy_id: String,
    },

    /// Single-order implied notional exceeds the per-strategy portfolio
    /// exposure cap.  Fail-closed.
    ExposureDenied {
        /// Human-readable reason including computed values.
        reason: String,
    },

    /// Order notional would consume more capital than the exhaustion reserve
    /// allows.  Fail-closed.
    ExhaustionDenied {
        /// Human-readable reason including computed values.
        reason: String,
    },

    /// Market order: implied notional not computable without a price reference.
    ///
    /// Also covers portfolio drift, which is always unmeasurable at signal time
    /// without runtime portfolio state.  Honest pass-through (analogous to
    /// [`super::PositionSizingOutcome::SizingUnverifiable`] in TV-04C).
    RiskUnverifiable {
        /// Human-readable explanation.
        reason: String,
    },

    /// Policy path is configured but the file is unreadable or invalid.
    /// Fail-closed.
    PolicyInvalid {
        /// Human-readable reason.
        reason: String,
    },

    /// Evaluator could not run.  Reserved.  Fail-closed.
    Unavailable {
        /// Human-readable reason.
        reason: String,
    },
}

impl PortfolioRiskOutcome {
    /// Truth-state label for operator-visible surfaces.
    pub fn truth_state(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::NoRiskConstraints => "no_risk_constraints",
            Self::Authorized { .. } => "authorized",
            Self::ExposureDenied { .. } => "exposure_denied",
            Self::ExhaustionDenied { .. } => "exhaustion_denied",
            Self::RiskUnverifiable { .. } => "risk_unverifiable",
            Self::PolicyInvalid { .. } => "policy_invalid",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// Whether this outcome is safe to allow signal ingestion.
    ///
    /// `true` for `NotConfigured`, `NoRiskConstraints`, `Authorized`,
    /// `RiskUnverifiable`.
    /// `false` for `ExposureDenied`, `ExhaustionDenied`, `PolicyInvalid`,
    /// `Unavailable`.
    pub fn is_signal_safe(&self) -> bool {
        matches!(
            self,
            Self::NotConfigured
                | Self::NoRiskConstraints
                | Self::Authorized { .. }
                | Self::RiskUnverifiable { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Pure evaluator — portfolio risk (TV-04E)
// ---------------------------------------------------------------------------

/// Evaluate per-signal portfolio risk (exposure and capital exhaustion) against
/// the policy at `path`.
///
/// Pure: no env reads, no network, no DB.  Pass `None` to represent an
/// unconfigured path.
///
/// # Parameters
///
/// - `path` — path to `capital_allocation_policy.json`; `None` → `NotConfigured`
/// - `strategy_id` — the strategy emitting the signal
/// - `qty` — share quantity from the validated signal (must be > 0)
/// - `limit_price_micros` — limit price in 1/1_000_000 USD; `None` means
///   market order
///
/// # Validation contract
///
/// 1. `path` must be `Some` — otherwise `NotConfigured`.
/// 2. File must be readable and valid JSON — otherwise `PolicyInvalid`.
/// 3. `schema_version` must equal `"policy-v1"` — otherwise `PolicyInvalid`.
/// 4. `max_portfolio_notional_usd` absent or ≤ 0 → `NoRiskConstraints`
///    (no cap to anchor exposure ratio against).
/// 5. Strategy entry absent or has no risk fields → `NoRiskConstraints`.
/// 6. Market order (`limit_price_micros` is `None`) with any risk cap present
///    → `RiskUnverifiable` (honest pass-through; drift also falls here).
/// 7. Limit order with `max_order_exposure_pct_of_portfolio`:
///    - `implied_notional / portfolio_cap > pct_cap` → `ExposureDenied`.
/// 8. Limit order with `capital_exhaustion_reserve_usd`:
///    - `implied_notional > portfolio_cap − reserve` → `ExhaustionDenied`.
/// 9. All applicable checks pass → `Authorized`.
pub fn evaluate_portfolio_risk(
    path: Option<&Path>,
    strategy_id: &str,
    qty: i64,
    limit_price_micros: Option<i64>,
) -> PortfolioRiskOutcome {
    let path = match path {
        None => return PortfolioRiskOutcome::NotConfigured,
        Some(p) if p.as_os_str().is_empty() => return PortfolioRiskOutcome::NotConfigured,
        Some(p) => p,
    };

    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return PortfolioRiskOutcome::PolicyInvalid {
                reason: format!("cannot read '{}': {e}", path.display()),
            }
        }
    };

    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return PortfolioRiskOutcome::PolicyInvalid {
                reason: format!("invalid JSON in '{}': {e}", path.display()),
            }
        }
    };

    match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == super::CAPITAL_POLICY_SCHEMA_VERSION => {}
        _ => {
            return PortfolioRiskOutcome::PolicyInvalid {
                reason: "missing or unsupported schema_version".to_string(),
            }
        }
    }

    // Portfolio cap is required to anchor the exposure percentage check.
    // Absent or invalid cap means no exposure ratio can be computed.
    let max_portfolio_notional = match j.get("max_portfolio_notional_usd") {
        None => return PortfolioRiskOutcome::NoRiskConstraints,
        Some(v) => match v.as_f64() {
            Some(n) if n > 0.0 => n,
            _ => return PortfolioRiskOutcome::NoRiskConstraints,
        },
    };

    let budgets = match j.get("per_strategy_budgets").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return PortfolioRiskOutcome::NoRiskConstraints,
    };

    let entry = budgets.iter().find(|e| {
        e.get("strategy_id")
            .and_then(|v| v.as_str())
            .map(|s| s == strategy_id)
            .unwrap_or(false)
    });

    let entry = match entry {
        Some(e) => e,
        None => return PortfolioRiskOutcome::NoRiskConstraints,
    };

    let exposure_pct = entry
        .get("max_order_exposure_pct_of_portfolio")
        .and_then(|v| v.as_f64())
        .filter(|&n| n > 0.0);

    let exhaustion_reserve = entry
        .get("capital_exhaustion_reserve_usd")
        .and_then(|v| v.as_f64())
        .filter(|&n| n >= 0.0);

    // Neither risk field present for this strategy.
    if exposure_pct.is_none() && exhaustion_reserve.is_none() {
        return PortfolioRiskOutcome::NoRiskConstraints;
    }

    // Market order: implied notional not computable.
    // Also covers portfolio drift, which requires runtime portfolio state.
    let Some(limit_micros) = limit_price_micros else {
        return PortfolioRiskOutcome::RiskUnverifiable {
            reason: format!(
                "market order for strategy '{}': implied notional cannot be computed \
                 without a price reference; portfolio risk checks (exposure, exhaustion) \
                 bypassed — use limit orders when risk caps are active. \
                 Note: portfolio drift is not measurable at signal time without \
                 runtime portfolio state.",
                strategy_id
            ),
        };
    };

    let limit_price_usd = limit_micros as f64 / 1_000_000.0;
    let implied_notional = qty as f64 * limit_price_usd;

    // Exposure check: single-order notional must not exceed the per-strategy
    // portfolio exposure fraction.
    if let Some(pct_cap) = exposure_pct {
        let exposure_ratio = implied_notional / max_portfolio_notional;
        if exposure_ratio > pct_cap {
            return PortfolioRiskOutcome::ExposureDenied {
                reason: format!(
                    "strategy '{}' single-order exposure {:.2}% \
                     (${:.2} / ${:.2}) exceeds \
                     max_order_exposure_pct_of_portfolio={:.2}%; \
                     reduce qty or limit_price",
                    strategy_id,
                    exposure_ratio * 100.0,
                    implied_notional,
                    max_portfolio_notional,
                    pct_cap * 100.0,
                ),
            };
        }
    }

    // Exhaustion check: implied notional must not consume more than
    // (portfolio_cap - reserve).
    if let Some(reserve) = exhaustion_reserve {
        let available = max_portfolio_notional - reserve;
        if implied_notional > available {
            return PortfolioRiskOutcome::ExhaustionDenied {
                reason: format!(
                    "strategy '{}' implied notional ${:.2} exceeds available capital \
                     ${:.2} (portfolio_cap=${:.2} − reserve=${:.2}); \
                     capital exhaustion reserve would be breached",
                    strategy_id, implied_notional, available, max_portfolio_notional, reserve,
                ),
            };
        }
    }

    PortfolioRiskOutcome::Authorized {
        strategy_id: strategy_id.to_string(),
    }
}

/// Read [`super::ENV_CAPITAL_POLICY_PATH`] from the environment and evaluate
/// per-signal portfolio risk for `strategy_id`.
///
/// Returns `NotConfigured` when the env var is absent or empty.
pub fn evaluate_portfolio_risk_from_env(
    strategy_id: &str,
    qty: i64,
    limit_price_micros: Option<i64>,
) -> PortfolioRiskOutcome {
    let raw = std::env::var(super::ENV_CAPITAL_POLICY_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(raw.trim()))
    };
    evaluate_portfolio_risk(path.as_deref(), strategy_id, qty, limit_price_micros)
}
