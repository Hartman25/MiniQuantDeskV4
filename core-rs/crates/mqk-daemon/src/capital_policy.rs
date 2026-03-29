//! TV-04A: Portfolio-level capital allocation policy seam.
//! TV-04B: Per-strategy budget / risk-bucket enforcement.
//!
//! Establishes the minimum capital-allocation and strategy-budget control seam
//! so the runtime can truthfully distinguish between:
//!
//! - **artifact valid and deployable** — proven by TV-01/TV-02 gates
//! - **strategy enabled and not suppressed** — proven by CC-01/CC-02 gates
//! - **strategy capital-budget authorized** — proven by this module (TV-04B)
//!
//! # Operator configuration
//!
//! The operator creates a `capital_allocation_policy.json` file (schema
//! `policy-v1`) and sets [`ENV_CAPITAL_POLICY_PATH`] to its path.
//!
//! If the env var is absent or empty, no policy is enforced — the gate is
//! `NotConfigured` at the start boundary and `PolicyNotConfigured` at the
//! signal boundary.  This is honest: absence of a policy file does not
//! fabricate capital authorization; it means capital policy is not yet
//! established for this deployment.
//!
//! # Policy file schema (`policy-v1`)
//!
//! ```json
//! {
//!   "schema_version": "policy-v1",
//!   "policy_id": "paper-2026-q1",
//!   "enabled": true,
//!   "max_portfolio_notional_usd": 25000,
//!   "per_strategy_budgets": [
//!     {
//!       "strategy_id": "strat-momentum-001",
//!       "budget_authorized": true,
//!       "max_position_notional_usd": 10000,
//!       "risk_bucket": "equity_long_only"
//!     },
//!     {
//!       "strategy_id": "strat-mean-revert-002",
//!       "budget_authorized": false,
//!       "deny_reason": "under review; budget not released for this run"
//!     }
//!   ]
//! }
//! ```
//!
//! # Outcome separation
//!
//! - [`CapitalPolicyOutcome`] — result of the portfolio-level policy check at
//!   the **runtime start boundary** (TV-04A).
//! - [`StrategyBudgetOutcome`] — result of the per-strategy budget check at
//!   the **signal ingestion boundary** (TV-04B).
//!
//! Both are pure functions: no env reads, no network, no DB.  The `_from_env`
//! variants read [`ENV_CAPITAL_POLICY_PATH`] and delegate.

use std::path::Path;

/// Only accepted schema version string.  Must match operator-produced files.
const CAPITAL_POLICY_SCHEMA_VERSION: &str = "policy-v1";

/// Env var the operator sets to the path of the `capital_allocation_policy.json`
/// file.
///
/// Example:
/// `MQK_CAPITAL_POLICY_PATH=/home/user/policies/paper-2026-q1/capital_allocation_policy.json`
pub const ENV_CAPITAL_POLICY_PATH: &str = "MQK_CAPITAL_POLICY_PATH";

// ---------------------------------------------------------------------------
// TV-04A — Portfolio-level policy outcome
// ---------------------------------------------------------------------------

/// Result of evaluating the portfolio-level capital allocation policy at the
/// runtime **start boundary**.
///
/// Only [`CapitalPolicyOutcome::NotConfigured`] and
/// [`CapitalPolicyOutcome::Authorized`] are start-safe.  All other variants
/// are fail-closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapitalPolicyOutcome {
    /// No policy path was configured (env var absent or empty).
    ///
    /// Honest absence: operator has not established a capital policy for this
    /// deployment.  The gate is not applicable; callers pass through.
    NotConfigured,

    /// Policy path was configured but the file is unreadable, not valid JSON,
    /// carries an unsupported `schema_version`, or is missing required fields.
    ///
    /// Always fail-closed.
    PolicyInvalid {
        /// Human-readable reason for the validation failure.
        reason: String,
    },

    /// Policy file is valid but `enabled = false`.
    ///
    /// The operator has explicitly disabled this policy.  Fail-closed: the
    /// operator must set `enabled = true` and the policy must be re-evaluated.
    Denied {
        /// Human-readable reason (includes `policy_id`).
        reason: String,
    },

    /// Policy file is valid, `schema_version = "policy-v1"`, and
    /// `enabled = true`.
    ///
    /// This is the only start-safe non-NotConfigured outcome.
    Authorized {
        /// The `policy_id` from the file.
        policy_id: String,
    },

    /// The policy evaluator itself could not be run.
    ///
    /// Reserved for future panic-guard wrapper.  Always fail-closed.
    Unavailable {
        /// Human-readable reason.
        reason: String,
    },
}

impl CapitalPolicyOutcome {
    /// Truth-state label for the control-plane surface.
    pub fn truth_state(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::PolicyInvalid { .. } => "policy_invalid",
            Self::Denied { .. } => "denied",
            Self::Authorized { .. } => "authorized",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// Whether this outcome is safe to allow runtime start.
    ///
    /// Returns `true` for `NotConfigured` (gate not applicable) and
    /// `Authorized` (policy valid and enabled).  All other variants block
    /// start.
    pub fn is_start_safe(&self) -> bool {
        matches!(self, Self::NotConfigured | Self::Authorized { .. })
    }
}

// ---------------------------------------------------------------------------
// TV-04B — Per-strategy budget outcome
// ---------------------------------------------------------------------------

/// Result of evaluating per-strategy budget / risk-bucket authorization at
/// the signal **ingestion boundary**.
///
/// Only [`StrategyBudgetOutcome::PolicyNotConfigured`] and
/// [`StrategyBudgetOutcome::BudgetAuthorized`] are signal-safe.  All other
/// variants cause the signal to be refused fail-closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrategyBudgetOutcome {
    /// No policy path was configured (env var absent or empty).
    ///
    /// Budget enforcement is not active for this deployment; callers pass
    /// through.  This does NOT imply capital authorization — it means capital
    /// budget enforcement is not yet established.
    PolicyNotConfigured,

    /// Policy path was configured but the file is invalid.
    ///
    /// Always fail-closed: the policy cannot be evaluated.
    PolicyInvalid {
        /// Human-readable reason for the validation failure.
        reason: String,
    },

    /// Policy is valid but the strategy is not budget-authorized.
    ///
    /// Causes: strategy has no entry in `per_strategy_budgets`; strategy entry
    /// has `budget_authorized = false`; or the policy itself is `enabled = false`.
    BudgetDenied {
        /// Human-readable reason.
        reason: String,
    },

    /// Policy is valid, the strategy has an entry, and `budget_authorized = true`.
    ///
    /// This is the only start-safe non-PolicyNotConfigured outcome.
    BudgetAuthorized {
        /// The strategy that was authorized.
        strategy_id: String,
        /// The `risk_bucket` label from the policy entry, if present.
        risk_bucket: Option<String>,
    },

    /// The budget evaluator itself could not be run.
    ///
    /// Reserved for future panic-guard wrapper.  Always fail-closed.
    Unavailable {
        /// Human-readable reason.
        reason: String,
    },
}

impl StrategyBudgetOutcome {
    /// Truth-state label for the control-plane surface.
    pub fn truth_state(&self) -> &'static str {
        match self {
            Self::PolicyNotConfigured => "not_configured",
            Self::PolicyInvalid { .. } => "policy_invalid",
            Self::BudgetDenied { .. } => "budget_denied",
            Self::BudgetAuthorized { .. } => "authorized",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// Whether this outcome is safe to allow signal ingestion.
    ///
    /// Returns `true` for `PolicyNotConfigured` (no enforcement active) and
    /// `BudgetAuthorized` (explicit strategy budget authorization).
    pub fn is_signal_safe(&self) -> bool {
        matches!(
            self,
            Self::PolicyNotConfigured | Self::BudgetAuthorized { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Pure evaluator — portfolio-level policy (TV-04A)
// ---------------------------------------------------------------------------

/// Evaluate the portfolio-level capital allocation policy at `path`.
///
/// Pure: no env reads, no network, no DB.  Pass `None` to represent an
/// unconfigured path.
///
/// # Validation contract
/// 1. `path` must be `Some` and non-empty — otherwise `NotConfigured`.
/// 2. File must be readable — otherwise `PolicyInvalid`.
/// 3. Contents must be valid JSON — otherwise `PolicyInvalid`.
/// 4. `schema_version` must equal `"policy-v1"` — otherwise `PolicyInvalid`.
/// 5. `policy_id` must be present and non-empty — otherwise `PolicyInvalid`.
/// 6. `enabled` must be a boolean; `false` → `Denied`.
/// 7. `enabled = true` → `Authorized`.
pub fn evaluate_capital_policy(path: Option<&Path>) -> CapitalPolicyOutcome {
    let path = match path {
        None => return CapitalPolicyOutcome::NotConfigured,
        Some(p) if p.as_os_str().is_empty() => return CapitalPolicyOutcome::NotConfigured,
        Some(p) => p,
    };

    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return CapitalPolicyOutcome::PolicyInvalid {
                reason: format!("cannot read '{}': {e}", path.display()),
            }
        }
    };

    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return CapitalPolicyOutcome::PolicyInvalid {
                reason: format!("invalid JSON in '{}': {e}", path.display()),
            }
        }
    };

    match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == CAPITAL_POLICY_SCHEMA_VERSION => {}
        Some(other) => {
            return CapitalPolicyOutcome::PolicyInvalid {
                reason: format!(
                    "unsupported schema_version '{}'; expected '{}'",
                    other, CAPITAL_POLICY_SCHEMA_VERSION
                ),
            }
        }
        None => {
            return CapitalPolicyOutcome::PolicyInvalid {
                reason: "missing 'schema_version' field".to_string(),
            }
        }
    }

    let policy_id = match j
        .get("policy_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        Some(id) => id.to_string(),
        None => {
            return CapitalPolicyOutcome::PolicyInvalid {
                reason: "missing or empty 'policy_id' field".to_string(),
            }
        }
    };

    let enabled = match j.get("enabled").and_then(|v| v.as_bool()) {
        Some(b) => b,
        None => {
            return CapitalPolicyOutcome::PolicyInvalid {
                reason: "missing or non-boolean 'enabled' field".to_string(),
            }
        }
    };

    if !enabled {
        return CapitalPolicyOutcome::Denied {
            reason: format!(
                "capital allocation policy '{}' is present but enabled=false; \
                 set enabled=true in the policy file to authorize runtime start",
                policy_id
            ),
        };
    }

    CapitalPolicyOutcome::Authorized { policy_id }
}

// ---------------------------------------------------------------------------
// Pure evaluator — per-strategy budget (TV-04B)
// ---------------------------------------------------------------------------

/// Evaluate the per-strategy budget / risk-bucket authorization for
/// `strategy_id` against the policy at `path`.
///
/// Pure: no env reads, no network, no DB.  Pass `None` to represent an
/// unconfigured path.
///
/// # Validation contract
/// 1. `path` must be `Some` and non-empty — otherwise `PolicyNotConfigured`.
/// 2. File must be readable and valid JSON — otherwise `PolicyInvalid`.
/// 3. `schema_version` must equal `"policy-v1"` — otherwise `PolicyInvalid`.
/// 4. `enabled` must be a boolean; `false` → `BudgetDenied`.
/// 5. `per_strategy_budgets` must be present and an array — otherwise
///    `BudgetDenied` (absent array ≠ authorized).
/// 6. An entry for `strategy_id` must exist — otherwise `BudgetDenied`
///    (absent entry ≠ authorized).
/// 7. The entry's `budget_authorized` must be a boolean; `false` →
///    `BudgetDenied` (uses `deny_reason` if present).
/// 8. `budget_authorized = true` → `BudgetAuthorized`.
pub fn evaluate_strategy_budget(path: Option<&Path>, strategy_id: &str) -> StrategyBudgetOutcome {
    let path = match path {
        None => return StrategyBudgetOutcome::PolicyNotConfigured,
        Some(p) if p.as_os_str().is_empty() => return StrategyBudgetOutcome::PolicyNotConfigured,
        Some(p) => p,
    };

    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return StrategyBudgetOutcome::PolicyInvalid {
                reason: format!("cannot read '{}': {e}", path.display()),
            }
        }
    };

    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return StrategyBudgetOutcome::PolicyInvalid {
                reason: format!("invalid JSON in '{}': {e}", path.display()),
            }
        }
    };

    match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == CAPITAL_POLICY_SCHEMA_VERSION => {}
        Some(other) => {
            return StrategyBudgetOutcome::PolicyInvalid {
                reason: format!(
                    "unsupported schema_version '{}'; expected '{}'",
                    other, CAPITAL_POLICY_SCHEMA_VERSION
                ),
            }
        }
        None => {
            return StrategyBudgetOutcome::PolicyInvalid {
                reason: "missing 'schema_version' field".to_string(),
            }
        }
    }

    let enabled = match j.get("enabled").and_then(|v| v.as_bool()) {
        Some(b) => b,
        None => {
            return StrategyBudgetOutcome::PolicyInvalid {
                reason: "missing or non-boolean 'enabled' field".to_string(),
            }
        }
    };

    if !enabled {
        return StrategyBudgetOutcome::BudgetDenied {
            reason: format!(
                "capital allocation policy is present but enabled=false; \
                 strategy '{}' budget is denied until the policy is enabled",
                strategy_id
            ),
        };
    }

    let budgets = match j.get("per_strategy_budgets").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            return StrategyBudgetOutcome::BudgetDenied {
                reason: format!(
                    "strategy '{}' has no budget entry in the capital allocation policy \
                     (per_strategy_budgets absent or not an array); \
                     absent budget entry is not authorized — add an explicit entry",
                    strategy_id
                ),
            }
        }
    };

    let entry = budgets.iter().find(|e| {
        e.get("strategy_id")
            .and_then(|v| v.as_str())
            .map(|s| s == strategy_id)
            .unwrap_or(false)
    });

    let entry = match entry {
        Some(e) => e,
        None => {
            return StrategyBudgetOutcome::BudgetDenied {
                reason: format!(
                    "strategy '{}' has no budget entry in the capital allocation policy; \
                     absent entry is not authorized — add an explicit entry with \
                     budget_authorized=true to permit signals from this strategy",
                    strategy_id
                ),
            }
        }
    };

    let budget_authorized = match entry.get("budget_authorized").and_then(|v| v.as_bool()) {
        Some(b) => b,
        None => {
            return StrategyBudgetOutcome::PolicyInvalid {
                reason: format!(
                    "budget entry for strategy '{}' is missing or has non-boolean \
                     'budget_authorized' field",
                    strategy_id
                ),
            }
        }
    };

    if !budget_authorized {
        let deny_reason = entry
            .get("deny_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("budget_authorized=false in policy")
            .to_string();
        return StrategyBudgetOutcome::BudgetDenied {
            reason: format!(
                "strategy '{}' budget denied by capital allocation policy: {}",
                strategy_id, deny_reason
            ),
        };
    }

    let risk_bucket = entry
        .get("risk_bucket")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    StrategyBudgetOutcome::BudgetAuthorized {
        strategy_id: strategy_id.to_string(),
        risk_bucket,
    }
}

// ---------------------------------------------------------------------------
// Production entry points (read env var)
// ---------------------------------------------------------------------------

/// Read [`ENV_CAPITAL_POLICY_PATH`] from the environment and evaluate the
/// portfolio-level capital allocation policy.
///
/// Returns `NotConfigured` when the env var is absent or empty.
pub fn evaluate_capital_policy_from_env() -> CapitalPolicyOutcome {
    let raw = std::env::var(ENV_CAPITAL_POLICY_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(raw.trim()))
    };
    evaluate_capital_policy(path.as_deref())
}

/// Read [`ENV_CAPITAL_POLICY_PATH`] from the environment and evaluate the
/// per-strategy budget authorization for `strategy_id`.
///
/// Returns `PolicyNotConfigured` when the env var is absent or empty.
pub fn evaluate_strategy_budget_from_env(strategy_id: &str) -> StrategyBudgetOutcome {
    let raw = std::env::var(ENV_CAPITAL_POLICY_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(raw.trim()))
    };
    evaluate_strategy_budget(path.as_deref(), strategy_id)
}

// ---------------------------------------------------------------------------
// TV-04C — Position sizing realism outcome
// ---------------------------------------------------------------------------

/// Result of evaluating position sizing realism under broker/account limits
/// at the signal **ingestion boundary**.
///
/// Called after [`StrategyBudgetOutcome`] is signal-safe (Gate 1e passes).
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
        Some(sv) if sv == CAPITAL_POLICY_SCHEMA_VERSION => {}
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

/// Read [`ENV_CAPITAL_POLICY_PATH`] from the environment and evaluate
/// position sizing realism for `strategy_id`.
///
/// Returns `NotConfigured` when the env var is absent or empty.
pub fn evaluate_position_sizing_from_env(
    strategy_id: &str,
    qty: i64,
    limit_price_micros: Option<i64>,
) -> PositionSizingOutcome {
    let raw = std::env::var(ENV_CAPITAL_POLICY_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(raw.trim()))
    };
    evaluate_position_sizing(path.as_deref(), strategy_id, qty, limit_price_micros)
}
