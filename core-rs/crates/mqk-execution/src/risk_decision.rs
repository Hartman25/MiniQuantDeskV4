//! B2: Structured Risk Decisions
//!
//! Replaces bare `bool` returns from the risk gate with typed, reasoned
//! decisions.  All types are pure data — no IO, no side-effects, deterministic.
//!
//! # Fail-closed contract
//!
//! Any state where the risk engine cannot be consulted MUST produce
//! `Deny(RiskDenial { reason: RiskReason::RiskEngineUnavailable, .. })`.
//! Callers are not permitted to silently downgrade a `Deny` to `Allow`.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// RiskReason
// ---------------------------------------------------------------------------

/// The reason a risk gate denied an order request.
///
/// Variants are machine-readable codes used for operator diagnostics and
/// circuit-breaker routing.  `RiskEngineUnavailable` is the fail-closed
/// catch-all: if the risk engine cannot be consulted, execution is blocked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskReason {
    /// The resulting position would exceed the configured per-symbol position limit.
    PositionLimitExceeded,
    /// The requested order quantity exceeds the single-order size limit.
    MaxOrderSizeExceeded,
    /// The symbol is not in the allowed-symbols list.
    SymbolNotAllowed,
    /// Adding this order would exceed the available capital limit.
    CapitalLimitExceeded,
    /// The risk engine could not be reached or returned an indeterminate answer.
    ///
    /// Fail-closed: treat as `Deny`.
    RiskEngineUnavailable,
}

impl RiskReason {
    /// Machine-readable reason code (used in `SystemBlockState.reason_code`).
    pub fn as_code(&self) -> &'static str {
        match self {
            RiskReason::PositionLimitExceeded => "POSITION_LIMIT_EXCEEDED",
            RiskReason::MaxOrderSizeExceeded => "MAX_ORDER_SIZE_EXCEEDED",
            RiskReason::SymbolNotAllowed => "SYMBOL_NOT_ALLOWED",
            RiskReason::CapitalLimitExceeded => "CAPITAL_LIMIT_EXCEEDED",
            RiskReason::RiskEngineUnavailable => "RISK_ENGINE_UNAVAILABLE",
        }
    }

    /// Human-readable one-line summary.
    pub fn as_summary(&self) -> &'static str {
        match self {
            RiskReason::PositionLimitExceeded => {
                "Order denied — resulting position would exceed limit"
            }
            RiskReason::MaxOrderSizeExceeded => {
                "Order denied — requested quantity exceeds single-order size limit"
            }
            RiskReason::SymbolNotAllowed => "Order denied — symbol is not in the allowed list",
            RiskReason::CapitalLimitExceeded => {
                "Order denied — insufficient capital for this order"
            }
            RiskReason::RiskEngineUnavailable => {
                "Order denied — risk engine unavailable (fail-closed)"
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RiskEvidence
// ---------------------------------------------------------------------------

/// Supporting key-value evidence for a risk denial, used in operator diagnostics.
///
/// All fields are optional — only the fields relevant to the specific denial
/// reason need to be populated.  Callers should populate all fields they have
/// available so operators can diagnose the denial without re-running the check.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RiskEvidence {
    /// The symbol being checked.
    pub symbol: Option<String>,
    /// The order quantity being requested.
    pub requested_qty: Option<i64>,
    /// The current net position in the symbol before this order.
    pub current_position: Option<i64>,
    /// The configured limit that was breached.
    pub limit: Option<i64>,
}

impl RiskEvidence {
    /// Convert evidence to key-value pairs for inclusion in
    /// `SystemBlockState.evidence` (B4 observability).
    pub fn to_kv_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        if let Some(ref sym) = self.symbol {
            pairs.push(("symbol".to_string(), sym.clone()));
        }
        if let Some(qty) = self.requested_qty {
            pairs.push(("requested_qty".to_string(), qty.to_string()));
        }
        if let Some(pos) = self.current_position {
            pairs.push(("current_position".to_string(), pos.to_string()));
        }
        if let Some(lim) = self.limit {
            pairs.push(("limit".to_string(), lim.to_string()));
        }
        pairs
    }
}

// ---------------------------------------------------------------------------
// RiskDenial
// ---------------------------------------------------------------------------

/// A structured denial from the risk gate, combining a reason code with
/// supporting evidence for operator diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskDenial {
    /// The reason the order was denied.
    pub reason: RiskReason,
    /// Supporting evidence for operator diagnostics.
    pub evidence: RiskEvidence,
}

impl RiskDenial {
    /// Machine-readable reason code.
    pub fn reason_code(&self) -> &'static str {
        self.reason.as_code()
    }

    /// Human-readable one-line summary.
    pub fn reason_summary(&self) -> &'static str {
        self.reason.as_summary()
    }
}

// ---------------------------------------------------------------------------
// RiskDecision
// ---------------------------------------------------------------------------

/// The structured outcome of a risk gate evaluation.
///
/// `Allow` means all configured risk checks passed.
/// `Deny` carries the specific reason and supporting evidence.
///
/// # Fail-closed contract
///
/// Any indeterminate or unavailable state MUST produce
/// `Deny(RiskDenial { reason: RiskReason::RiskEngineUnavailable, .. })`.
/// Implementors MUST NOT silently downgrade a denial to `Allow`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskDecision {
    /// All risk checks passed — order may proceed to the next gate.
    Allow,
    /// A risk check failed — the reason and evidence are included.
    Deny(RiskDenial),
}

impl RiskDecision {
    /// Returns `true` if this decision allows the order.
    pub fn is_allowed(&self) -> bool {
        matches!(self, RiskDecision::Allow)
    }
}
