//! mqk-portfolio: constraints
//!
//! Institutional Addendum §8.1 – Constraint checking
//!
//! Provides post-allocation (and general) constraint verification:
//!   - Weight bounds  (per-symbol min/max, portfolio gross/net)
//!   - Sector limits  (gross and net exposure per sector)
//!   - Turnover limit (one-way turnover vs current weights)
//!
//! All functions are pure — no IO, no allocator dependency at runtime.
//! The `Allocator` in `allocator.rs` enforces constraints *during* construction;
//! this module checks constraints *after the fact* and produces violation reports,
//! useful for compliance gating and pre-trade checks.

use std::collections::{BTreeMap, HashMap};

use crate::valuation::{
    compute_portfolio_weights, PortfolioWeightsSnapshot, PositionMark, PositionWeightInput,
};

// ─── ConstraintViolation ──────────────────────────────────────────────────────

/// A single constraint breach detected during validation.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstraintViolation {
    /// A single-position weight exceeds its upper bound.
    WeightTooLarge {
        symbol: String,
        weight: f64,
        limit: f64,
    },
    /// A single-position weight is below its lower bound.
    WeightTooSmall {
        symbol: String,
        weight: f64,
        limit: f64,
    },
    /// Portfolio gross weight (Σ|wᵢ|) exceeds its limit.
    GrossWeightExceeded { actual: f64, limit: f64 },
    /// Portfolio net weight (Σwᵢ) absolute value exceeds its limit.
    NetWeightExceeded { actual: f64, limit: f64 },
    /// A sector's gross exposure exceeds its limit.
    SectorGrossExceeded {
        sector: String,
        actual: f64,
        limit: f64,
    },
    /// A sector's net exposure absolute value exceeds its limit.
    SectorNetExceeded {
        sector: String,
        actual: f64,
        limit: f64,
    },
    /// One-way portfolio turnover exceeds its limit.
    TurnoverExceeded { actual: f64, limit: f64 },
}

impl std::fmt::Display for ConstraintViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WeightTooLarge {
                symbol,
                weight,
                limit,
            } => {
                write!(
                    f,
                    "weight {weight:.4} for '{symbol}' exceeds max {limit:.4}"
                )
            }
            Self::WeightTooSmall {
                symbol,
                weight,
                limit,
            } => {
                write!(f, "weight {weight:.4} for '{symbol}' below min {limit:.4}")
            }
            Self::GrossWeightExceeded { actual, limit } => {
                write!(f, "gross weight {actual:.4} exceeds limit {limit:.4}")
            }
            Self::NetWeightExceeded { actual, limit } => {
                write!(f, "net weight {actual:.4} exceeds limit {limit:.4}")
            }
            Self::SectorGrossExceeded {
                sector,
                actual,
                limit,
            } => {
                write!(
                    f,
                    "sector '{sector}' gross {actual:.4} exceeds limit {limit:.4}"
                )
            }
            Self::SectorNetExceeded {
                sector,
                actual,
                limit,
            } => {
                write!(
                    f,
                    "sector '{sector}' net {actual:.4} exceeds limit {limit:.4}"
                )
            }
            Self::TurnoverExceeded { actual, limit } => {
                write!(f, "one-way turnover {actual:.4} exceeds limit {limit:.4}")
            }
        }
    }
}

// ─── WeightBoundsConstraint ───────────────────────────────────────────────────

/// Per-position and portfolio-level weight bounds.
///
/// All fields are optional; `None` means unconstrained on that dimension.
///
/// Convention: `min_weight` and `max_weight` apply to the *signed* weight
/// (e.g. `min_weight = Some(-0.10)` means no position more than 10 % short;
///  `min_weight = Some(0.0)` enforces long-only).
#[derive(Clone, Debug, PartialEq)]
pub struct WeightBoundsConstraint {
    /// Floor on signed weight for any single position.
    pub min_weight: Option<f64>,
    /// Ceiling on signed weight for any single position.
    pub max_weight: Option<f64>,
    /// Maximum portfolio gross weight Σ|wᵢ|.
    pub max_gross_weight: Option<f64>,
    /// Maximum |Σwᵢ| (absolute net tilt).
    pub max_net_weight: Option<f64>,
}

impl WeightBoundsConstraint {
    /// No bounds — everything permitted.
    pub fn unconstrained() -> Self {
        Self {
            min_weight: None,
            max_weight: None,
            max_gross_weight: None,
            max_net_weight: None,
        }
    }

    /// Standard long-only: weights ∈ [0, 0.20], gross ≤ 1.0.
    pub fn long_only_standard() -> Self {
        Self {
            min_weight: Some(0.0),
            max_weight: Some(0.20),
            max_gross_weight: Some(1.0),
            max_net_weight: None,
        }
    }
}

/// Check a weight map against `WeightBoundsConstraint`.
///
/// Returns every violation found (empty ⇒ all constraints satisfied).
pub fn check_weight_bounds(
    weights: &BTreeMap<String, f64>,
    constraint: &WeightBoundsConstraint,
) -> Vec<ConstraintViolation> {
    let mut violations = Vec::new();

    for (sym, &w) in weights {
        if let Some(mn) = constraint.min_weight {
            if w < mn - 1e-12 {
                violations.push(ConstraintViolation::WeightTooSmall {
                    symbol: sym.clone(),
                    weight: w,
                    limit: mn,
                });
            }
        }
        if let Some(mx) = constraint.max_weight {
            if w > mx + 1e-12 {
                violations.push(ConstraintViolation::WeightTooLarge {
                    symbol: sym.clone(),
                    weight: w,
                    limit: mx,
                });
            }
        }
    }

    let gross: f64 = weights.values().map(|w| w.abs()).sum();
    if let Some(mg) = constraint.max_gross_weight {
        if gross > mg + 1e-12 {
            violations.push(ConstraintViolation::GrossWeightExceeded {
                actual: gross,
                limit: mg,
            });
        }
    }

    let net: f64 = weights.values().sum();
    if let Some(mn) = constraint.max_net_weight {
        if net.abs() > mn + 1e-12 {
            violations.push(ConstraintViolation::NetWeightExceeded {
                actual: net,
                limit: mn,
            });
        }
    }

    violations
}

// ─── SectorConstraint ────────────────────────────────────────────────────────

/// Exposure limit for a single sector.
#[derive(Clone, Debug, PartialEq)]
pub struct SectorConstraint {
    /// The sector identifier (matches values in the sector map).
    pub sector: String,
    /// Maximum gross exposure for this sector: Σ|wᵢ| for symbols in sector.
    pub max_gross_weight: f64,
    /// Maximum net exposure absolute value for this sector (optional).
    pub max_net_weight: Option<f64>,
}

impl SectorConstraint {
    pub fn new<S: Into<String>>(sector: S, max_gross_weight: f64) -> Self {
        Self {
            sector: sector.into(),
            max_gross_weight,
            max_net_weight: None,
        }
    }

    pub fn with_net_cap(mut self, max_net_weight: f64) -> Self {
        self.max_net_weight = Some(max_net_weight);
        self
    }
}

/// Check a weight map against a list of sector constraints.
///
/// `sector_map`: symbol → sector label.  Symbols not in the map are ignored
/// (treated as belonging to no sector).
///
/// Returns every violation found.
pub fn check_sector_limits(
    weights: &BTreeMap<String, f64>,
    sector_map: &HashMap<String, String>,
    constraints: &[SectorConstraint],
) -> Vec<ConstraintViolation> {
    // Aggregate per-sector gross and net.
    let mut sector_gross: HashMap<&str, f64> = HashMap::new();
    let mut sector_net: HashMap<&str, f64> = HashMap::new();

    for (sym, &w) in weights {
        if let Some(sector) = sector_map.get(sym) {
            *sector_gross.entry(sector.as_str()).or_insert(0.0) += w.abs();
            *sector_net.entry(sector.as_str()).or_insert(0.0) += w;
        }
    }

    let mut violations = Vec::new();

    for sc in constraints {
        let gross = sector_gross.get(sc.sector.as_str()).copied().unwrap_or(0.0);
        if gross > sc.max_gross_weight + 1e-12 {
            violations.push(ConstraintViolation::SectorGrossExceeded {
                sector: sc.sector.clone(),
                actual: gross,
                limit: sc.max_gross_weight,
            });
        }
        if let Some(mn) = sc.max_net_weight {
            let net = sector_net.get(sc.sector.as_str()).copied().unwrap_or(0.0);
            if net.abs() > mn + 1e-12 {
                violations.push(ConstraintViolation::SectorNetExceeded {
                    sector: sc.sector.clone(),
                    actual: net,
                    limit: mn,
                });
            }
        }
    }

    violations
}

// ─── Turnover ────────────────────────────────────────────────────────────────

/// Compute one-way portfolio turnover between `current` and `target` weights.
///
/// One-way turnover = Σ|w_target,i − w_current,i| / 2
///
/// This equals the fraction of NAV that must be traded to move from current
/// to target.  A value of 0.10 means 10 % of NAV must change hands.
///
/// Symbols absent from a map are treated as weight 0.
pub fn compute_turnover(current: &BTreeMap<String, f64>, target: &BTreeMap<String, f64>) -> f64 {
    // Union of all symbols.
    let mut all_symbols: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for k in current.keys() {
        all_symbols.insert(k.as_str());
    }
    for k in target.keys() {
        all_symbols.insert(k.as_str());
    }

    let sum_abs_diff: f64 = all_symbols
        .iter()
        .map(|sym| {
            let w_cur = current.get(*sym).copied().unwrap_or(0.0);
            let w_tgt = target.get(*sym).copied().unwrap_or(0.0);
            (w_tgt - w_cur).abs()
        })
        .sum();

    sum_abs_diff / 2.0
}

/// Turnover constraint.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnoverConstraint {
    /// Maximum permitted one-way turnover (fraction of NAV).
    pub max_one_way_turnover: f64,
}

impl TurnoverConstraint {
    pub fn new(max_one_way_turnover: f64) -> Self {
        Self {
            max_one_way_turnover,
        }
    }
}

/// Check whether moving from `current` to `target` violates the turnover limit.
///
/// Returns a single-element vec if violated, empty if satisfied.
pub fn check_turnover(
    current: &BTreeMap<String, f64>,
    target: &BTreeMap<String, f64>,
    constraint: &TurnoverConstraint,
) -> Vec<ConstraintViolation> {
    let turnover = compute_turnover(current, target);
    if turnover > constraint.max_one_way_turnover + 1e-12 {
        vec![ConstraintViolation::TurnoverExceeded {
            actual: turnover,
            limit: constraint.max_one_way_turnover,
        }]
    } else {
        vec![]
    }
}

/// Run all constraint checks in one call and return every violation.
///
/// Convenience wrapper; individual check functions remain available.
pub fn check_all(
    weights: &BTreeMap<String, f64>,
    bounds: &WeightBoundsConstraint,
    sector_map: &HashMap<String, String>,
    sector_constraints: &[SectorConstraint],
    current_weights: Option<&BTreeMap<String, f64>>,
    turnover_constraint: Option<&TurnoverConstraint>,
) -> Vec<ConstraintViolation> {
    let mut all = check_weight_bounds(weights, bounds);
    all.extend(check_sector_limits(weights, sector_map, sector_constraints));
    if let (Some(cur), Some(tc)) = (current_weights, turnover_constraint) {
        all.extend(check_turnover(cur, weights, tc));
    }
    all
}

// ─── Live, bps-based sector exposure gate (ETF-RISK-CLOSURE-01) ──────────────
//
// Distinct from `SectorConstraint`/`check_sector_limits` above, which check a
// caller-supplied *fractional* weight map (`BTreeMap<String, f64>`) — that is
// the research allocator's post-allocation constraint check (Institutional
// Addendum §8.1) and is unrelated to this gate; it is not modified by
// ETF-RISK-CLOSURE-01.
//
// This evaluator is for the live, mark-price-driven admission path: it
// consumes the same integer-basis-point weights that
// `crate::valuation::compute_portfolio_weights` produces, recomputes them
// before and after a candidate order, and never fabricates a mark, NAV, or
// weight — every fail-closed branch is driven by
// `PortfolioWeightsSnapshot::truth_state`, never assumed.

/// Result of evaluating a candidate order against a configured per-sector
/// gross exposure cap, using real live weights (ETF-RISK-CLOSURE-01).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectorRiskEvaluation {
    /// `true` iff the candidate order is safe to admit on sector-risk grounds
    /// alone — other gates may still deny it for unrelated reasons.
    pub allowed: bool,
    /// Human-readable explanation. `None` for simple pass-throughs
    /// (disabled / unclassified symbol / no cap for this sector / within
    /// cap); `Some` whenever a denial or a notable allow (risk-reducing
    /// override) needs explaining.
    pub reason_code: Option<String>,
    /// The candidate symbol's sector, when known.
    pub sector: Option<String>,
    /// Current sector gross exposure (sum of `|weight_bps|` for every symbol
    /// in `sector`), before the candidate order. `None` when not computed
    /// (disabled / unclassified / no limit for this sector / fail-closed).
    pub current_weight_bps: Option<i64>,
    /// Sector gross exposure after applying the candidate order.
    pub prospective_weight_bps: Option<i64>,
    /// The configured cap for `sector`, when one applies.
    pub max_weight_bps: Option<i64>,
    /// Stable machine-readable code, always present. One of:
    /// `"sector_risk_disabled"`, `"sector_metadata_missing"`,
    /// `"sector_limit_ok"`, `"sector_weights_missing"`,
    /// `"sector_nav_unavailable"`, `"sector_risk_reducing_allowed"`,
    /// `"sector_limit_exceeded"`.
    pub truth_state: String,
}

fn sector_risk_passthrough(truth_state: &str, sector: Option<String>) -> SectorRiskEvaluation {
    SectorRiskEvaluation {
        allowed: true,
        reason_code: None,
        sector,
        current_weight_bps: None,
        prospective_weight_bps: None,
        max_weight_bps: None,
        truth_state: truth_state.to_string(),
    }
}

fn sector_risk_fail_closed(
    truth_state: &str,
    sector: String,
    max_weight_bps: i64,
    reason: String,
) -> SectorRiskEvaluation {
    SectorRiskEvaluation {
        allowed: false,
        reason_code: Some(reason),
        sector: Some(sector),
        current_weight_bps: None,
        prospective_weight_bps: None,
        max_weight_bps: Some(max_weight_bps),
        truth_state: truth_state.to_string(),
    }
}

/// `i64::abs()` panics on `i64::MIN`; this saturates instead. Never panics,
/// never silently wraps.
fn abs_i64_saturating(x: i64) -> i64 {
    if x == i64::MIN {
        i64::MAX
    } else {
        x.abs()
    }
}

/// Sum of `|weight_bps|` over every row in `snapshot` whose symbol maps to
/// `sector` in `sector_map`. Rows with `weight_bps = None` (always false for
/// an `"active"` snapshot, but defensively skipped rather than assumed-zero)
/// do not contribute. Saturating sum: never panics, never wraps.
fn sector_gross_weight_bps(
    snapshot: &PortfolioWeightsSnapshot,
    sector_map: &HashMap<String, String>,
    sector: &str,
) -> i64 {
    snapshot
        .positions
        .iter()
        .filter(|row| sector_map.get(&row.symbol).map(|s| s.as_str()) == Some(sector))
        .filter_map(|row| row.weight_bps)
        .fold(0i64, |acc, w| acc.saturating_add(abs_i64_saturating(w)))
}

/// Evaluate a candidate order against configured per-sector gross exposure
/// caps, using real live weights (ETF-RISK-CLOSURE-01).
///
/// Pure: no IO, no env reads, no clock. Callers (the daemon-side gate) are
/// responsible for: fetching `current_positions`/`cash_micros` from the live
/// execution snapshot, fetching `marks` from `md_bars` (leaving a symbol out
/// of `marks` when no mark is available — never fabricating one), loading
/// `sector_map` from the instrument registry, and parsing
/// `sector_limits_bps` from configuration.
///
/// `sector_limits_bps` is `sector -> max_gross_weight_bps`. An empty map
/// means sector risk is disabled: this function returns `allowed: true,
/// truth_state: "sector_risk_disabled"` immediately, without consulting
/// `marks`, `sector_map`, or `current_positions` at all — the caller does
/// not need to fetch any of them when this map is empty.
///
/// # Behavior
///
/// 1. `sector_limits_bps.is_empty()` → disabled. No marks needed.
/// 2. `candidate_symbol` absent from `sector_map` → `"sector_metadata_missing"`,
///    allowed. No marks needed. (Safest default: this gate does not block an
///    untagged symbol; a future patch may add an explicit
///    require-sector-metadata flag if that is ever wanted.)
/// 3. `candidate_symbol`'s sector has no entry in `sector_limits_bps` →
///    `"sector_limit_ok"`, allowed, `sector: Some(..)`, no cap to check. No
///    marks needed.
/// 4. Otherwise, compute the "current" weights snapshot from
///    `current_positions`/`marks`/`cash_micros`, and a "prospective"
///    snapshot with `candidate_signed_delta_qty` applied to
///    `candidate_symbol`'s quantity (inserting a new position if
///    `candidate_symbol` was flat or absent). Both snapshots use the same
///    `marks` (the order does not change any mark) and the same
///    `cash_micros` (this is an exposure/weight check, not a cash-impact
///    check).
///    - Either snapshot's `truth_state == "missing_marks"` → fail closed,
///      `"sector_weights_missing"`.
///    - Either snapshot's `truth_state == "nav_unavailable"` → fail closed,
///      `"sector_nav_unavailable"`.
/// 5. Otherwise, sum `|weight_bps|` over every symbol in the candidate's
///    sector, for both snapshots, giving `current_weight_bps` /
///    `prospective_weight_bps`.
///    - `prospective_weight_bps <= max_weight_bps` → `"sector_limit_ok"`,
///      allowed.
///    - `prospective_weight_bps > max_weight_bps` AND `prospective_weight_bps
///      < current_weight_bps` (the order strictly reduces sector exposure) →
///      `"sector_risk_reducing_allowed"`, allowed — even though the sector
///      remains over cap.
///    - Otherwise → `"sector_limit_exceeded"`, denied.
pub fn evaluate_sector_risk(
    cash_micros: i64,
    current_positions: &[PositionWeightInput],
    marks: &BTreeMap<String, PositionMark>,
    sector_map: &HashMap<String, String>,
    sector_limits_bps: &HashMap<String, i64>,
    candidate_symbol: &str,
    candidate_signed_delta_qty: i64,
) -> SectorRiskEvaluation {
    if sector_limits_bps.is_empty() {
        return sector_risk_passthrough("sector_risk_disabled", None);
    }

    let Some(sector) = sector_map.get(candidate_symbol).cloned() else {
        return sector_risk_passthrough("sector_metadata_missing", None);
    };

    let Some(&max_weight_bps) = sector_limits_bps.get(&sector) else {
        return sector_risk_passthrough("sector_limit_ok", Some(sector));
    };

    let current_snapshot = compute_portfolio_weights(cash_micros, current_positions, marks);

    let mut prospective_positions: Vec<PositionWeightInput> = current_positions.to_vec();
    match prospective_positions
        .iter_mut()
        .find(|p| p.symbol == candidate_symbol)
    {
        Some(p) => p.signed_qty = p.signed_qty.saturating_add(candidate_signed_delta_qty),
        None => prospective_positions.push(PositionWeightInput {
            symbol: candidate_symbol.to_string(),
            signed_qty: candidate_signed_delta_qty,
        }),
    }
    let prospective_snapshot =
        compute_portfolio_weights(cash_micros, &prospective_positions, marks);

    for snapshot in [&current_snapshot, &prospective_snapshot] {
        if snapshot.truth_state == "missing_marks" {
            let reason = format!(
                "sector '{sector}' gross exposure cannot be verified: one or more \
                 positions are missing a live mark (missing_mark_symbols={:?})",
                snapshot.missing_mark_symbols
            );
            return sector_risk_fail_closed(
                "sector_weights_missing",
                sector,
                max_weight_bps,
                reason,
            );
        }
        if snapshot.truth_state == "nav_unavailable" {
            let reason = format!(
                "sector '{sector}' gross exposure cannot be verified: portfolio NAV is \
                 not positive (nav_micros={:?})",
                snapshot.nav_micros
            );
            return sector_risk_fail_closed(
                "sector_nav_unavailable",
                sector,
                max_weight_bps,
                reason,
            );
        }
    }

    let current_weight_bps = sector_gross_weight_bps(&current_snapshot, sector_map, &sector);
    let prospective_weight_bps =
        sector_gross_weight_bps(&prospective_snapshot, sector_map, &sector);
    let is_risk_reducing = prospective_weight_bps < current_weight_bps;
    let within_cap = prospective_weight_bps <= max_weight_bps;

    let (allowed, truth_state, reason_code) = if within_cap {
        (true, "sector_limit_ok", None)
    } else if is_risk_reducing {
        (
            true,
            "sector_risk_reducing_allowed",
            Some(format!(
                "sector '{sector}' prospective gross exposure {prospective_weight_bps} bps \
                 exceeds max_weight_bps={max_weight_bps}, but the order reduces exposure from \
                 {current_weight_bps} bps — allowed as risk-reducing"
            )),
        )
    } else {
        (
            false,
            "sector_limit_exceeded",
            Some(format!(
                "sector '{sector}' prospective gross exposure {prospective_weight_bps} bps \
                 would exceed max_weight_bps={max_weight_bps} (current {current_weight_bps} \
                 bps); order is not risk-reducing"
            )),
        )
    };

    SectorRiskEvaluation {
        allowed,
        reason_code,
        sector: Some(sector),
        current_weight_bps: Some(current_weight_bps),
        prospective_weight_bps: Some(prospective_weight_bps),
        max_weight_bps: Some(max_weight_bps),
        truth_state: truth_state.to_string(),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn wmap(items: &[(&str, f64)]) -> BTreeMap<String, f64> {
        items.iter().map(|(s, w)| (s.to_string(), *w)).collect()
    }

    fn smap(items: &[(&str, &str)]) -> HashMap<String, String> {
        items
            .iter()
            .map(|(sym, sec)| (sym.to_string(), sec.to_string()))
            .collect()
    }

    // ── WeightBoundsConstraint ────────────────────────────────────────────────

    #[test]
    fn no_violations_for_unconstrained() {
        let weights = wmap(&[("SPY", 0.5), ("QQQ", 0.5)]);
        let c = WeightBoundsConstraint::unconstrained();
        assert!(check_weight_bounds(&weights, &c).is_empty());
    }

    #[test]
    fn weight_too_large_detected() {
        let weights = wmap(&[("SPY", 0.30)]);
        let c = WeightBoundsConstraint {
            max_weight: Some(0.20),
            ..WeightBoundsConstraint::unconstrained()
        };
        let v = check_weight_bounds(&weights, &c);
        assert_eq!(v.len(), 1);
        assert!(matches!(
            &v[0],
            ConstraintViolation::WeightTooLarge { symbol, .. } if symbol == "SPY"
        ));
    }

    #[test]
    fn weight_too_small_detected() {
        let weights = wmap(&[("SPY", -0.15)]);
        let c = WeightBoundsConstraint {
            min_weight: Some(0.0), // long-only
            ..WeightBoundsConstraint::unconstrained()
        };
        let v = check_weight_bounds(&weights, &c);
        assert_eq!(v.len(), 1);
        assert!(matches!(
            &v[0],
            ConstraintViolation::WeightTooSmall { symbol, .. } if symbol == "SPY"
        ));
    }

    #[test]
    fn gross_weight_exceeded_detected() {
        let weights = wmap(&[("SPY", 0.6), ("QQQ", 0.6)]);
        let c = WeightBoundsConstraint {
            max_gross_weight: Some(1.0),
            ..WeightBoundsConstraint::unconstrained()
        };
        let v = check_weight_bounds(&weights, &c);
        assert!(v
            .iter()
            .any(|x| matches!(x, ConstraintViolation::GrossWeightExceeded { .. })));
    }

    #[test]
    fn net_weight_exceeded_detected() {
        let weights = wmap(&[("SPY", 0.4), ("QQQ", 0.4)]);
        let c = WeightBoundsConstraint {
            max_net_weight: Some(0.5),
            ..WeightBoundsConstraint::unconstrained()
        };
        let v = check_weight_bounds(&weights, &c);
        assert!(v
            .iter()
            .any(|x| matches!(x, ConstraintViolation::NetWeightExceeded { .. })));
    }

    #[test]
    fn long_only_standard_accepts_valid_portfolio() {
        // 5 equal longs at 0.20 each → gross = 1.0, each ≤ 0.20
        let weights = wmap(&[
            ("SPY", 0.20),
            ("QQQ", 0.20),
            ("IWM", 0.20),
            ("DIA", 0.20),
            ("TLT", 0.20),
        ]);
        let c = WeightBoundsConstraint::long_only_standard();
        assert!(check_weight_bounds(&weights, &c).is_empty());
    }

    #[test]
    fn long_only_standard_rejects_short() {
        let weights = wmap(&[("SPY", -0.10)]);
        let c = WeightBoundsConstraint::long_only_standard();
        let v = check_weight_bounds(&weights, &c);
        assert!(v
            .iter()
            .any(|x| matches!(x, ConstraintViolation::WeightTooSmall { .. })));
    }

    #[test]
    fn empty_weights_no_violations() {
        let weights = wmap(&[]);
        let c = WeightBoundsConstraint {
            max_gross_weight: Some(1.0),
            max_net_weight: Some(0.5),
            min_weight: Some(0.0),
            max_weight: Some(0.25),
        };
        assert!(check_weight_bounds(&weights, &c).is_empty());
    }

    // ── SectorConstraint ──────────────────────────────────────────────────────

    #[test]
    fn sector_gross_violation_detected() {
        let weights = wmap(&[("SPY", 0.40), ("QQQ", 0.40)]);
        let sectors = smap(&[("SPY", "EQUITY"), ("QQQ", "EQUITY")]);
        let constraints = vec![SectorConstraint::new("EQUITY", 0.60)];
        let v = check_sector_limits(&weights, &sectors, &constraints);
        assert_eq!(v.len(), 1);
        assert!(matches!(
            &v[0],
            ConstraintViolation::SectorGrossExceeded { sector, .. } if sector == "EQUITY"
        ));
    }

    #[test]
    fn sector_net_violation_detected() {
        // Both long → net = 0.30, limit = 0.20
        let weights = wmap(&[("SPY", 0.20), ("QQQ", 0.10)]);
        let sectors = smap(&[("SPY", "EQUITY"), ("QQQ", "EQUITY")]);
        let constraints = vec![SectorConstraint::new("EQUITY", 1.0).with_net_cap(0.20)];
        let v = check_sector_limits(&weights, &sectors, &constraints);
        assert_eq!(v.len(), 1);
        assert!(matches!(
            &v[0],
            ConstraintViolation::SectorNetExceeded { sector, .. } if sector == "EQUITY"
        ));
    }

    #[test]
    fn sector_constraint_satisfied_no_violations() {
        let weights = wmap(&[("SPY", 0.20), ("QQQ", 0.20)]);
        let sectors = smap(&[("SPY", "EQUITY"), ("QQQ", "EQUITY")]);
        let constraints = vec![SectorConstraint::new("EQUITY", 0.50)];
        assert!(check_sector_limits(&weights, &sectors, &constraints).is_empty());
    }

    #[test]
    fn symbols_not_in_sector_map_are_ignored() {
        let weights = wmap(&[("BTC", 0.50), ("ETH", 0.50)]);
        let sectors = smap(&[]); // nothing mapped
        let constraints = vec![SectorConstraint::new("CRYPTO", 0.30)];
        // No symbols contribute to CRYPTO → gross = 0 → no violation
        assert!(check_sector_limits(&weights, &sectors, &constraints).is_empty());
    }

    #[test]
    fn long_short_sector_net_within_limit() {
        // SPY long 0.30, TLT short -0.25 → gross = 0.55, net = 0.05
        let weights = wmap(&[("SPY", 0.30), ("TLT", -0.25)]);
        let sectors = smap(&[("SPY", "MIXED"), ("TLT", "MIXED")]);
        let constraints = vec![SectorConstraint::new("MIXED", 0.60).with_net_cap(0.10)];
        assert!(check_sector_limits(&weights, &sectors, &constraints).is_empty());
    }

    // ── compute_turnover ─────────────────────────────────────────────────────

    #[test]
    fn zero_turnover_when_identical() {
        let w = wmap(&[("SPY", 0.50), ("QQQ", 0.50)]);
        assert!((compute_turnover(&w, &w) - 0.0).abs() < 1e-10);
    }

    #[test]
    fn full_turnover_from_empty_to_invested() {
        let current = wmap(&[]);
        let target = wmap(&[("SPY", 0.50), ("QQQ", 0.50)]);
        // Σ|Δ| / 2 = (0.5 + 0.5) / 2 = 0.5
        let t = compute_turnover(&current, &target);
        assert!((t - 0.5).abs() < 1e-10, "got {t}");
    }

    #[test]
    fn partial_rebalance_turnover() {
        // SPY 0.50→0.30 (Δ=0.20), QQQ 0.50→0.70 (Δ=0.20) → one-way = 0.20
        let current = wmap(&[("SPY", 0.50), ("QQQ", 0.50)]);
        let target = wmap(&[("SPY", 0.30), ("QQQ", 0.70)]);
        let t = compute_turnover(&current, &target);
        assert!((t - 0.20).abs() < 1e-10, "got {t}");
    }

    #[test]
    fn turnover_symbol_exit_and_entry() {
        // Exit SPY (0.50→0), enter TLT (0→0.50) → Σ|Δ|/2 = (0.5+0.5)/2 = 0.5
        let current = wmap(&[("SPY", 0.50)]);
        let target = wmap(&[("TLT", 0.50)]);
        let t = compute_turnover(&current, &target);
        assert!((t - 0.50).abs() < 1e-10, "got {t}");
    }

    // ── check_turnover ────────────────────────────────────────────────────────

    #[test]
    fn turnover_within_limit_no_violation() {
        let current = wmap(&[("SPY", 0.50)]);
        let target = wmap(&[("SPY", 0.60)]);
        // one-way = 0.05
        let tc = TurnoverConstraint::new(0.10);
        assert!(check_turnover(&current, &target, &tc).is_empty());
    }

    #[test]
    fn turnover_exceeded_violation() {
        let current = wmap(&[("SPY", 0.50)]);
        let target = wmap(&[("QQQ", 0.50)]);
        // one-way = 0.50
        let tc = TurnoverConstraint::new(0.20);
        let v = check_turnover(&current, &target, &tc);
        assert_eq!(v.len(), 1);
        assert!(matches!(v[0], ConstraintViolation::TurnoverExceeded { .. }));
    }

    // ── check_all ────────────────────────────────────────────────────────────

    #[test]
    fn check_all_aggregates_multiple_violations() {
        // Weight exceeds single cap AND turnover exceeded
        let current = wmap(&[("SPY", 0.50)]);
        let target = wmap(&[("QQQ", 0.50)]); // SPY exits, QQQ enters
        let bounds = WeightBoundsConstraint {
            max_weight: Some(0.30), // QQQ 0.50 > 0.30 → violation
            ..WeightBoundsConstraint::unconstrained()
        };
        let sectors = smap(&[]);
        let tc = TurnoverConstraint::new(0.10); // turnover 0.50 > 0.10 → violation
        let v = check_all(&target, &bounds, &sectors, &[], Some(&current), Some(&tc));
        assert!(v.len() >= 2, "expected ≥2 violations, got {}", v.len());
    }

    #[test]
    fn check_all_no_violations_clean_portfolio() {
        let weights = wmap(&[("SPY", 0.20), ("QQQ", 0.20)]);
        let bounds = WeightBoundsConstraint::long_only_standard();
        let sectors = smap(&[]);
        let v = check_all(&weights, &bounds, &sectors, &[], None, None);
        assert!(v.is_empty());
    }

    // ── Display ───────────────────────────────────────────────────────────────

    #[test]
    fn violation_display_is_non_empty() {
        let cases = vec![
            ConstraintViolation::WeightTooLarge {
                symbol: "SPY".into(),
                weight: 0.30,
                limit: 0.20,
            },
            ConstraintViolation::WeightTooSmall {
                symbol: "SPY".into(),
                weight: -0.10,
                limit: 0.0,
            },
            ConstraintViolation::GrossWeightExceeded {
                actual: 1.20,
                limit: 1.0,
            },
            ConstraintViolation::NetWeightExceeded {
                actual: 0.60,
                limit: 0.50,
            },
            ConstraintViolation::SectorGrossExceeded {
                sector: "EQUITY".into(),
                actual: 0.80,
                limit: 0.60,
            },
            ConstraintViolation::SectorNetExceeded {
                sector: "EQUITY".into(),
                actual: 0.70,
                limit: 0.50,
            },
            ConstraintViolation::TurnoverExceeded {
                actual: 0.40,
                limit: 0.20,
            },
        ];
        for v in cases {
            assert!(!v.to_string().is_empty(), "empty display for {v:?}");
        }
    }
}
