//! mqk-portfolio
//!
//! PATCH 06: Portfolio & Accounting Model
//! - Fill-driven ledger is the source of truth
//! - FIFO lot accounting
//! - Realized vs unrealized PnL
//! - Equity + exposure metrics
//! - Max gross exposure enforcement
//! - Pure deterministic logic (no IO, no time, no broker wiring)

mod accounting;
mod fixedpoint;
mod instrument_economics;
mod metrics;
mod ordering;
mod portfolio_economics;
mod types;
mod valuation;

pub mod allocator;
pub mod constraints;
pub mod ledger;

pub use accounting::{apply_entry, apply_fill, recompute_from_ledger};
pub use allocator::{
    AllocationConstraints, AllocationDecision, AllocationError, Allocator, Candidate,
    RejectedCandidate, RejectionReason,
};
pub use constraints::{
    check_all, check_sector_limits, check_turnover, check_weight_bounds, compute_turnover,
    evaluate_sector_risk, ConstraintViolation, SectorConstraint, SectorRiskEvaluation,
    TurnoverConstraint, WeightBoundsConstraint,
};
pub use ledger::{Ledger, LedgerError, LedgerSnapshot};

pub use metrics::{
    compute_equity_micros, compute_exposure_micros, compute_unrealized_pnl_micros,
    enforce_max_gross_exposure, EquityMetrics, ExposureBreach, ExposureMetrics,
};

// M4-1: fixed-point money type
pub use fixedpoint::Micros;

// R3-2: canonical fill ordering policy
pub use ordering::{apply_fills_canonical, sort_fills_canonical, TaggedFill};

pub use types::{CashEntry, Fill, LedgerEntry, Lot, PortfolioState, PositionState, Side};

// PORTFOLIO-LIVE-WEIGHTS-01: live position valuation / weight truth seam
pub use valuation::{
    compute_portfolio_weights, PositionMark, PositionWeightInput, PositionWeightRow,
    PortfolioWeightsSnapshot,
};

// PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01B: per-position unrealized P&L
// from a single blended avg cost basis + mark (broker-snapshot route layer).
pub use valuation::unrealized_pnl_micros;

// ASSET-CORE-04A: pure, default-unused instrument economics model
// (multiplier/currency/quantity-scale-aware single-position valuation).
pub use instrument_economics::{
    value_position_economics, InstrumentEconomics, InstrumentEconomicsTruthState,
    PositionEconomicsInput, PositionEconomicsValue,
};

// ASSET-CORE-04C: pure, default-unused multi-asset portfolio NAV/exposure
// aggregation model (composes PositionEconomicsValue rows into a snapshot).
pub use portfolio_economics::{
    aggregate_portfolio_economics, PortfolioEconomicsExposureRow, PortfolioEconomicsInput,
    PortfolioEconomicsPositionRow, PortfolioEconomicsSnapshot, PortfolioEconomicsTruthState,
};

use std::collections::BTreeMap;

/// Price/cash scale: micros (1e-6).
pub const MICROS_SCALE: i64 = 1_000_000;

/// Canonical mark map type (symbol -> price_micros).
pub type MarkMap = BTreeMap<String, i64>;

/// Helper to build a MarkMap with minimal boilerplate.
pub fn marks<I, S>(items: I) -> MarkMap
where
    I: IntoIterator<Item = (S, i64)>,
    S: Into<String>,
{
    let mut m = MarkMap::new();
    for (sym, px) in items {
        m.insert(sym.into(), px);
    }
    m
}
