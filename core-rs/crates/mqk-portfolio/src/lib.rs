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
mod canonical_decimal;
mod fixedpoint;
mod instrument_economics;
mod metrics;
mod ordering;
mod portfolio_economics;
mod types;
mod valuation;

pub mod allocator;
pub mod conflict_policy;
pub mod constraints;
pub mod cycle;
pub mod dynamic_selection;
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
// RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase D: pure allocation-cycle model,
// zero new callers (Bundle 5 daemon wiring is the only intended consumer).
pub use cycle::{
    compute_allocation_cycle, AllocationCandidateInput, AllocationCandidateResult,
    AllocationCycleContext, AllocationCycleResult, AllocationDisposition,
};
pub use ledger::{Ledger, LedgerError, LedgerSnapshot};

// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 2: pure dynamic strategy-symbol
// selection model, zero new callers (Bundle 7 daemon wiring is the only
// intended consumer).
// IR7: pure decimal-exact score canonicalization/comparison, zero I/O.
pub use canonical_decimal::{
    canonical_decimal_to_micros_if_exact, canonicalize_decimal_token,
    compare_canonical_decimal_strings,
};

pub use dynamic_selection::{
    canonical_plan_identity_material, canonical_symbol as dynamic_selection_canonical_symbol,
    compute_dynamic_selection_plan, verify_plan_selection_coherence,
    verify_symbol_selection_coherence, DynamicSelectionContext, DynamicSelectionMode,
    DynamicSelectionPlan, ExactSelectionReason, ReadinessBlockerReason,
    SelectionCandidateDisposition, SelectionCandidateEvidence, SelectionCandidateInput,
    SelectionCandidateResult, SelectionCoherenceViolation, SymbolSelectionResult,
    DYNAMIC_SELECTION_SCHEMA_VERSION, MAX_CANDIDATE_PAIRS, MAX_ELIGIBLE_SYMBOLS,
    MAX_STRATEGY_UNIVERSE, REASON_NOT_SELECTED_LOST_TIE_BREAK, REASON_NOT_SELECTED_LOWER_SCORE,
    REASON_NO_VALID_CANDIDATE as DYNAMIC_SELECTION_REASON_NO_VALID_CANDIDATE,
    REASON_REFUSED_BLANK_IDENTITY as DYNAMIC_SELECTION_REASON_REFUSED_BLANK_IDENTITY,
    REASON_REFUSED_DATA_NOT_READY, REASON_REFUSED_DIVERGENT_DUPLICATE,
    REASON_REFUSED_EVIDENCE_READ_FAILED, REASON_REFUSED_EXPIRED,
    REASON_REFUSED_FINGERPRINT_MISMATCH, REASON_REFUSED_FINGERPRINT_V2_MISMATCH,
    REASON_REFUSED_FINGERPRINT_V2_MISSING, REASON_REFUSED_MISSING_RANK_FOR_TIE,
    REASON_REFUSED_MISSING_SCORE, REASON_REFUSED_NOT_ACTIVE_PAPER,
    REASON_REFUSED_NOT_PAPER_CANDIDATE, REASON_REFUSED_NOT_YET_EFFECTIVE,
    REASON_REFUSED_PROMOTION_QUERY_FAILED, REASON_REFUSED_TIMEFRAME_MISMATCH,
    REASON_REFUSED_UNSUPPORTED_STRATEGY, REASON_SELECTED_HIGHEST_SCORE,
    REASON_SELECTED_TIE_BREAK_RANK, REASON_SELECTED_TIE_BREAK_STRATEGY_ID,
    REASON_SELECTED_TIE_BREAK_WATCHLIST, TRUTH_STATE_BLANK_ELIGIBLE_SYMBOL,
    TRUTH_STATE_CANDIDATES_OVER_LIMIT, TRUTH_STATE_CANDIDATE_OUTSIDE_ELIGIBLE_SET,
    TRUTH_STATE_COMPUTED as DYNAMIC_SELECTION_TRUTH_STATE_COMPUTED,
    TRUTH_STATE_DUPLICATE_ELIGIBLE_SYMBOL, TRUTH_STATE_ELIGIBLE_SYMBOLS_OVER_LIMIT,
    TRUTH_STATE_NO_ELIGIBLE_SYMBOLS, TRUTH_STATE_SYMBOL_CANDIDATES_OVER_LIMIT,
};

// MULTI-STRATEGY-CONFLICT-POLICY-01 Phase A: pure conflict-resolution model,
// zero new callers (Bundle 6 daemon wiring is the only intended consumer).
pub use conflict_policy::{
    canonical_symbol, resolve_conflict_cycle, ConflictCandidateInput, ConflictCandidateResult,
    ConflictCycleContext, ConflictCycleResult, ConflictDisposition, ConflictSymbolResult,
    CONFLICT_POLICY_SCHEMA_VERSION, REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED,
    REASON_ARITHMETIC_OVERFLOW, REASON_CONFLICTING_INCREASE_TARGETS_REFUSED,
    REASON_DUPLICATE_ECONOMIC_CANDIDATE, REASON_INCREASE_OVERRIDDEN_BY_RISK_REDUCTION,
    REASON_INVALID_CANDIDATE_REFUSED, REASON_MISSING_OR_MISMATCHED_BAR_FACTS, REASON_NOT_SELECTED,
    REASON_NO_VALID_CANDIDATE, REASON_RISK_REDUCING_CANDIDATE_SELECTED,
    REASON_SINGLE_CANDIDATE_PASSTHROUGH, REASON_TARGET_CONSENSUS_PASSTHROUGH,
    REASON_WOULD_CREATE_SHORT,
};

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
    compute_portfolio_weights, PortfolioWeightsSnapshot, PositionMark, PositionWeightInput,
    PositionWeightRow,
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
