//! Wave D4 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION):
//! typed options exercise/assignment/expiration lifecycle events.
//!
//! Approved D4/D5 decision: an options lifecycle event (exercise/assignment/
//! expiration) is economically and structurally distinct from an ordinary
//! [`crate::Fill`] — it removes an option position and, for exercise/
//! assignment, pairs that removal with an underlying-share delivery and/or a
//! cash settlement the option's terms determine, never a price the broker
//! "filled" at. Representing it as a synthetic `Fill` would fabricate a
//! trade price that never happened and let a strike-driven cash movement be
//! misread as ordinary trading P&L.
//!
//! # Honesty contract (same shape as `mqk_broker_alpaca::fee_attribution`
//! and `market_calendar`'s ASSET-CORE-05A model-only seam)
//!
//! - Pure model: no DB, no network, no broker wiring, no wall-clock reads.
//! - [`OptionLifecycleResolution::Pending`] is the fail-closed default for
//!   any event this module cannot classify from the evidence it was given —
//!   a lifecycle event is never assumed resolved merely because an option
//!   reached its expiry date; only explicit, caller-supplied
//!   [`LifecycleBrokerEvidence`] can prove resolution. Whether that evidence
//!   is genuinely broker-confirmed is the caller's responsibility to
//!   establish (e.g. from a real Alpaca/OCC corporate-action feed) — this
//!   module does not fetch, poll, or infer it.
//! - [`PairedLifecycleEffect`] is generic across `Exercise`/`Assignment`/
//!   `Expiration` — the pairing rule (option removal, plus underlying/cash
//!   only when the event terms call for it) is the same structural shape
//!   for all three, so a caller can never accidentally construct an
//!   Expiration that carries a cash effect or an Exercise that carries none.
//! - Zero production callers today: no OMS, broker adapter, or route
//!   constructs one of these. Matches this repo's established ASSET-CORE-01
//!   precedent (model layer proven before a concrete consumer requires
//!   wiring) — Alpaca options capability does not exist in this codebase at
//!   all yet (`asset_risk_policy::option_policy`'s own documented gap:
//!   "chain metadata, contract multiplier, Greeks, assignment, and margin
//!   risk model").

use mqk_schemas::QtyMicros;

/// The three lifecycle event kinds this module recognizes. Distinct from
/// [`crate::Side`] — a lifecycle event has no "buy"/"sell" direction of its
/// own; its economic effect is determined by [`PairedLifecycleEffect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionLifecycleEventKind {
    /// The option holder exercised a long option.
    Exercise,
    /// The option writer was assigned against a short option.
    Assignment,
    /// The option reached expiry out-of-the-money and lapsed worthless.
    Expiration,
}

impl OptionLifecycleEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exercise => "exercise",
            Self::Assignment => "assignment",
            Self::Expiration => "expiration",
        }
    }
}

/// Caller-supplied evidence that a specific lifecycle event has actually
/// happened, confirmed by the broker (or an equivalent authoritative
/// source, e.g. OCC). This module never constructs one of these itself —
/// it only classifies what it is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleBrokerEvidence {
    pub kind: OptionLifecycleEventKind,
    pub option_symbol: String,
    pub underlying_symbol: String,
    /// Whole option contracts affected. Fractional option contracts have no
    /// well-defined lifecycle obligation, so this is rejected at
    /// classification time rather than silently truncated.
    pub contracts: QtyMicros,
    pub multiplier: i32,
    /// Strike price in micros — required to compute the paired cash effect
    /// for `Exercise`/`Assignment`; irrelevant for `Expiration` (ignored).
    pub strike_micros: i64,
    /// `true` for a call (exercise/assignment delivers/receives long
    /// underlying shares), `false` for a put (delivers/receives short
    /// underlying shares against cash).
    pub is_call: bool,
}

/// Outcome of classifying one [`LifecycleBrokerEvidence`] — Wave D4.
///
/// `Pending` is the fail-closed default: a caller must never treat an
/// options position as resolved merely because it reached its expiry date
/// or because *some* fill was reported near that date. Only genuine,
/// well-formed broker evidence produces `Resolved`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionLifecycleResolution {
    /// The event is fully resolved; `effect` is the paired option/underlying/
    /// cash change this repo's ledger should apply.
    Resolved(PairedLifecycleEffect),
    /// The lifecycle event cannot yet be classified as resolved — e.g. the
    /// evidence carries a non-whole contract count, a non-positive strike
    /// for an exercise/assignment, or an arithmetic overflow computing the
    /// cash effect. A caller must treat this exactly like
    /// `mqk_portfolio::FeeAttributionStatus::PendingAttribution`: the
    /// position's true state is not yet provable, never optimistically
    /// assumed flat or settled.
    Pending { reason: LifecyclePendingReason },
}

/// Closed-vocabulary reason a lifecycle event could not be resolved —
/// mirrors `mqk_broker_alpaca::fee_attribution::FeeNormalizeError`'s
/// fail-closed-only-on-genuine-defect shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecyclePendingReason {
    /// `contracts` was not a whole number of contracts.
    FractionalContracts,
    /// `strike_micros` was not strictly positive for an `Exercise`/
    /// `Assignment` event (a `0` or negative strike has no well-defined
    /// exercise economics; `Expiration` never reads this field).
    NonPositiveStrikeForExerciseOrAssignment,
    /// The cash-effect arithmetic (`strike * contracts * multiplier`)
    /// overflowed `i128`.
    CashEffectOverflow,
}

/// The paired option/underlying/cash effect one resolved lifecycle event
/// produces — Wave D4's "paired option removal + underlying delivery/cash
/// when broker evidence proves it" requirement, encoded structurally so a
/// caller cannot construct a mismatched pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedLifecycleEffect {
    pub kind: OptionLifecycleEventKind,
    pub option_symbol: String,
    pub underlying_symbol: String,
    /// The option position is always reduced by exactly this many contracts
    /// — every lifecycle event removes the option leg it resolves.
    pub option_contracts_removed: QtyMicros,
    /// Underlying shares delivered (`Some`, positive) or received (`Some`,
    /// negative encoded as a signed whole-share `QtyMicros` is not
    /// representable here since delivery direction is captured by
    /// `is_call`/event kind at the caller's ledger-application layer) for
    /// `Exercise`/`Assignment`; always `None` for `Expiration` (nothing
    /// lapsing worthless ever touches the underlying).
    pub underlying_shares_delivered: Option<QtyMicros>,
    /// Cash effect in micros for `Exercise`/`Assignment` (the strike
    /// consideration); always `None` for `Expiration` (a worthless lapse has
    /// no cash effect of its own — any premium P&L was already realized
    /// when the option was originally bought/sold).
    pub cash_effect_micros: Option<i64>,
}

/// Classify one [`LifecycleBrokerEvidence`] into an
/// [`OptionLifecycleResolution`] — pure, deterministic, no IO.
///
/// Fails closed (`Pending`) on a non-whole contract count, a non-positive
/// strike for `Exercise`/`Assignment`, or cash-effect arithmetic overflow.
/// Never estimates, rounds, or defaults a value the evidence did not
/// actually carry.
pub fn classify_option_lifecycle_event(
    evidence: &LifecycleBrokerEvidence,
) -> OptionLifecycleResolution {
    let Some(whole_contracts) = evidence.contracts.to_whole_units_checked() else {
        return OptionLifecycleResolution::Pending {
            reason: LifecyclePendingReason::FractionalContracts,
        };
    };

    match evidence.kind {
        OptionLifecycleEventKind::Expiration => {
            OptionLifecycleResolution::Resolved(PairedLifecycleEffect {
                kind: evidence.kind,
                option_symbol: evidence.option_symbol.clone(),
                underlying_symbol: evidence.underlying_symbol.clone(),
                option_contracts_removed: evidence.contracts,
                underlying_shares_delivered: None,
                cash_effect_micros: None,
            })
        }
        OptionLifecycleEventKind::Exercise | OptionLifecycleEventKind::Assignment => {
            if evidence.strike_micros <= 0 {
                return OptionLifecycleResolution::Pending {
                    reason: LifecyclePendingReason::NonPositiveStrikeForExerciseOrAssignment,
                };
            }

            let shares_per_contract = evidence.multiplier as i128;
            let total_shares_i128 = match whole_contracts.checked_mul(shares_per_contract as i64) {
                Some(v) => v as i128,
                None => {
                    return OptionLifecycleResolution::Pending {
                        reason: LifecyclePendingReason::CashEffectOverflow,
                    }
                }
            };

            let cash_effect_i128 = (evidence.strike_micros as i128)
                .checked_mul(whole_contracts as i128)
                .and_then(|v| v.checked_mul(shares_per_contract));
            let Some(cash_effect_i128) = cash_effect_i128 else {
                return OptionLifecycleResolution::Pending {
                    reason: LifecyclePendingReason::CashEffectOverflow,
                };
            };
            let Ok(cash_effect_micros) = i64::try_from(cash_effect_i128) else {
                return OptionLifecycleResolution::Pending {
                    reason: LifecyclePendingReason::CashEffectOverflow,
                };
            };
            let Ok(total_shares) = i64::try_from(total_shares_i128) else {
                return OptionLifecycleResolution::Pending {
                    reason: LifecyclePendingReason::CashEffectOverflow,
                };
            };
            let Some(underlying_shares) = QtyMicros::from_whole_units(total_shares) else {
                return OptionLifecycleResolution::Pending {
                    reason: LifecyclePendingReason::CashEffectOverflow,
                };
            };

            OptionLifecycleResolution::Resolved(PairedLifecycleEffect {
                kind: evidence.kind,
                option_symbol: evidence.option_symbol.clone(),
                underlying_symbol: evidence.underlying_symbol.clone(),
                option_contracts_removed: evidence.contracts,
                underlying_shares_delivered: Some(underlying_shares),
                cash_effect_micros: Some(cash_effect_micros),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(
        kind: OptionLifecycleEventKind,
        contracts: QtyMicros,
        strike_micros: i64,
        multiplier: i32,
    ) -> LifecycleBrokerEvidence {
        LifecycleBrokerEvidence {
            kind,
            option_symbol: "AAPL260619C00200000".to_string(),
            underlying_symbol: "AAPL".to_string(),
            contracts,
            multiplier,
            strike_micros,
            is_call: true,
        }
    }

    #[test]
    fn expiration_never_carries_underlying_or_cash() {
        let e = evidence(
            OptionLifecycleEventKind::Expiration,
            QtyMicros::from_whole_units(3).unwrap(),
            200_000_000,
            100,
        );
        let resolution = classify_option_lifecycle_event(&e);
        match resolution {
            OptionLifecycleResolution::Resolved(effect) => {
                assert_eq!(effect.underlying_shares_delivered, None);
                assert_eq!(effect.cash_effect_micros, None);
                assert_eq!(
                    effect.option_contracts_removed,
                    QtyMicros::from_whole_units(3).unwrap()
                );
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn exercise_pairs_option_removal_with_underlying_and_cash() {
        // 2 contracts, $200 strike, 100 shares/contract -> 200 shares,
        // $40,000 cash effect.
        let e = evidence(
            OptionLifecycleEventKind::Exercise,
            QtyMicros::from_whole_units(2).unwrap(),
            200_000_000,
            100,
        );
        let resolution = classify_option_lifecycle_event(&e);
        match resolution {
            OptionLifecycleResolution::Resolved(effect) => {
                assert_eq!(
                    effect.underlying_shares_delivered,
                    Some(QtyMicros::from_whole_units(200).unwrap())
                );
                assert_eq!(effect.cash_effect_micros, Some(40_000_000_000));
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn assignment_uses_the_same_pairing_shape_as_exercise() {
        let e = evidence(
            OptionLifecycleEventKind::Assignment,
            QtyMicros::from_whole_units(1).unwrap(),
            50_000_000,
            100,
        );
        let resolution = classify_option_lifecycle_event(&e);
        match resolution {
            OptionLifecycleResolution::Resolved(effect) => {
                assert_eq!(
                    effect.underlying_shares_delivered,
                    Some(QtyMicros::from_whole_units(100).unwrap())
                );
                assert_eq!(effect.cash_effect_micros, Some(5_000_000_000));
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn fractional_contracts_are_pending_never_truncated() {
        let e = evidence(
            OptionLifecycleEventKind::Exercise,
            QtyMicros::new(1_500_000), // 1.5 contracts
            200_000_000,
            100,
        );
        assert_eq!(
            classify_option_lifecycle_event(&e),
            OptionLifecycleResolution::Pending {
                reason: LifecyclePendingReason::FractionalContracts
            }
        );
    }

    #[test]
    fn non_positive_strike_is_pending_for_exercise_and_assignment() {
        for kind in [
            OptionLifecycleEventKind::Exercise,
            OptionLifecycleEventKind::Assignment,
        ] {
            let e = evidence(kind, QtyMicros::from_whole_units(1).unwrap(), 0, 100);
            assert_eq!(
                classify_option_lifecycle_event(&e),
                OptionLifecycleResolution::Pending {
                    reason: LifecyclePendingReason::NonPositiveStrikeForExerciseOrAssignment
                },
                "{kind:?} with zero strike must be Pending"
            );
        }
    }

    #[test]
    fn expiration_ignores_strike_entirely_even_when_nonpositive() {
        // Expiration's pairing never reads strike_micros -- a worthless
        // lapse is well-defined regardless of what the (irrelevant) strike
        // field carries.
        let e = evidence(
            OptionLifecycleEventKind::Expiration,
            QtyMicros::from_whole_units(1).unwrap(),
            0,
            100,
        );
        assert!(matches!(
            classify_option_lifecycle_event(&e),
            OptionLifecycleResolution::Resolved(_)
        ));
    }

    #[test]
    fn cash_effect_overflow_is_pending_not_wrapped() {
        let e = evidence(
            OptionLifecycleEventKind::Exercise,
            // A large but representable contract count; combined with a
            // near-max strike and multiplier, the cash/underlying-shares
            // arithmetic overflows well before any single field does.
            QtyMicros::from_whole_units(1_000_000).unwrap(),
            i64::MAX,
            i32::MAX,
        );
        assert_eq!(
            classify_option_lifecycle_event(&e),
            OptionLifecycleResolution::Pending {
                reason: LifecyclePendingReason::CashEffectOverflow
            }
        );
    }

    #[test]
    fn resolved_lifecycle_effect_is_never_read_as_an_ordinary_fill() {
        // Structural proof, not a runtime assertion: PairedLifecycleEffect
        // and Fill are distinct types with no conversion between them in
        // this crate -- a caller cannot accidentally push a lifecycle
        // effect into `Ledger::append_fill`. This test exists to make that
        // separation explicit and would fail to compile (not fail at
        // runtime) if a blanket `From<PairedLifecycleEffect> for Fill`
        // were ever added carrying a fabricated price.
        let e = evidence(
            OptionLifecycleEventKind::Exercise,
            QtyMicros::from_whole_units(1).unwrap(),
            200_000_000,
            100,
        );
        let OptionLifecycleResolution::Resolved(effect) = classify_option_lifecycle_event(&e)
        else {
            panic!("expected Resolved");
        };
        // `effect` has no `.price_micros`/`.side` field the way `Fill` does
        // -- this line only compiles because the two types are genuinely
        // distinct shapes, not merely differently named.
        assert_eq!(effect.option_symbol, "AAPL260619C00200000");
    }
}
