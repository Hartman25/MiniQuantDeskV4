//! mqk-portfolio: allocator
//!
//! Institutional Addendum §8.1 – Portfolio Construction & Allocation
//!
//! Responsibilities (pure, no IO, no broker):
//! - Accept a universe of candidate symbols with expected-return estimates.
//! - Accept equity (NAV) and a set of `AllocationConstraints`.
//! - Produce `AllocationDecision`: a target-weight map (symbol → f64 in [-1, 1])
//!   and a rejection log.
//!
//! Design notes:
//! - Weights are dimensionless fractions of equity (1.0 = 100 % of NAV long).
//! - Negative weights mean short.
//! - The allocator does NOT talk to a broker or read prices; callers supply
//!   pre-computed signal/score values.
//! - Rounding to integer share quantities happens downstream (execution layer).
//! - This module is intentionally constraint-free by default; callers compose
//!   constraints via `AllocationConstraints`.

use std::collections::BTreeMap;

// ─── Error ───────────────────────────────────────────────────────────────────

/// Errors produced during allocation.
#[derive(Clone, Debug, PartialEq)]
pub enum AllocationError {
    /// Equity NAV is zero or negative; cannot compute weights.
    NonPositiveEquity,
    /// A candidate symbol is an empty string.
    EmptySymbol,
    /// A score value is NaN or infinite.
    InvalidScore { symbol: String },
    /// Maximum position count constraint is zero.
    ZeroMaxPositions,
}

impl std::fmt::Display for AllocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonPositiveEquity => write!(f, "equity NAV must be > 0"),
            Self::EmptySymbol => write!(f, "candidate symbol must not be empty"),
            Self::InvalidScore { symbol } => {
                write!(f, "invalid (NaN/inf) score for symbol '{symbol}'")
            }
            Self::ZeroMaxPositions => write!(f, "max_positions constraint must be > 0"),
        }
    }
}

impl std::error::Error for AllocationError {}

// ─── Candidate ───────────────────────────────────────────────────────────────

/// A single candidate instrument for allocation consideration.
///
/// `score` is a real-valued signal (e.g. expected return, alpha estimate).
/// A positive score implies a long bias; negative implies short.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub symbol: String,
    /// Dimensionless signal in any real-valued range.
    pub score: f64,
}

impl Candidate {
    pub fn new<S: Into<String>>(symbol: S, score: f64) -> Self {
        Self {
            symbol: symbol.into(),
            score,
        }
    }
}

// ─── AllocationConstraints ───────────────────────────────────────────────────

/// Constraints applied during allocation.
///
/// Each field is independently optional; `None` means unconstrained.
///
/// # Constraint semantics
/// - `max_gross_weight`: sum of |w_i| ≤ this (e.g. 1.5 = 150 % gross leverage).
/// - `max_net_weight`: |sum of w_i| ≤ this (e.g. 0.2 = 20 % max net tilt).
/// - `max_single_weight`: |w_i| ≤ this for every symbol.
/// - `max_positions`: at most N non-zero allocations emitted.
#[derive(Clone, Debug, PartialEq)]
pub struct AllocationConstraints {
    pub max_gross_weight: Option<f64>,
    pub max_net_weight: Option<f64>,
    pub max_single_weight: Option<f64>,
    pub max_positions: Option<usize>,
}

impl AllocationConstraints {
    /// No constraints — everything is permitted.
    pub fn unconstrained() -> Self {
        Self {
            max_gross_weight: None,
            max_net_weight: None,
            max_single_weight: None,
            max_positions: None,
        }
    }

    /// Typical long-only constraints:
    /// - gross ≤ 1.0 (fully invested, no leverage)
    /// - net ≤ 1.0 (trivially satisfied for long-only)
    /// - single ≤ 0.20 (20 % position cap)
    pub fn long_only_standard() -> Self {
        Self {
            max_gross_weight: Some(1.0),
            max_net_weight: Some(1.0),
            max_single_weight: Some(0.20),
            max_positions: None,
        }
    }
}

impl Default for AllocationConstraints {
    fn default() -> Self {
        Self::unconstrained()
    }
}

// ─── AllocationDecision ──────────────────────────────────────────────────────

/// Why a candidate was excluded from the final allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectionReason {
    /// Excluded because `max_positions` was reached.
    MaxPositionsReached,
    /// Weight was scaled down to zero by gross-leverage cap.
    ScaledToZero,
    /// Net-weight constraint forced exclusion of this position.
    NetWeightCapExceeded,
}

/// A candidate that was considered but not allocated.
#[derive(Clone, Debug, PartialEq)]
pub struct RejectedCandidate {
    pub symbol: String,
    pub score: f64,
    pub reason: RejectionReason,
}

/// The output of one allocation run.
///
/// `weights`: symbol → target weight in [-1.0, +1.0] (fraction of NAV).
/// `rejected`: candidates that were considered but not allocated.
/// `gross_weight`: sum |w_i| of the final portfolio.
/// `net_weight`: sum w_i of the final portfolio.
#[derive(Clone, Debug, PartialEq)]
pub struct AllocationDecision {
    pub weights: BTreeMap<String, f64>,
    pub rejected: Vec<RejectedCandidate>,
    pub gross_weight: f64,
    pub net_weight: f64,
}

impl AllocationDecision {
    /// Returns true if no positions were allocated.
    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }

    /// Number of non-zero allocated positions.
    pub fn position_count(&self) -> usize {
        self.weights.len()
    }
}

// ─── Allocator ───────────────────────────────────────────────────────────────

/// Portfolio allocator — converts scored candidates into target weights.
///
/// # Algorithm (equal-weight signal-sign, then constraint trimming)
///
/// 1. Validate inputs.
/// 2. Sort candidates by |score| descending (highest-conviction first).
/// 3. Apply `max_positions` by truncating the sorted list.
/// 4. Assign raw weight = score / sum(|score|) so weights are normalised.
///    - If all scores are zero, assign 0 to everything.
/// 5. Clip each |weight| to `max_single_weight` if set.
/// 6. Scale the whole portfolio down if gross weight exceeds `max_gross_weight`.
/// 7. Check net weight; if exceeded, attempt to trim the smallest positions
///    until net weight is within bound (or all positions removed).
/// 8. Return `AllocationDecision`.
///
/// This is an intentionally simple, transparent algorithm.  A production
/// system may swap in a quadratic-programming solver; this module's public
/// API surface is stable regardless.
pub struct Allocator {
    constraints: AllocationConstraints,
}

impl Allocator {
    /// Create an allocator with the given constraints.
    pub fn new(constraints: AllocationConstraints) -> Self {
        Self { constraints }
    }

    /// Create an allocator with no constraints.
    pub fn unconstrained() -> Self {
        Self::new(AllocationConstraints::unconstrained())
    }

    pub fn constraints(&self) -> &AllocationConstraints {
        &self.constraints
    }

    /// Run allocation against the given universe and NAV.
    ///
    /// `equity_micros` — NAV in micros (only used to gate non-positive check).
    /// `candidates`     — universe; order does not matter, algorithm re-sorts.
    pub fn allocate(
        &self,
        equity_micros: i64,
        candidates: &[Candidate],
    ) -> Result<AllocationDecision, AllocationError> {
        // ── 0. Guard inputs ──────────────────────────────────────────────────
        if equity_micros <= 0 {
            return Err(AllocationError::NonPositiveEquity);
        }

        if let Some(mp) = self.constraints.max_positions {
            if mp == 0 {
                return Err(AllocationError::ZeroMaxPositions);
            }
        }

        for c in candidates {
            if c.symbol.is_empty() {
                return Err(AllocationError::EmptySymbol);
            }
            if !c.score.is_finite() {
                return Err(AllocationError::InvalidScore {
                    symbol: c.symbol.clone(),
                });
            }
        }

        // ── 1. Sort by |score| desc ──────────────────────────────────────────
        let mut sorted: Vec<&Candidate> = candidates.iter().collect();
        sorted.sort_by(|a, b| {
            b.score
                .abs()
                .partial_cmp(&a.score.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // ── 2. max_positions truncation ──────────────────────────────────────
        let mut rejected: Vec<RejectedCandidate> = Vec::new();

        let keep_count = if let Some(mp) = self.constraints.max_positions {
            mp.min(sorted.len())
        } else {
            sorted.len()
        };

        for c in sorted.iter().skip(keep_count) {
            rejected.push(RejectedCandidate {
                symbol: c.symbol.clone(),
                score: c.score,
                reason: RejectionReason::MaxPositionsReached,
            });
        }

        let active: Vec<&Candidate> = sorted.into_iter().take(keep_count).collect();

        // ── 3. Normalise scores → raw weights ────────────────────────────────
        let total_abs_score: f64 = active.iter().map(|c| c.score.abs()).sum();

        let mut weights: BTreeMap<String, f64> = BTreeMap::new();

        if total_abs_score > 0.0 {
            for c in &active {
                let w = c.score / total_abs_score;
                weights.insert(c.symbol.clone(), w);
            }
        } else {
            // All scores zero → assign 0 to all (they'll be pruned later)
            for c in &active {
                weights.insert(c.symbol.clone(), 0.0);
            }
        }

        // ── 4. Clip per-position max ──────────────────────────────────────────
        if let Some(max_single) = self.constraints.max_single_weight {
            for w in weights.values_mut() {
                let sign = w.signum();
                if w.abs() > max_single {
                    *w = sign * max_single;
                }
            }
        }

        // ── 5. Scale for gross leverage cap ──────────────────────────────────
        let gross: f64 = weights.values().map(|w| w.abs()).sum();
        if let Some(max_gross) = self.constraints.max_gross_weight {
            if gross > max_gross && gross > 0.0 {
                let scale = max_gross / gross;
                for w in weights.values_mut() {
                    *w *= scale;
                }
            }
        }

        // ── 6. Net weight check ───────────────────────────────────────────────
        if let Some(max_net) = self.constraints.max_net_weight {
            let net: f64 = weights.values().sum();
            if net.abs() > max_net {
                // Simple trim: remove smallest-|weight| positions until net OK.
                // Collect sorted by |weight| ascending.
                let mut order: Vec<String> = weights.keys().cloned().collect();
                order.sort_by(|a, b| {
                    let wa = weights[a].abs();
                    let wb = weights[b].abs();
                    wa.partial_cmp(&wb).unwrap_or(std::cmp::Ordering::Equal)
                });

                for sym in order {
                    let net_check: f64 = weights.values().sum();
                    if net_check.abs() <= max_net {
                        break;
                    }
                    let w = weights.remove(&sym).unwrap_or(0.0);
                    // Find original candidate to record rejection
                    if let Some(c) = active.iter().find(|c| c.symbol == sym) {
                        rejected.push(RejectedCandidate {
                            symbol: c.symbol.clone(),
                            score: c.score,
                            reason: RejectionReason::NetWeightCapExceeded,
                        });
                    } else {
                        rejected.push(RejectedCandidate {
                            symbol: sym.clone(),
                            score: 0.0,
                            reason: RejectionReason::NetWeightCapExceeded,
                        });
                    }
                    // If the removed weight was itself very small, check again
                    let _ = w;
                }
            }
        }

        // ── 7. Prune zero-weight entries ─────────────────────────────────────
        weights.retain(|_, w| *w != 0.0);

        // ── 8. Final metrics ──────────────────────────────────────────────────
        let final_gross: f64 = weights.values().map(|w| w.abs()).sum();
        let final_net: f64 = weights.values().sum();

        Ok(AllocationDecision {
            weights,
            rejected,
            gross_weight: final_gross,
            net_weight: final_net,
        })
    }
}

impl Default for Allocator {
    fn default() -> Self {
        Self::unconstrained()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const NAV: i64 = 100_000 * 1_000_000; // $100 000 in micros

    fn cand(sym: &str, score: f64) -> Candidate {
        Candidate::new(sym, score)
    }

    // ── Input validation ─────────────────────────────────────────────────────

    #[test]
    fn rejects_non_positive_equity() {
        let a = Allocator::unconstrained();
        assert_eq!(
            a.allocate(0, &[cand("SPY", 1.0)]).unwrap_err(),
            AllocationError::NonPositiveEquity
        );
        assert_eq!(
            a.allocate(-1, &[cand("SPY", 1.0)]).unwrap_err(),
            AllocationError::NonPositiveEquity
        );
    }

    #[test]
    fn rejects_empty_symbol() {
        let a = Allocator::unconstrained();
        assert_eq!(
            a.allocate(NAV, &[cand("", 1.0)]).unwrap_err(),
            AllocationError::EmptySymbol
        );
    }

    #[test]
    fn rejects_nan_score() {
        let a = Allocator::unconstrained();
        let err = a.allocate(NAV, &[cand("SPY", f64::NAN)]).unwrap_err();
        assert_eq!(
            err,
            AllocationError::InvalidScore {
                symbol: "SPY".to_string()
            }
        );
    }

    #[test]
    fn rejects_inf_score() {
        let a = Allocator::unconstrained();
        let err = a.allocate(NAV, &[cand("SPY", f64::INFINITY)]).unwrap_err();
        assert_eq!(
            err,
            AllocationError::InvalidScore {
                symbol: "SPY".to_string()
            }
        );
    }

    #[test]
    fn rejects_zero_max_positions_constraint() {
        let a = Allocator::new(AllocationConstraints {
            max_positions: Some(0),
            ..AllocationConstraints::unconstrained()
        });
        assert_eq!(
            a.allocate(NAV, &[cand("SPY", 1.0)]).unwrap_err(),
            AllocationError::ZeroMaxPositions
        );
    }

    // ── Empty universe ────────────────────────────────────────────────────────

    #[test]
    fn empty_universe_returns_empty_decision() {
        let a = Allocator::unconstrained();
        let dec = a.allocate(NAV, &[]).unwrap();
        assert!(dec.is_empty());
        assert_eq!(dec.gross_weight, 0.0);
        assert_eq!(dec.net_weight, 0.0);
        assert!(dec.rejected.is_empty());
    }

    // ── Basic normalisation ───────────────────────────────────────────────────

    #[test]
    fn single_candidate_gets_full_weight() {
        let a = Allocator::unconstrained();
        let dec = a.allocate(NAV, &[cand("SPY", 2.0)]).unwrap();
        let w = dec.weights["SPY"];
        assert!((w - 1.0).abs() < 1e-10, "expected 1.0 got {w}");
        assert!((dec.gross_weight - 1.0).abs() < 1e-10);
        assert!((dec.net_weight - 1.0).abs() < 1e-10);
    }

    #[test]
    fn two_equal_long_candidates_split_evenly() {
        let a = Allocator::unconstrained();
        let dec = a
            .allocate(NAV, &[cand("SPY", 1.0), cand("QQQ", 1.0)])
            .unwrap();
        let ws = &dec.weights["SPY"];
        let wq = &dec.weights["QQQ"];
        assert!((ws - 0.5).abs() < 1e-10, "SPY={ws}");
        assert!((wq - 0.5).abs() < 1e-10, "QQQ={wq}");
        assert!((dec.gross_weight - 1.0).abs() < 1e-10);
    }

    #[test]
    fn short_signal_produces_negative_weight() {
        let a = Allocator::unconstrained();
        let dec = a.allocate(NAV, &[cand("SPY", -1.0)]).unwrap();
        let w = dec.weights["SPY"];
        assert!((w + 1.0).abs() < 1e-10, "expected -1.0 got {w}");
    }

    #[test]
    fn long_short_portfolio_net_near_zero() {
        let a = Allocator::unconstrained();
        let dec = a
            .allocate(NAV, &[cand("SPY", 1.0), cand("TLT", -1.0)])
            .unwrap();
        assert!((dec.net_weight).abs() < 1e-10, "net={}", dec.net_weight);
        assert!((dec.gross_weight - 1.0).abs() < 1e-10);
    }

    #[test]
    fn all_zero_scores_produce_empty_decision() {
        let a = Allocator::unconstrained();
        let dec = a
            .allocate(NAV, &[cand("SPY", 0.0), cand("QQQ", 0.0)])
            .unwrap();
        // Zero weights are pruned.
        assert!(
            dec.is_empty(),
            "expected empty decision, got {:?}",
            dec.weights
        );
    }

    // ── max_positions ─────────────────────────────────────────────────────────

    #[test]
    fn max_positions_truncates_lowest_conviction() {
        let a = Allocator::new(AllocationConstraints {
            max_positions: Some(1),
            ..AllocationConstraints::unconstrained()
        });
        let dec = a
            .allocate(NAV, &[cand("SPY", 3.0), cand("QQQ", 1.0), cand("TLT", 0.5)])
            .unwrap();
        assert_eq!(dec.position_count(), 1);
        assert!(dec.weights.contains_key("SPY"), "highest-conviction kept");
        assert_eq!(dec.rejected.len(), 2);
        assert!(dec
            .rejected
            .iter()
            .all(|r| r.reason == RejectionReason::MaxPositionsReached));
    }

    // ── max_single_weight ─────────────────────────────────────────────────────

    #[test]
    fn single_weight_cap_clips_dominant_position() {
        let a = Allocator::new(AllocationConstraints {
            max_single_weight: Some(0.30),
            ..AllocationConstraints::unconstrained()
        });
        let dec = a.allocate(NAV, &[cand("SPY", 10.0)]).unwrap();
        let w = dec.weights["SPY"];
        assert!(w <= 0.30 + 1e-10, "weight {w} exceeds single cap 0.30");
    }

    #[test]
    fn single_weight_cap_respects_short_sign() {
        let a = Allocator::new(AllocationConstraints {
            max_single_weight: Some(0.25),
            ..AllocationConstraints::unconstrained()
        });
        let dec = a.allocate(NAV, &[cand("TLT", -5.0)]).unwrap();
        let w = dec.weights["TLT"];
        assert!(w < 0.0, "should be negative");
        assert!(
            w.abs() <= 0.25 + 1e-10,
            "|weight| {:.4} exceeds single cap 0.25",
            w.abs()
        );
    }

    // ── max_gross_weight ──────────────────────────────────────────────────────

    #[test]
    fn gross_leverage_cap_scales_all_weights() {
        let a = Allocator::new(AllocationConstraints {
            max_gross_weight: Some(0.5),
            ..AllocationConstraints::unconstrained()
        });
        let dec = a
            .allocate(NAV, &[cand("SPY", 1.0), cand("QQQ", 1.0)])
            .unwrap();
        assert!(
            (dec.gross_weight - 0.5).abs() < 1e-9,
            "gross={:.6}",
            dec.gross_weight
        );
    }

    // ── long_only_standard preset ─────────────────────────────────────────────

    #[test]
    fn long_only_standard_caps_single_at_20pct() {
        let a = Allocator::new(AllocationConstraints::long_only_standard());
        // 6 equal-signal longs → raw 1/6 ≈ 16.7 % each, within 20 % cap
        let candidates: Vec<Candidate> = ["SPY", "QQQ", "IWM", "DIA", "TLT", "GLD"]
            .iter()
            .map(|s| cand(s, 1.0))
            .collect();
        let dec = a.allocate(NAV, &candidates).unwrap();
        for (sym, w) in &dec.weights {
            assert!(
                *w <= 0.20 + 1e-9,
                "symbol {sym} weight {w:.4} exceeds 20 % cap"
            );
            assert!(*w >= 0.0, "long-only: no short weights expected");
        }
    }

    // ── AllocationDecision helpers ────────────────────────────────────────────

    #[test]
    fn decision_is_empty_and_position_count() {
        let a = Allocator::unconstrained();
        let dec = a.allocate(NAV, &[]).unwrap();
        assert!(dec.is_empty());
        assert_eq!(dec.position_count(), 0);

        let dec2 = a
            .allocate(NAV, &[cand("SPY", 1.0), cand("QQQ", 1.0)])
            .unwrap();
        assert!(!dec2.is_empty());
        assert_eq!(dec2.position_count(), 2);
    }

    // ── AllocationError Display ───────────────────────────────────────────────

    #[test]
    fn allocation_error_display() {
        assert!(!AllocationError::NonPositiveEquity.to_string().is_empty());
        assert!(!AllocationError::EmptySymbol.to_string().is_empty());
        assert!(!AllocationError::InvalidScore { symbol: "X".into() }
            .to_string()
            .is_empty());
        assert!(!AllocationError::ZeroMaxPositions.to_string().is_empty());
    }
}
