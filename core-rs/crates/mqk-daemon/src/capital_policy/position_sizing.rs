//! TV-04C — Position sizing realism outcome and evaluator.

use std::path::Path;

// ---------------------------------------------------------------------------
// TV-04C — Position sizing realism outcome
// ---------------------------------------------------------------------------

/// Result of evaluating position sizing realism under broker/account limits
/// at the signal **ingestion boundary**.
///
/// Called after [`super::StrategyBudgetOutcome`] is signal-safe (Gate 1e passes).
/// Adds the explicit distinction: **budget-authorized ≠ size-executable**.
///
/// The evaluator reads `max_position_notional_usd` from the strategy's
/// `per_strategy_budgets` entry.  For limit orders the implied notional is
/// computable (`qty × limit_price`).  For market orders the notional is not
/// computable without a live price — [`PositionSizingOutcome::SizingUnverifiable`]
/// is returned: honest, not optimistically authorized.
///
/// # Signal-safe variants
///
/// | Variant              | Meaning                                                      |
/// |----------------------|--------------------------------------------------------------|
/// | `NotConfigured`      | No policy path configured; sizing gate not applicable.       |
/// | `NoSizingConstraint` | Entry exists but carries no `max_position_notional_usd`.    |
/// | `SizingAuthorized`   | Implied notional is within the policy cap (limit order).     |
/// | `SizingUnverifiable` | Market order: notional uncomputable; passed through honestly.|
///
/// # Fail-closed variants
///
/// | Variant        | Meaning                                                          |
/// |----------------|------------------------------------------------------------------|
/// | `SizingDenied` | Implied notional exceeds `max_position_notional_usd`.           |
/// | `PolicyInvalid`| File is present but unreadable / invalid.                       |
/// | `Unavailable`  | Evaluator could not run (reserved).                             |
#[derive(Debug, Clone, PartialEq)]
pub enum PositionSizingOutcome {
    /// No policy path was configured (env var absent or empty).
    ///
    /// Sizing gate is not applicable; callers pass through.
    NotConfigured,

    /// Policy path was configured but the file is unreadable or structurally
    /// invalid.  Always fail-closed.
    PolicyInvalid {
        /// Human-readable reason.
        reason: String,
    },

    /// Policy and budget entry exist but carry no `max_position_notional_usd`
    /// constraint.  Sizing is unconstrained by the current policy entry.
    NoSizingConstraint,

    /// Limit order: implied notional (`qty × limit_price`) is within the
    /// policy cap.  Explicit sizing authorization.
    SizingAuthorized {
        /// The strategy that was authorized.
        strategy_id: String,
        /// Computed implied notional in USD.
        implied_notional_usd: f64,
        /// The `max_position_notional_usd` cap from the policy entry.
        max_position_notional_usd: f64,
    },

    /// Market order: notional cannot be computed without a price reference.
    ///
    /// Honest: we cannot deny what we cannot measure.  Surfaced explicitly
    /// so operators observe the gap; not silently authorized.
    SizingUnverifiable {
        /// Human-readable reason including the cap value.
        reason: String,
    },

    /// The implied notional exceeds `max_position_notional_usd`.
    ///
    /// Always fail-closed: strategy is budget-authorized but its requested
    /// size is not realistically executable under the stated policy cap.
    SizingDenied {
        /// Human-readable reason including quantities and the cap.
        reason: String,
    },

    /// The sizing evaluator could not be run.
    ///
    /// Reserved for future panic-guard wrapper.  Always fail-closed.
    Unavailable {
        /// Human-readable reason.
        reason: String,
    },
}

impl PositionSizingOutcome {
    /// Truth-state label for the control-plane surface.
    pub fn truth_state(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::PolicyInvalid { .. } => "policy_invalid",
            Self::NoSizingConstraint => "no_sizing_constraint",
            Self::SizingAuthorized { .. } => "authorized",
            Self::SizingUnverifiable { .. } => "unverifiable",
            Self::SizingDenied { .. } => "sizing_denied",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// Whether this outcome is safe to allow signal ingestion.
    ///
    /// Returns `true` for:
    /// - `NotConfigured` — no sizing policy in effect
    /// - `NoSizingConstraint` — entry exists but no notional cap
    /// - `SizingAuthorized` — limit order is within cap
    /// - `SizingUnverifiable` — market order; honest pass-through
    ///
    /// Returns `false` for `SizingDenied`, `PolicyInvalid`, `Unavailable`.
    pub fn is_signal_safe(&self) -> bool {
        matches!(
            self,
            Self::NotConfigured
                | Self::NoSizingConstraint
                | Self::SizingAuthorized { .. }
                | Self::SizingUnverifiable { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Pure evaluator — position sizing realism (TV-04C)
// ---------------------------------------------------------------------------

/// Evaluate position sizing realism for a signal against the per-strategy
/// policy entry.
///
/// Pure: no env reads, no network, no DB.  Pass `None` to represent an
/// unconfigured path.
///
/// # Parameters
///
/// - `path` — path to `capital_allocation_policy.json`; `None` → `NotConfigured`
/// - `strategy_id` — the strategy emitting the signal
/// - `qty` — share quantity from the validated signal (must be > 0)
/// - `limit_price_micros` — limit price in 1/1_000_000 USD, present only for
///   limit orders; `None` means market order
///
/// # Validation contract
///
/// 1. `path` must be `Some` and non-empty — otherwise `NotConfigured`.
/// 2. File must be readable and valid JSON — otherwise `PolicyInvalid`.
/// 3. `schema_version` must equal `"policy-v1"` — otherwise `PolicyInvalid`.
/// 4. If `per_strategy_budgets` is absent or the strategy has no entry:
///    `NoSizingConstraint` (no cap to enforce).
/// 5. If the entry has no `max_position_notional_usd`: `NoSizingConstraint`.
/// 6. If `max_position_notional_usd` is not a positive number: `PolicyInvalid`.
/// 7. Market order (`limit_price_micros` is `None`): `SizingUnverifiable`.
/// 8. Limit order: compute `qty × (limit_price_micros / 1_000_000)`.
///    - Implied notional ≤ cap → `SizingAuthorized`.
///    - Implied notional > cap → `SizingDenied`.
pub fn evaluate_position_sizing(
    path: Option<&Path>,
    strategy_id: &str,
    qty: i64,
    limit_price_micros: Option<i64>,
) -> PositionSizingOutcome {
    let path = match path {
        None => return PositionSizingOutcome::NotConfigured,
        Some(p) if p.as_os_str().is_empty() => return PositionSizingOutcome::NotConfigured,
        Some(p) => p,
    };

    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return PositionSizingOutcome::PolicyInvalid {
                reason: format!("cannot read '{}': {e}", path.display()),
            }
        }
    };

    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return PositionSizingOutcome::PolicyInvalid {
                reason: format!("invalid JSON in '{}': {e}", path.display()),
            }
        }
    };

    match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == super::CAPITAL_POLICY_SCHEMA_VERSION => {}
        _ => {
            return PositionSizingOutcome::PolicyInvalid {
                reason: "missing or unsupported schema_version".to_string(),
            }
        }
    }

    // Locate the per-strategy entry.  Absent entry or absent budgets array
    // means no sizing constraint is defined for this strategy.
    let budgets = match j.get("per_strategy_budgets").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return PositionSizingOutcome::NoSizingConstraint,
    };

    let entry = budgets.iter().find(|e| {
        e.get("strategy_id")
            .and_then(|v| v.as_str())
            .map(|s| s == strategy_id)
            .unwrap_or(false)
    });

    let entry = match entry {
        Some(e) => e,
        None => return PositionSizingOutcome::NoSizingConstraint,
    };

    // Read max_position_notional_usd.  Absent → no cap.
    let max_notional = match entry.get("max_position_notional_usd") {
        None => return PositionSizingOutcome::NoSizingConstraint,
        Some(v) => match v.as_f64() {
            Some(n) if n > 0.0 => n,
            _ => {
                return PositionSizingOutcome::PolicyInvalid {
                    reason: format!(
                        "max_position_notional_usd for strategy '{}' must be a positive number",
                        strategy_id
                    ),
                }
            }
        },
    };

    // Market order: notional is not computable without a price reference.
    let Some(limit_price_micros) = limit_price_micros else {
        return PositionSizingOutcome::SizingUnverifiable {
            reason: format!(
                "market order for strategy '{}': implied notional cannot be computed \
                 without a price reference; max_position_notional_usd=${:.2} cannot be \
                 checked — operator should use limit orders when a notional cap is active",
                strategy_id, max_notional
            ),
        };
    };

    // Limit order: compute implied notional and compare to cap.
    // limit_price_micros is in 1/1_000_000 USD.
    let limit_price_usd = limit_price_micros as f64 / 1_000_000.0;
    let implied_notional = qty as f64 * limit_price_usd;

    if implied_notional > max_notional {
        return PositionSizingOutcome::SizingDenied {
            reason: format!(
                "strategy '{}' implied notional ${:.2} ({} shares × ${:.6}) exceeds \
                 max_position_notional_usd=${:.2} in capital allocation policy; \
                 reduce qty or use a lower limit_price",
                strategy_id, implied_notional, qty, limit_price_usd, max_notional
            ),
        };
    }

    PositionSizingOutcome::SizingAuthorized {
        strategy_id: strategy_id.to_string(),
        implied_notional_usd: implied_notional,
        max_position_notional_usd: max_notional,
    }
}

/// Read [`super::ENV_CAPITAL_POLICY_PATH`] from the environment and evaluate
/// position sizing realism for `strategy_id`.
///
/// Returns `NotConfigured` when the env var is absent or empty.
pub fn evaluate_position_sizing_from_env(
    strategy_id: &str,
    qty: i64,
    limit_price_micros: Option<i64>,
) -> PositionSizingOutcome {
    let raw = std::env::var(super::ENV_CAPITAL_POLICY_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(raw.trim()))
    };
    evaluate_position_sizing(path.as_deref(), strategy_id, qty, limit_price_micros)
}
