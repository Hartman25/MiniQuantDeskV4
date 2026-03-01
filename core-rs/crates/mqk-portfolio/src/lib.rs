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
mod metrics;
mod ordering;
mod types;

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
    ConstraintViolation, SectorConstraint, TurnoverConstraint, WeightBoundsConstraint,
};
pub use ledger::{Ledger, LedgerError, LedgerSnapshot};

pub use metrics::{
    compute_equity_micros, compute_exposure_micros, compute_unrealized_pnl_micros,
    enforce_max_gross_exposure, EquityMetrics, ExposureBreach, ExposureMetrics,
};

// R3-2: canonical fill ordering policy
pub use ordering::{apply_fills_canonical, sort_fills_canonical, TaggedFill};

pub use types::{CashEntry, Fill, LedgerEntry, Lot, PortfolioState, PositionState, Side};

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
