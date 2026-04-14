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
pub(crate) const CAPITAL_POLICY_SCHEMA_VERSION: &str = "policy-v1";

/// Env var the operator sets to the path of the `capital_allocation_policy.json`
/// file.
///
/// Example:
/// `MQK_CAPITAL_POLICY_PATH=/home/user/policies/paper-2026-q1/capital_allocation_policy.json`
pub const ENV_CAPITAL_POLICY_PATH: &str = "MQK_CAPITAL_POLICY_PATH";

// ---------------------------------------------------------------------------
// Submodules
// ---------------------------------------------------------------------------

pub mod deployment_economics;
pub mod portfolio_risk;
pub mod position_sizing;

// Flat re-exports — preserve existing `crate::capital_policy::X` call sites.
pub use deployment_economics::{
    evaluate_deployment_economics, evaluate_deployment_economics_from_env,
    DeploymentEconomicsOutcome,
};
pub use portfolio_risk::{
    evaluate_portfolio_risk, evaluate_portfolio_risk_from_env, PortfolioRiskOutcome,
};
pub use position_sizing::{
    evaluate_position_sizing, evaluate_position_sizing_from_env, PositionSizingOutcome,
};

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
