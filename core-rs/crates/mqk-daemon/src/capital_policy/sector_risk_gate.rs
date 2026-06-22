//! ETF-RISK-EXTERNAL-SIGNAL-GATE-01: shared sector-risk admission glue.
//!
//! `capital_policy::sector_risk` is pure (env parsing only — see its module
//! docs). This module is the one async, DB-touching exception in the
//! `capital_policy` package: it exists solely so the internal decision path
//! (`decision.rs`'s Gate 1h) and the external HTTP signal path
//! (`routes/strategy.rs`) share one mechanism instead of each
//! re-implementing the registry/snapshot/marks glue around the pure
//! `mqk_portfolio::evaluate_sector_risk` evaluator. Before this patch that
//! glue lived inline in `decision.rs`; it is now here, called by both paths.
//!
//! [`evaluate_sector_risk_gate`]:
//! 1. Parses `MQK_SECTOR_EXPOSURE_LIMITS_BPS` (delegates to
//!    [`super::sector_risk::sector_exposure_limits_bps_from_env`]).
//! 2. Short-circuits when disabled, or when the candidate symbol is
//!    untagged or its sector carries no configured cap — no DB or live
//!    snapshot is touched in either case, matching the pure evaluator's own
//!    no-marks-needed early exits.
//! 3. Loads the instrument registry `sector_map`.
//! 4. Loads the live execution snapshot (positions/cash) — only when the
//!    candidate's sector is actually capped.
//! 5. Loads the latest completed `md_bars` mark for every non-flat symbol in
//!    that sector plus the candidate — only when a cap applies, and only
//!    ever via the same `md_bars` latest-completed-row source used by
//!    `PORTFOLIO-LIVE-WEIGHTS-01` (never order price, last fill, or a
//!    caller-supplied quote).
//! 6. Calls the pure `mqk_portfolio::evaluate_sector_risk` evaluator.
//! 7. Returns a route/path-neutral [`SectorRiskGateResult`] — callers map it
//!    to their own disposition/HTTP vocabulary.
//!
//! Risk-reducing determination is never taken from a caller-supplied claim:
//! `side`/`qty` (already-validated fields both callers already collect) are
//! converted to a signed delta here, exactly as `decision.rs` already did
//! before this patch; the evaluator alone decides risk-reducing status from
//! real current/prospective weights.

use std::collections::BTreeMap;

use crate::state::AppState;

/// Outcome of the shared sector-risk admission glue. Route/path-neutral:
/// both `decision.rs` (Gate 1h) and `routes/strategy.rs` (external signal
/// gate) map this to their own disposition strings and HTTP status codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SectorRiskGateResult {
    /// `true` iff sector risk does not block this candidate order.
    pub(crate) allowed: bool,
    /// Stable machine-readable code. Mirrors
    /// `mqk_portfolio::SectorRiskEvaluation::truth_state` for every outcome
    /// produced by the pure evaluator, plus two gate-level codes that
    /// precede evaluation: `"sector_config_invalid"` (malformed env var) and
    /// `"unavailable"` (instrument registry could not be loaded).
    pub(crate) reason_code: String,
    /// Operator-readable explanation, with no path-specific framing words —
    /// callers prepend their own (e.g. "internal decision refused: " /
    /// "strategy signal refused: "). `None` for clean pass-throughs.
    pub(crate) message: Option<String>,
    /// The candidate symbol's sector, when known.
    pub(crate) sector: Option<String>,
    pub(crate) current_weight_bps: Option<i64>,
    pub(crate) prospective_weight_bps: Option<i64>,
    pub(crate) max_weight_bps: Option<i64>,
}

impl SectorRiskGateResult {
    fn passthrough(reason_code: &str, sector: Option<String>) -> Self {
        Self {
            allowed: true,
            reason_code: reason_code.to_string(),
            message: None,
            sector,
            current_weight_bps: None,
            prospective_weight_bps: None,
            max_weight_bps: None,
        }
    }

    fn fail_closed(
        reason_code: &str,
        message: String,
        sector: Option<String>,
        max_weight_bps: Option<i64>,
    ) -> Self {
        Self {
            allowed: false,
            reason_code: reason_code.to_string(),
            message: Some(message),
            sector,
            current_weight_bps: None,
            prospective_weight_bps: None,
            max_weight_bps,
        }
    }

    fn from_evaluation(eval: mqk_portfolio::SectorRiskEvaluation) -> Self {
        Self {
            allowed: eval.allowed,
            reason_code: eval.truth_state,
            message: eval.reason_code,
            sector: eval.sector,
            current_weight_bps: eval.current_weight_bps,
            prospective_weight_bps: eval.prospective_weight_bps,
            max_weight_bps: eval.max_weight_bps,
        }
    }
}

/// Same mark source as `PORTFOLIO-LIVE-WEIGHTS-01` and the pre-existing
/// internal-path Gate 1h: latest completed daily bar close.
const SECTOR_RISK_MARK_TIMEFRAME: &str = "1D";

/// Evaluate the shared sector-risk gate for a candidate order.
///
/// `side` is matched case-insensitively against `"buy"`; any other value is
/// treated as a sell (matching both callers' own Gate-0 validation, which
/// already restricts `side` to `"buy" | "sell"` before this gate runs).
/// `qty` must already be validated positive by the caller.
pub(crate) async fn evaluate_sector_risk_gate(
    state: &AppState,
    candidate_symbol: &str,
    side: &str,
    qty: i64,
) -> SectorRiskGateResult {
    let sector_limits_bps = match super::sector_risk::sector_exposure_limits_bps_from_env() {
        Ok(limits) => limits,
        Err(parse_err) => {
            return SectorRiskGateResult::fail_closed(
                "sector_config_invalid",
                format!("MQK_SECTOR_EXPOSURE_LIMITS_BPS is set but malformed: {parse_err}"),
                None,
                None,
            );
        }
    };

    if sector_limits_bps.is_empty() {
        return SectorRiskGateResult::passthrough("sector_risk_disabled", None);
    }

    let candidate_symbol = candidate_symbol.trim();
    let registry_path = std::path::PathBuf::from(&state.instrument_registry_path);
    let sector_map = match mqk_md::instrument_registry::load_instrument_registry(&registry_path) {
        Ok(instruments) => mqk_md::instrument_registry::sector_map(&instruments),
        Err(err) => {
            return SectorRiskGateResult::fail_closed(
                "unavailable",
                format!("instrument registry could not be loaded for the sector risk gate: {err}"),
                None,
                None,
            );
        }
    };

    // Only fetch the live snapshot/marks when the candidate symbol is
    // actually in a capped sector — an untagged symbol or a sector with no
    // configured cap never needs them, matching the pure evaluator's own
    // no-marks-needed early exits.
    let candidate_sector = sector_map.get(candidate_symbol).cloned();
    let needs_live_weights = candidate_sector
        .as_deref()
        .is_some_and(|sector| sector_limits_bps.contains_key(sector));

    if !needs_live_weights {
        let reason_code = if candidate_sector.is_some() {
            "sector_limit_ok"
        } else {
            "sector_metadata_missing"
        };
        return SectorRiskGateResult::passthrough(reason_code, candidate_sector);
    }

    let max_weight_bps = candidate_sector
        .as_deref()
        .and_then(|sector| sector_limits_bps.get(sector))
        .copied();

    let Some(snapshot) = state.current_execution_snapshot().await else {
        return SectorRiskGateResult::fail_closed(
            "sector_weights_missing",
            format!(
                "sector risk is configured for '{candidate_symbol}'s sector but no live \
                 execution snapshot is available to compute current weights"
            ),
            candidate_sector,
            max_weight_bps,
        );
    };

    let Some(db) = state.db.as_ref() else {
        return SectorRiskGateResult::fail_closed(
            "sector_weights_missing",
            format!(
                "sector risk is configured for '{candidate_symbol}'s sector but no database \
                 is available to fetch live marks"
            ),
            candidate_sector,
            max_weight_bps,
        );
    };

    let delta: i64 = if side.trim().eq_ignore_ascii_case("buy") {
        qty
    } else {
        -qty
    };

    let cash_micros = snapshot.portfolio.cash_micros;
    let current_positions: Vec<mqk_portfolio::PositionWeightInput> = snapshot
        .portfolio
        .positions
        .iter()
        .map(|p| mqk_portfolio::PositionWeightInput {
            symbol: p.symbol.clone(),
            signed_qty: p.net_qty,
        })
        .collect();

    let mut symbols_needing_marks: Vec<String> = current_positions
        .iter()
        .filter(|p| p.signed_qty != 0)
        .map(|p| p.symbol.clone())
        .collect();
    if !symbols_needing_marks.iter().any(|s| s == candidate_symbol) {
        symbols_needing_marks.push(candidate_symbol.to_string());
    }

    let mut marks: BTreeMap<String, mqk_portfolio::PositionMark> = BTreeMap::new();
    for symbol in &symbols_needing_marks {
        match mqk_db::fetch_recent_completed_bars_for_strategy(
            db,
            symbol,
            SECTOR_RISK_MARK_TIMEFRAME,
            1,
        )
        .await
        {
            Ok(bars) => {
                if let Some(bar) = bars.last() {
                    marks.insert(
                        symbol.clone(),
                        mqk_portfolio::PositionMark {
                            symbol: symbol.clone(),
                            mark_price_micros: bar.close_micros,
                            mark_ts_utc: Some(bar.end_ts),
                            source: format!("md_bars:{SECTOR_RISK_MARK_TIMEFRAME}:close"),
                        },
                    );
                }
            }
            Err(err) => {
                tracing::warn!("sector risk gate: md_bars lookup failed for {symbol}: {err}");
            }
        }
    }

    let evaluation = mqk_portfolio::evaluate_sector_risk(
        cash_micros,
        &current_positions,
        &marks,
        &sector_map,
        &sector_limits_bps,
        candidate_symbol,
        delta,
    );

    SectorRiskGateResult::from_evaluation(evaluation)
}
