//! Restart-recoverable held-quantity state for `FixedInitialCapitalFractionV1`.
//!
//! Pure data and validation only: no IO. The runtime owns persistence; this
//! module defines what a durable record must contain and how a record is
//! proven against the deployment's sizing contract before a wrapper may be
//! built from it.
//!
//! A record is accepted only if every identity field equals the contract and
//! the stored quantity is exactly what the shared resolver
//! ([`crate::resolve_capital_fraction_target`]) yields for the stored
//! reference price, so a tampered, partial or foreign record fails closed.

use mqk_execution::QtyMicros;

use crate::sizing::{
    resolve_capital_fraction_target, TargetSizing, SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1,
};

pub const HELD_SIZING_STATE_VERSION: i32 = 1;

/// Deployment/strategy scope of one wrapper instance. `deployment_id` is the
/// caller-supplied canonical deployment identity; it is opaque here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldSizingScope {
    pub deployment_id: String,
    pub strategy_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldSizingStatus {
    Active,
    Released,
}

impl HeldSizingStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Released => "released",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "active" => Some(Self::Active),
            "released" => Some(Self::Released),
            _ => None,
        }
    }
}

/// One durable held-entry record, keyed by `(deployment_id, strategy_id,
/// symbol)`. `entry_generation` increments on every genuine flat->long entry
/// and orders retries: the same generation is the same entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldSizingRecord {
    pub deployment_id: String,
    pub strategy_id: String,
    pub symbol: String,
    pub state_version: i32,
    pub entry_generation: i64,
    pub status: HeldSizingStatus,
    pub policy_id: String,
    pub allocation_fraction_bps: i64,
    pub initial_allocated_capital_micros: i64,
    pub max_target_qty_micros: Option<i64>,
    pub max_notional_usd: Option<i64>,
    pub resolved_target_qty_micros: i64,
    pub reference_bar_end_ts: i64,
    pub reference_price_micros: i64,
}

/// A change the wrapper made to its held state, to be persisted by the
/// runtime before any decision derived from the same bar is submitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeldSizingTransition {
    Entered(HeldSizingRecord),
    /// The same record with `status == Released`.
    Released(HeldSizingRecord),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeldSizingRecoveryError {
    ForeignRecord { field: &'static str },
    UnsupportedStateVersion { found: i32 },
    ContractMismatch { field: &'static str },
    MalformedRecord { field: &'static str },
    QuantityNotReproducible { stored_micros: i64 },
    DuplicateSymbol { symbol: String },
    InvalidContract { reason: &'static str },
}

impl HeldSizingRecoveryError {
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::ForeignRecord { .. } => "held_sizing_foreign_record",
            Self::UnsupportedStateVersion { .. } => "held_sizing_unsupported_state_version",
            Self::ContractMismatch { .. } => "held_sizing_contract_mismatch",
            Self::MalformedRecord { .. } => "held_sizing_malformed_record",
            Self::QuantityNotReproducible { .. } => "held_sizing_quantity_not_reproducible",
            Self::DuplicateSymbol { .. } => "held_sizing_duplicate_symbol",
            Self::InvalidContract { .. } => "held_sizing_invalid_contract",
        }
    }
}

impl std::fmt::Display for HeldSizingRecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {self:?}", self.reason_code())
    }
}

impl std::error::Error for HeldSizingRecoveryError {}

/// The deployment contract a durable record is proven against.
#[derive(Clone, Debug)]
pub struct HeldSizingContract {
    pub scope: HeldSizingScope,
    pub allocation_fraction_bps: i64,
    pub initial_allocated_capital_micros: i64,
    pub caps: TargetSizing,
}

impl HeldSizingContract {
    pub fn validate_scope(&self) -> Result<(), HeldSizingRecoveryError> {
        if self.scope.deployment_id.trim().is_empty() {
            return Err(HeldSizingRecoveryError::InvalidContract {
                reason: "deployment_id is blank",
            });
        }
        if self.scope.strategy_id.trim().is_empty() {
            return Err(HeldSizingRecoveryError::InvalidContract {
                reason: "strategy_id is blank",
            });
        }
        Ok(())
    }

    /// Prove `record` against this contract. Never repairs or defaults.
    pub fn validate_record(&self, r: &HeldSizingRecord) -> Result<(), HeldSizingRecoveryError> {
        use HeldSizingRecoveryError as E;
        if r.deployment_id != self.scope.deployment_id {
            return Err(E::ForeignRecord {
                field: "deployment_id",
            });
        }
        if r.strategy_id != self.scope.strategy_id {
            return Err(E::ForeignRecord {
                field: "strategy_id",
            });
        }
        if r.symbol.trim().is_empty() || r.symbol.trim() != r.symbol {
            return Err(E::MalformedRecord { field: "symbol" });
        }
        if r.state_version != HELD_SIZING_STATE_VERSION {
            return Err(E::UnsupportedStateVersion {
                found: r.state_version,
            });
        }
        if r.entry_generation < 1 {
            return Err(E::MalformedRecord {
                field: "entry_generation",
            });
        }
        if r.policy_id != SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1 {
            return Err(E::ContractMismatch { field: "policy_id" });
        }
        if r.allocation_fraction_bps != self.allocation_fraction_bps {
            return Err(E::ContractMismatch {
                field: "allocation_fraction_bps",
            });
        }
        if r.initial_allocated_capital_micros != self.initial_allocated_capital_micros {
            return Err(E::ContractMismatch {
                field: "initial_allocated_capital_micros",
            });
        }
        if r.max_target_qty_micros != self.caps.max_target_qty().map(|q| q.raw()) {
            return Err(E::ContractMismatch {
                field: "max_target_qty_micros",
            });
        }
        if r.max_notional_usd != self.caps.max_notional_usd() {
            return Err(E::ContractMismatch {
                field: "max_notional_usd",
            });
        }
        if r.reference_bar_end_ts <= 0 {
            return Err(E::MalformedRecord {
                field: "reference_bar_end_ts",
            });
        }
        if r.reference_price_micros <= 0 {
            return Err(E::MalformedRecord {
                field: "reference_price_micros",
            });
        }
        let resolved = resolve_capital_fraction_target(
            self.allocation_fraction_bps,
            self.initial_allocated_capital_micros,
            r.reference_price_micros,
            &self.caps,
        )
        .map_err(|_| E::QuantityNotReproducible {
            stored_micros: r.resolved_target_qty_micros,
        })?;
        if resolved.resolved_target_qty != QtyMicros::new(r.resolved_target_qty_micros) {
            return Err(E::QuantityNotReproducible {
                stored_micros: r.resolved_target_qty_micros,
            });
        }
        Ok(())
    }
}
