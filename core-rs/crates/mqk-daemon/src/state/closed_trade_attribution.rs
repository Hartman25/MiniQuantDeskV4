//! WAVE05-STRATEGY-CLOSED-TRADE-READ-MODEL-01: deterministic, read-only
//! FIFO closed-trade attribution over the canonical effective-fill replay.
//!
//! This is NOT a second accounting system. It consumes the exact ordered
//! `CanonicalAppliedFill` sequence produced by
//! [`super::snapshot::recover_oms_and_portfolio_traced`] -- the same
//! canonical replay that feeds `mqk_portfolio`'s own FIFO accounting -- and
//! mirrors `mqk_portfolio::accounting`'s `buy_fifo`/`sell_fifo` arithmetic
//! bit-for-bit to attach durable strategy lineage (via
//! [`mqk_db::fetch_fill_strategy_lineage`], P1's exact
//! `internal_order_id -> oms_outbox.idempotency_key` resolver) to each FIFO
//! lot closure. Nothing here mutates `mqk_portfolio::Lot` or writes any
//! durable table -- callers must independently verify
//! `sum_gross_realized_pnl_micros == canonical_realized_pnl_micros` (proof
//! that this mirror stayed faithful) before trusting the fragments.
//!
//! `gross_realized_pnl_micros` is GROSS trading P&L, matching
//! `mqk_portfolio::PortfolioState::realized_pnl_micros` exactly -- fees are
//! never netted in here.

use std::collections::{BTreeMap, HashMap};

use mqk_portfolio::Side;
use sqlx::PgPool;
use uuid::Uuid;

use super::snapshot::{recover_oms_and_portfolio_traced, CanonicalAppliedFill};
use super::types::RuntimeLifecycleError;
use crate::routes::portfolio_provenance::{
    classify_portfolio_provenance, validate_run_scoped_snapshot_authority, PortfolioProvenanceState,
};

/// Durable strategy identity resolved for one canonical fill, collapsed from
/// [`mqk_db::FillStrategyLineage`] into the coarser categories a closure
/// attribution decision needs. Never invents a fingerprint for a legacy
/// order and never collapses a lookup failure/corruption into "manual".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedLineage {
    /// Fully-bound strategy identity: both `strategy_id` and
    /// `strategy_semantic_fingerprint` durably resolved.
    Strategy {
        strategy_id: String,
        strategy_semantic_fingerprint: String,
    },
    /// A durable `strategy_id` exists but the order predates fingerprint
    /// capture -- exact semantic identity cannot be proven.
    LegacyStrategy { strategy_id: String },
    /// Originating outbox row is run-coherent and carries no strategy
    /// signal provenance at all -- a genuine manual/non-strategy order.
    Manual,
    /// The originating outbox row's lineage fields are malformed/
    /// contradictory, or it belongs to a different run
    /// ([`mqk_db::FillStrategyLineage::Invalid`]).
    Invalid,
    /// `internal_order_id` does not resolve to any `oms_outbox` row
    /// ([`mqk_db::FillStrategyLineage::OriginatingOrderMissing`]).
    Missing,
}

impl ResolvedLineage {
    fn from_fill_strategy_lineage(lineage: mqk_db::FillStrategyLineage) -> Self {
        match lineage {
            mqk_db::FillStrategyLineage::Resolved {
                strategy_id: Some(strategy_id),
                strategy_semantic_fingerprint: Some(strategy_semantic_fingerprint),
            } => Self::Strategy {
                strategy_id,
                strategy_semantic_fingerprint,
            },
            mqk_db::FillStrategyLineage::Resolved {
                strategy_id: Some(strategy_id),
                strategy_semantic_fingerprint: None,
            } => Self::LegacyStrategy { strategy_id },
            mqk_db::FillStrategyLineage::Resolved {
                strategy_id: None, ..
            } => Self::Manual,
            mqk_db::FillStrategyLineage::OriginatingOrderMissing => Self::Missing,
            mqk_db::FillStrategyLineage::Invalid { .. } => Self::Invalid,
        }
    }

    /// `(strategy_id, strategy_semantic_fingerprint)` as durable evidence to
    /// surface on a closure row -- never fabricated, `None` where the
    /// underlying field is genuinely absent or untrustworthy.
    pub(crate) fn identity_pair(&self) -> (Option<String>, Option<String>) {
        match self {
            Self::Strategy {
                strategy_id,
                strategy_semantic_fingerprint,
            } => (
                Some(strategy_id.clone()),
                Some(strategy_semantic_fingerprint.clone()),
            ),
            Self::LegacyStrategy { strategy_id } => (Some(strategy_id.clone()), None),
            Self::Manual | Self::Invalid | Self::Missing => (None, None),
        }
    }
}

/// Exact semantic-strategy attribution state of one FIFO closure fragment.
/// See WAVE05-STRATEGY-CLOSED-TRADE-READ-MODEL-01 Invariant C.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClosureAttribution {
    /// Opening and closing fills share the same `strategy_id` AND the same
    /// `strategy_semantic_fingerprint`.
    Attributed,
    /// Opening and closing fills resolve to different `strategy_id`s.
    CrossStrategy,
    /// Same `strategy_id` on both sides, but a different
    /// `strategy_semantic_fingerprint` -- not the same semantic strategy.
    SemanticIdentityChanged,
    /// At least one side is a genuine manual/non-strategy order.
    ManualOrMixed,
    /// At least one side is a legacy strategy fill missing its fingerprint
    /// -- gross math is visible but exact semantic attribution cannot be
    /// proven.
    LineageIncomplete,
    /// At least one side's lineage is malformed/contradictory
    /// (`FillStrategyLineage::Invalid`).
    LineageInvalid,
    /// At least one side's originating outbox row is missing entirely.
    LineageMissing,
}

impl ClosureAttribution {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Attributed => "attributed",
            Self::CrossStrategy => "cross_strategy",
            Self::SemanticIdentityChanged => "semantic_identity_changed",
            Self::ManualOrMixed => "manual_or_mixed",
            Self::LineageIncomplete => "lineage_incomplete",
            Self::LineageInvalid => "lineage_invalid",
            Self::LineageMissing => "lineage_missing",
        }
    }
}

/// Combine an opening and a closing fill's resolved lineage into one
/// closure attribution state. Order of checks is priority, most-certain
/// non-attributable evidence first: a missing/invalid originating order on
/// either side must never be upgraded into "manual" or "attributed" just
/// because the other side looks clean.
fn combine_attribution(open: &ResolvedLineage, close: &ResolvedLineage) -> ClosureAttribution {
    use ResolvedLineage::*;
    match (open, close) {
        (Missing, _) | (_, Missing) => ClosureAttribution::LineageMissing,
        (Invalid, _) | (_, Invalid) => ClosureAttribution::LineageInvalid,
        (Manual, _) | (_, Manual) => ClosureAttribution::ManualOrMixed,
        (LegacyStrategy { .. }, _) | (_, LegacyStrategy { .. }) => {
            ClosureAttribution::LineageIncomplete
        }
        (
            Strategy {
                strategy_id: open_id,
                strategy_semantic_fingerprint: open_fp,
            },
            Strategy {
                strategy_id: close_id,
                strategy_semantic_fingerprint: close_fp,
            },
        ) => {
            if open_id != close_id {
                ClosureAttribution::CrossStrategy
            } else if open_fp != close_fp {
                ClosureAttribution::SemanticIdentityChanged
            } else {
                ClosureAttribution::Attributed
            }
        }
    }
}

/// One deterministic FIFO lot closure: an opposite-side canonical effective
/// fill fully or partially consumed an open lot. `direction` names the
/// direction of the LOT THAT WAS CLOSED (`"long"` closed by a sell,
/// `"short"` closed by a buy) -- not the closing fill's own side.
#[derive(Debug, Clone)]
pub(crate) struct ClosureFragment {
    pub(crate) symbol: String,
    pub(crate) direction: &'static str,
    pub(crate) qty: i64,
    pub(crate) entry_price_micros: i64,
    pub(crate) exit_price_micros: i64,
    pub(crate) gross_realized_pnl_micros: i64,
    pub(crate) open_inbox_id: i64,
    pub(crate) open_internal_order_id: String,
    pub(crate) close_inbox_id: i64,
    pub(crate) close_internal_order_id: String,
    pub(crate) open_lineage: ResolvedLineage,
    pub(crate) close_lineage: ResolvedLineage,
    pub(crate) attribution: ClosureAttribution,
}

/// A read-model FIFO lot carrying opening provenance. Deliberately separate
/// from `mqk_portfolio::Lot` -- that type must never carry strategy data.
struct AttributedLot {
    qty_signed: i64,
    entry_price_micros: i64,
    open_inbox_id: i64,
    open_internal_order_id: String,
    open_lineage: ResolvedLineage,
}

impl AttributedLot {
    fn abs_qty(&self) -> i64 {
        self.qty_signed.abs()
    }
    fn is_long(&self) -> bool {
        self.qty_signed > 0
    }
    fn is_short(&self) -> bool {
        self.qty_signed < 0
    }
}

// This helper intentionally duplicates
// `mqk_portfolio::accounting::i128_to_i64_clamp` (private to that crate).
// CT11 (account parity) is the load-bearing proof that this mirror stays
// bit-for-bit identical; any divergence fails that test closed rather than
// silently drifting.
fn i128_to_i64_clamp(x: i128) -> i64 {
    if x > i64::MAX as i128 {
        i64::MAX
    } else if x < i64::MIN as i128 {
        i64::MIN
    } else {
        x as i64
    }
}

/// Mirrors `mqk_portfolio::accounting::buy_fifo`: covers short lots FIFO,
/// emitting one [`ClosureFragment`] per consumed lot, then opens a new long
/// lot with any remainder.
fn attribute_buy(
    lots: &mut Vec<AttributedLot>,
    fragments: &mut Vec<ClosureFragment>,
    sum_gross_realized_pnl_micros: &mut i64,
    symbol: &str,
    caf: &CanonicalAppliedFill,
    fill_lineage: &ResolvedLineage,
) {
    let mut qty = caf.fill.qty;
    let buy_px = caf.fill.price_micros;

    let mut i = 0usize;
    while qty > 0 && i < lots.len() {
        if !lots[i].is_short() {
            i += 1;
            continue;
        }

        let coverable = lots[i].abs_qty().min(qty);
        let entry_px = lots[i].entry_price_micros;
        let pnl = i128_to_i64_clamp((entry_px as i128 - buy_px as i128) * (coverable as i128));
        *sum_gross_realized_pnl_micros = sum_gross_realized_pnl_micros.saturating_add(pnl);

        let attribution = combine_attribution(&lots[i].open_lineage, fill_lineage);
        fragments.push(ClosureFragment {
            symbol: symbol.to_string(),
            direction: "short",
            qty: coverable,
            entry_price_micros: entry_px,
            exit_price_micros: buy_px,
            gross_realized_pnl_micros: pnl,
            open_inbox_id: lots[i].open_inbox_id,
            open_internal_order_id: lots[i].open_internal_order_id.clone(),
            close_inbox_id: caf.inbox_id,
            close_internal_order_id: caf.internal_order_id.clone(),
            open_lineage: lots[i].open_lineage.clone(),
            close_lineage: fill_lineage.clone(),
            attribution,
        });

        let remaining_abs = lots[i].abs_qty() - coverable;
        if remaining_abs == 0 {
            lots.remove(i);
        } else {
            lots[i].qty_signed = -(remaining_abs);
            i += 1;
        }
        qty -= coverable;
    }

    if qty > 0 {
        lots.push(AttributedLot {
            qty_signed: qty,
            entry_price_micros: buy_px,
            open_inbox_id: caf.inbox_id,
            open_internal_order_id: caf.internal_order_id.clone(),
            open_lineage: fill_lineage.clone(),
        });
    }
}

/// Mirrors `mqk_portfolio::accounting::sell_fifo`: reduces long lots FIFO,
/// emitting one [`ClosureFragment`] per consumed lot, then opens a new short
/// lot with any remainder.
fn attribute_sell(
    lots: &mut Vec<AttributedLot>,
    fragments: &mut Vec<ClosureFragment>,
    sum_gross_realized_pnl_micros: &mut i64,
    symbol: &str,
    caf: &CanonicalAppliedFill,
    fill_lineage: &ResolvedLineage,
) {
    let mut qty = caf.fill.qty;
    let sell_px = caf.fill.price_micros;

    let mut i = 0usize;
    while qty > 0 && i < lots.len() {
        if !lots[i].is_long() {
            i += 1;
            continue;
        }

        let sellable = lots[i].abs_qty().min(qty);
        let entry_px = lots[i].entry_price_micros;
        let pnl = i128_to_i64_clamp((sell_px as i128 - entry_px as i128) * (sellable as i128));
        *sum_gross_realized_pnl_micros = sum_gross_realized_pnl_micros.saturating_add(pnl);

        let attribution = combine_attribution(&lots[i].open_lineage, fill_lineage);
        fragments.push(ClosureFragment {
            symbol: symbol.to_string(),
            direction: "long",
            qty: sellable,
            entry_price_micros: entry_px,
            exit_price_micros: sell_px,
            gross_realized_pnl_micros: pnl,
            open_inbox_id: lots[i].open_inbox_id,
            open_internal_order_id: lots[i].open_internal_order_id.clone(),
            close_inbox_id: caf.inbox_id,
            close_internal_order_id: caf.internal_order_id.clone(),
            open_lineage: lots[i].open_lineage.clone(),
            close_lineage: fill_lineage.clone(),
            attribution,
        });

        let remaining_abs = lots[i].abs_qty() - sellable;
        if remaining_abs == 0 {
            lots.remove(i);
        } else {
            lots[i].qty_signed = remaining_abs;
            i += 1;
        }
        qty -= sellable;
    }

    if qty > 0 {
        lots.push(AttributedLot {
            qty_signed: -qty,
            entry_price_micros: sell_px,
            open_inbox_id: caf.inbox_id,
            open_internal_order_id: caf.internal_order_id.clone(),
            open_lineage: fill_lineage.clone(),
        });
    }
}

/// Deterministic read-only closed-trade attribution projection for one run.
pub(crate) struct ClosedTradeProjection {
    pub(crate) fragments: Vec<ClosureFragment>,
    pub(crate) sum_gross_realized_pnl_micros: i64,
    /// `realized_pnl_micros` from the SAME canonical replay this
    /// projection's fragments were built from -- the load-bearing parity
    /// proof a caller must check before trusting `fragments`.
    pub(crate) canonical_realized_pnl_micros: i64,
    /// The exact `recover_oms_and_portfolio_traced` replay watermark
    /// (`max(inbox_id)` across all applied rows) this projection was built
    /// from. A caller must compare this against the durable
    /// `sys_paper_portfolio_accounting_state.last_applied_inbox_id` before
    /// trusting durable accounting truth as same-watermark-current with this
    /// projection (WAVE05-STRATEGY-CLOSED-TRADE-READ-MODEL-01-REPAIR-01).
    pub(crate) canonical_last_applied_inbox_id: i64,
}

/// Build the attributed FIFO closed-trade projection for `run_id`.
///
/// Pure/read-only: performs no writes. Reuses
/// [`recover_oms_and_portfolio_traced`] for the canonical effective-fill
/// replay (Invariant A) and [`mqk_db::fetch_fill_strategy_lineage`] (P1) for
/// strategy identity -- never re-derives either from scratch. Accepts an
/// explicit `run_id` so later analytics can reuse this for historical runs.
pub(crate) async fn build_closed_trade_projection(
    db: &PgPool,
    run_id: Uuid,
) -> Result<ClosedTradeProjection, RuntimeLifecycleError> {
    let (_, _, portfolio, canonical_fills, canonical_last_applied_inbox_id) =
        recover_oms_and_portfolio_traced(db, run_id, 0).await?;

    let mut lineage_cache: HashMap<String, ResolvedLineage> = HashMap::new();
    let mut lots_by_symbol: BTreeMap<String, Vec<AttributedLot>> = BTreeMap::new();
    let mut fragments: Vec<ClosureFragment> = Vec::new();
    let mut sum_gross_realized_pnl_micros: i64 = 0;

    for caf in &canonical_fills {
        let lineage = match lineage_cache.get(&caf.internal_order_id) {
            Some(cached) => cached.clone(),
            None => {
                let raw =
                    mqk_db::fetch_fill_strategy_lineage(db, run_id, &caf.internal_order_id)
                        .await
                        .map_err(|e| {
                            RuntimeLifecycleError::internal(
                                "build_closed_trade_projection.lineage_lookup_failed",
                                format!(
                                    "run_id={run_id} internal_order_id={}: {e}",
                                    caf.internal_order_id
                                ),
                            )
                        })?;
                let resolved = ResolvedLineage::from_fill_strategy_lineage(raw);
                lineage_cache.insert(caf.internal_order_id.clone(), resolved.clone());
                resolved
            }
        };

        let lots = lots_by_symbol.entry(caf.fill.symbol.clone()).or_default();
        match caf.fill.side {
            Side::Buy => attribute_buy(
                lots,
                &mut fragments,
                &mut sum_gross_realized_pnl_micros,
                &caf.fill.symbol,
                caf,
                &lineage,
            ),
            Side::Sell => attribute_sell(
                lots,
                &mut fragments,
                &mut sum_gross_realized_pnl_micros,
                &caf.fill.symbol,
                caf,
                &lineage,
            ),
        }
    }

    Ok(ClosedTradeProjection {
        fragments,
        sum_gross_realized_pnl_micros,
        canonical_realized_pnl_micros: portfolio.realized_pnl_micros,
        canonical_last_applied_inbox_id,
    })
}

// ---------------------------------------------------------------------------
// WAVE05-STRATEGY-PERFORMANCE-ANALYTICS-01 (P3.1): shared closed-trade
// authority resolution.
//
// This is the SAME authority computation `routes/paper_journal.rs`'s
// `closed_trades_lane` and `routes/strategy_performance.rs` both consume --
// a narrow, behavior-preserving extraction of what was previously inlined
// only in `paper_journal`. Neither caller may hand-roll a second provenance
// classifier, snapshot-authority validator, canonical replay, or
// durable-accounting comparison: both call `resolve_authoritative_closed_trade_view`
// and map the SAME result into their own response shape.
// ---------------------------------------------------------------------------

/// Pure decision for the closed-trades authority's exposed `truth_state` plus
/// the bounded `accounting_watermark_state` label, given the shared portfolio
/// provenance verdict and the same-watermark / durable-realized-P&L parity
/// checks. No I/O -- fully unit-testable (WAVE05-STRATEGY-CLOSED-TRADE-
/// READ-MODEL-01-REPAIR-01).
///
/// `truth_state` is `"active"` only when ALL of the following hold:
/// - `provenance == Active` (shared classifier: run-scoped snapshot exists,
///   passes independent authority validation, and the durable accounting
///   row's `source_snapshot_id` matches it with `accounting_epoch ==
///   "complete"`);
/// - the durable accounting watermark exactly equals the canonical replay
///   watermark (Defect 2 repair -- a stale accounting row must never be
///   treated as same-watermark current merely because realized P&L happens
///   to still match);
/// - the durable `realized_pnl_micros` exactly equals the canonical
///   projection's summed gross realized P&L.
///
/// Any other `provenance` value fails closed to `"query_failed"` (when the
/// classifier itself reports a query failure) or `"incomplete"` (every other
/// non-active state -- `fill_history_incomplete`, `not_found`,
/// `accounting_epoch_unavailable`, `accounting_snapshot_mismatch`,
/// `unsupported_source`, `invalid_snapshot`) -- the exact classification is
/// still exposed separately via `accounting_provenance_state`, never hidden
/// behind this coarser label.
pub(crate) struct ClosedTradesAuthority {
    pub(crate) truth_state: &'static str,
    /// `Some("same_watermark" | "accounting_watermark_mismatch")` only when
    /// `provenance == Active` -- a same-watermark comparison is only
    /// meaningful once the shared classifier has already proven the
    /// snapshot/accounting relationship current. `None` otherwise.
    pub(crate) accounting_watermark_state: Option<&'static str>,
    /// `true` when the durable `realized_pnl_micros` contradicted the
    /// canonical projection's sum despite matching watermarks -- distinct
    /// from an ordinary watermark mismatch, which is never called
    /// "parity_failed" (a fresher, unpersisted accounting refresh is
    /// expected/benign, not a contradiction).
    pub(crate) durable_pnl_mismatch: bool,
}

pub(crate) fn classify_closed_trades_authority(
    provenance: PortfolioProvenanceState,
    canonical_last_applied_inbox_id: i64,
    accounting_last_applied_inbox_id: Option<i64>,
    canonical_sum_realized_pnl_micros: i64,
    accounting_realized_pnl_micros: Option<i64>,
) -> ClosedTradesAuthority {
    if provenance != PortfolioProvenanceState::Active {
        return ClosedTradesAuthority {
            truth_state: if provenance == PortfolioProvenanceState::QueryFailed {
                "query_failed"
            } else {
                "incomplete"
            },
            accounting_watermark_state: None,
            durable_pnl_mismatch: false,
        };
    }

    // `provenance == Active` guarantees (by `classify_portfolio_provenance`'s
    // own contract) that a durable accounting row exists, so both `Option`s
    // below are `Some` in practice -- compared via `Option` equality anyway
    // so a hypothetical `None` fails closed (mismatch) rather than panicking.
    if accounting_last_applied_inbox_id != Some(canonical_last_applied_inbox_id) {
        return ClosedTradesAuthority {
            truth_state: "incomplete",
            accounting_watermark_state: Some("accounting_watermark_mismatch"),
            durable_pnl_mismatch: false,
        };
    }

    if accounting_realized_pnl_micros != Some(canonical_sum_realized_pnl_micros) {
        return ClosedTradesAuthority {
            truth_state: "parity_failed",
            accounting_watermark_state: Some("same_watermark"),
            durable_pnl_mismatch: true,
        };
    }

    ClosedTradesAuthority {
        truth_state: "active",
        accounting_watermark_state: Some("same_watermark"),
        durable_pnl_mismatch: false,
    }
}

/// The single authoritative closed-trade result for one run -- everything a
/// caller needs to either surface the Paper Journal `closed_trades_lane` or
/// build strategy-performance analytics on top of `fragments`, without
/// re-deriving any of the provenance/parity classification itself.
#[allow(clippy::type_complexity)]
pub(crate) struct AuthoritativeClosedTradeView {
    pub(crate) truth_state: &'static str,
    pub(crate) accounting_epoch: Option<String>,
    pub(crate) accounting_epoch_reason: Option<String>,
    /// `Some` only when `truth_state` is `"active"` or `"incomplete"`.
    pub(crate) sum_gross_realized_pnl_micros: Option<i64>,
    /// The exact shared `classify_portfolio_provenance` verdict string.
    /// `None` only when the projection itself never got far enough to
    /// classify it (top-level projection-vs-canonical-replay parity
    /// failure, run lookup failure, or projection build failure).
    pub(crate) accounting_provenance_state: Option<&'static str>,
    pub(crate) canonical_last_applied_inbox_id: Option<i64>,
    pub(crate) accounting_last_applied_inbox_id: Option<i64>,
    pub(crate) accounting_watermark_state: Option<&'static str>,
    /// FIFO closure fragments. Populated only when `truth_state` is
    /// `"active"` or `"incomplete"` -- empty otherwise (never fabricated).
    pub(crate) fragments: Vec<ClosureFragment>,
}


/// Resolve the single authoritative closed-trade view for `run_id`. Pure
/// read-only: performs no writes. This is the ONLY place that combines
/// [`build_closed_trade_projection`], the shared portfolio-provenance
/// classifier, and [`classify_closed_trades_authority`] -- every caller
/// (Paper Journal, strategy performance analytics) must resolve through this
/// function rather than re-implementing any part of it.
pub(crate) async fn resolve_authoritative_closed_trade_view(
    db: &PgPool,
    run_id: Uuid,
) -> AuthoritativeClosedTradeView {
    let unavailable = |truth_state: &'static str| AuthoritativeClosedTradeView {
        truth_state,
        accounting_epoch: None,
        accounting_epoch_reason: None,
        sum_gross_realized_pnl_micros: None,
        accounting_provenance_state: None,
        canonical_last_applied_inbox_id: None,
        accounting_last_applied_inbox_id: None,
        accounting_watermark_state: None,
        fragments: vec![],
    };

    let run_record = mqk_db::fetch_run(db, run_id).await;
    if let Err(e) = &run_record {
        tracing::warn!("closed_trades run lookup failed (non-fatal): {e}");
        return unavailable("query_failed");
    }
    let is_paper_mode = run_record
        .as_ref()
        .map(|r| r.mode == "PAPER")
        .unwrap_or(false);

    let proj = match build_closed_trade_projection(db, run_id).await {
        Ok(proj) => proj,
        Err(e) => {
            tracing::warn!("closed_trades projection failed (non-fatal): {e}");
            return unavailable("query_failed");
        }
    };

    if proj.sum_gross_realized_pnl_micros != proj.canonical_realized_pnl_micros {
        tracing::error!(
            "closed_trades parity failure: projection_sum={} canonical_replay_realized_pnl={} run_id={run_id}",
            proj.sum_gross_realized_pnl_micros,
            proj.canonical_realized_pnl_micros,
        );
        return unavailable("parity_failed");
    }

    let snapshot_result = mqk_db::fetch_latest_paper_portfolio_snapshot_for_run(
        db,
        "paper",
        mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        run_id,
    )
    .await;
    if let Err(err) = &snapshot_result {
        tracing::warn!(error = %err, run_id = %run_id, "closed_trades_snapshot_query_failed");
    }
    let accounting_result = mqk_db::fetch_paper_portfolio_accounting_state(db, run_id).await;
    if let Err(err) = &accounting_result {
        tracing::warn!(error = %err, run_id = %run_id, "closed_trades_accounting_query_failed");
    }

    let snapshot_query_failed = snapshot_result.is_err();
    let accounting_query_failed = accounting_result.is_err();
    let snapshot_invalid = snapshot_result
        .as_ref()
        .ok()
        .and_then(|s| s.as_ref())
        .is_some_and(|s| validate_run_scoped_snapshot_authority(s, run_id).is_err());
    let snapshot_id = snapshot_result
        .as_ref()
        .ok()
        .and_then(|s| s.as_ref())
        .map(|s| s.snapshot.snapshot_id);
    let accounting = accounting_result.as_ref().ok().and_then(|a| a.clone());

    let provenance = classify_portfolio_provenance(
        is_paper_mode,
        snapshot_query_failed,
        accounting_query_failed,
        snapshot_id,
        snapshot_invalid,
        accounting.as_ref(),
    );

    let epoch = accounting.as_ref().map(|a| a.accounting_epoch.clone());
    let epoch_reason = accounting
        .as_ref()
        .and_then(|a| a.accounting_epoch_reason.clone());
    let accounting_watermark = accounting.as_ref().map(|a| a.last_applied_inbox_id);
    let accounting_realized_pnl = accounting.as_ref().map(|a| a.realized_pnl_micros);

    let authority = classify_closed_trades_authority(
        provenance,
        proj.canonical_last_applied_inbox_id,
        accounting_watermark,
        proj.sum_gross_realized_pnl_micros,
        accounting_realized_pnl,
    );

    if authority.durable_pnl_mismatch {
        tracing::error!(
            "closed_trades durable parity failure: projection_sum={} durable_realized_pnl={accounting_realized_pnl:?} run_id={run_id}",
            proj.sum_gross_realized_pnl_micros,
        );
    }

    let (sum, epoch_out, epoch_reason_out, fragments) =
        if matches!(authority.truth_state, "active" | "incomplete") {
            (
                Some(proj.sum_gross_realized_pnl_micros),
                epoch,
                epoch_reason,
                proj.fragments,
            )
        } else {
            (None, None, None, vec![])
        };

    AuthoritativeClosedTradeView {
        truth_state: authority.truth_state,
        accounting_epoch: epoch_out,
        accounting_epoch_reason: epoch_reason_out,
        sum_gross_realized_pnl_micros: sum,
        accounting_provenance_state: Some(provenance.as_str()),
        canonical_last_applied_inbox_id: Some(proj.canonical_last_applied_inbox_id),
        accounting_last_applied_inbox_id: accounting_watermark,
        accounting_watermark_state: authority.accounting_watermark_state,
        fragments,
    }
}

#[cfg(test)]
mod authority_tests {
    use super::{classify_closed_trades_authority, PortfolioProvenanceState};

    #[test]
    fn active_provenance_with_matching_watermark_and_pnl_is_active() {
        let authority = classify_closed_trades_authority(
            PortfolioProvenanceState::Active,
            5,
            Some(5),
            100,
            Some(100),
        );
        assert_eq!(authority.truth_state, "active");
        assert_eq!(authority.accounting_watermark_state, Some("same_watermark"));
        assert!(!authority.durable_pnl_mismatch);
    }

    /// RED6 target: CT15's exact counterexample -- realized P&L still
    /// matches, but the durable accounting watermark is stale relative to
    /// the canonical replay. Must never be "active" or "parity_failed".
    #[test]
    fn active_provenance_with_stale_watermark_and_matching_pnl_is_incomplete_not_active() {
        let authority = classify_closed_trades_authority(
            PortfolioProvenanceState::Active,
            6, // canonical watermark advanced past accounting's
            Some(5),
            100,
            Some(100),
        );
        assert_eq!(authority.truth_state, "incomplete");
        assert_eq!(
            authority.accounting_watermark_state,
            Some("accounting_watermark_mismatch")
        );
        assert!(
            !authority.durable_pnl_mismatch,
            "a stale watermark with unchanged realized P&L must never be reported as a P&L contradiction"
        );
    }

    #[test]
    fn active_provenance_with_matching_watermark_but_mismatched_pnl_is_parity_failed() {
        let authority = classify_closed_trades_authority(
            PortfolioProvenanceState::Active,
            5,
            Some(5),
            100,
            Some(999),
        );
        assert_eq!(authority.truth_state, "parity_failed");
        assert_eq!(authority.accounting_watermark_state, Some("same_watermark"));
        assert!(authority.durable_pnl_mismatch);
    }

    /// RED5 target: CT14's exact counterexample -- the shared classifier
    /// reports a stale-snapshot mismatch; realized-P&L equality must not
    /// bypass it.
    #[test]
    fn non_active_provenance_never_reaches_active_regardless_of_watermark_or_pnl_equality() {
        let authority = classify_closed_trades_authority(
            PortfolioProvenanceState::AccountingSnapshotMismatch,
            5,
            Some(5),
            100,
            Some(100),
        );
        assert_eq!(authority.truth_state, "incomplete");
        assert_eq!(authority.accounting_watermark_state, None);
        assert!(!authority.durable_pnl_mismatch);
    }

    #[test]
    fn query_failed_provenance_maps_to_query_failed_not_incomplete() {
        let authority = classify_closed_trades_authority(
            PortfolioProvenanceState::QueryFailed,
            5,
            None,
            100,
            None,
        );
        assert_eq!(authority.truth_state, "query_failed");
        assert_eq!(authority.accounting_watermark_state, None);
    }

    #[test]
    fn every_other_non_active_state_is_incomplete() {
        for provenance in [
            PortfolioProvenanceState::FillHistoryIncomplete,
            PortfolioProvenanceState::AccountingEpochUnavailable,
            PortfolioProvenanceState::NotFound,
            PortfolioProvenanceState::UnsupportedSource,
            PortfolioProvenanceState::InvalidSnapshot,
        ] {
            let authority = classify_closed_trades_authority(provenance, 5, None, 100, None);
            assert_eq!(
                authority.truth_state, "incomplete",
                "provenance={provenance:?}"
            );
            assert_eq!(
                authority.accounting_watermark_state, None,
                "provenance={provenance:?}"
            );
        }
    }
}
