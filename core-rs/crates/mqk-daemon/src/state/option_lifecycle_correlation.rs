//! D1: deterministic correlation of an `OPEXC`/`OPASN` lifecycle event with
//! its `OPTRD` settlement trade. Pure: no IO, no clock.
//!
//! The provider's own documentation shows a lifecycle row and its `OPTRD` row
//! sharing one `id`, but its examples are not internally consistent and its
//! newer event surface exposes explicit `ref_id`/`group_id`. Same-`id` is
//! therefore never assumed universal authority. Precedence (first satisfied
//! wins; a stage that finds nothing falls through to the next):
//!
//! 1. explicit provider correlation: the lifecycle row and the `OPTRD` row
//!    carry the same non-empty `group_id`;
//! 2. same activity id, only when the `OPTRD` row also satisfies every
//!    economic fact below;
//! 3. deterministic evidence match: exactly ONE unconsumed `OPTRD` row
//!    satisfies every fact.
//!
//! Economic facts an `OPTRD` candidate must satisfy (all, exactly):
//! executed status; same provider date; `symbol` equals the OCC contract's
//! underlying; signed share quantity = contracts x 100 with the sign the
//! `(event, right)` direction requires; `price` equals the OCC strike;
//! `net_amount` equals the signed strike cash (`-shares x strike`). Zero
//! candidates is `PendingEvidence`; two or more is `PendingAmbiguous` --
//! never `LIMIT 1`, never first-row-wins, never symbol/date-only guessing.

use std::collections::BTreeSet;

use mqk_db::option_lifecycle_activity::{NewOptionLifecycleActivity, OptionLifecycleActivityType};
use mqk_db::CorrelationBasis;
use mqk_execution::{OptionContractIdentity, STANDARD_OPTION_MULTIPLIER};

use super::option_lifecycle_decimal::{parse_exact_micros, DecimalParseError};

const MICROS: i128 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorrelationOutcome {
    /// Evidence is complete and unambiguous; the event may be applied.
    Ready {
        underlying_symbol: String,
        basis: CorrelationBasis,
        optrd_activity_id: Option<String>,
    },
    /// No candidate (yet) satisfies the facts; nothing may be applied.
    PendingEvidence {
        underlying_symbol: Option<String>,
        reason: String,
    },
    /// More than one candidate satisfies the facts; nothing may be applied.
    PendingAmbiguous {
        underlying_symbol: String,
        candidates: usize,
        reason: String,
    },
}

/// Signed share direction an exercise/assignment must produce:
/// long call exercise +, long put exercise -, short call assignment -,
/// short put assignment +.
pub fn expected_share_sign_positive(
    kind: OptionLifecycleActivityType,
    is_call: bool,
) -> Option<bool> {
    match kind {
        OptionLifecycleActivityType::Exercise => Some(is_call),
        OptionLifecycleActivityType::Assignment => Some(!is_call),
        OptionLifecycleActivityType::Expiration | OptionLifecycleActivityType::PairedTrade => None,
    }
}

fn pending_evidence(underlying: Option<String>, reason: impl Into<String>) -> CorrelationOutcome {
    CorrelationOutcome::PendingEvidence {
        underlying_symbol: underlying,
        reason: reason.into(),
    }
}

fn decimal(field: &str, raw: &str) -> Result<i64, String> {
    parse_exact_micros(raw).map_err(|e| match e {
        DecimalParseError::Malformed => format!("{field} {raw:?} is not a decimal"),
        DecimalParseError::NotExactOrOverflow => {
            format!("{field} {raw:?} is not exactly representable")
        }
    })
}

fn is_executed(row: &NewOptionLifecycleActivity) -> bool {
    row.provenance.status.as_deref() == Some("executed")
}

/// Validate the lifecycle row itself and return `(contract, contracts_micros)`.
fn lifecycle_facts(
    lifecycle: &NewOptionLifecycleActivity,
) -> Result<(OptionContractIdentity, i64), (Option<String>, String)> {
    let symbol = lifecycle
        .option_symbol
        .as_deref()
        .ok_or((None, "lifecycle row has no option symbol".to_string()))?;
    let contract = OptionContractIdentity::parse(symbol)
        .map_err(|e| (None, format!("lifecycle option symbol: {e}")))?;
    let underlying = Some(contract.underlying().to_string());
    if !is_executed(lifecycle) {
        return Err((underlying, "lifecycle status is not executed".to_string()));
    }
    let qty = decimal("lifecycle qty", &lifecycle.qty_raw).map_err(|e| (underlying.clone(), e))?;
    if qty == 0 {
        return Err((underlying, "lifecycle qty is zero".to_string()));
    }
    let net = decimal("lifecycle net_amount", &lifecycle.net_amount_raw)
        .map_err(|e| (underlying.clone(), e))?;
    if net != 0 {
        return Err((
            underlying,
            "lifecycle net_amount is not 0: a lifecycle row must not carry cash".to_string(),
        ));
    }
    if qty % 1_000_000 != 0 {
        return Err((
            underlying,
            "lifecycle qty is not whole contracts".to_string(),
        ));
    }
    match lifecycle.activity_type {
        OptionLifecycleActivityType::Exercise if qty > 0 => Err((
            underlying,
            "exercise must remove long contracts (negative qty)".to_string(),
        )),
        OptionLifecycleActivityType::Assignment if qty < 0 => Err((
            underlying,
            "assignment must remove short contracts (positive qty)".to_string(),
        )),
        _ => Ok((contract, qty)),
    }
}

/// The provider-signed settlement evidence of a verified `OPTRD` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementEvidence {
    /// Signed share delta (+ received, - delivered).
    pub underlying_qty_delta_micros: i64,
    /// Provider-signed `net_amount`, exactly as reported.
    pub cash_delta_micros: i64,
}

/// A lifecycle event whose every fact has been verified, with the signed
/// economics apply consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSettlement {
    pub contract: OptionContractIdentity,
    /// The lifecycle row's own signed contract delta.
    pub option_qty_delta_micros: i64,
    /// `Some` for exercise/assignment, `None` for expiration.
    pub settlement: Option<SettlementEvidence>,
}

/// The single verification of a lifecycle event against its settlement trade,
/// used by correlation (to filter candidates) and by apply (to re-verify
/// before any economic mutation). An expiration takes no trade (`optrd` must
/// be `None`); an exercise/assignment requires one that satisfies every fact.
pub fn verify_lifecycle_settlement(
    lifecycle: &NewOptionLifecycleActivity,
    optrd: Option<&NewOptionLifecycleActivity>,
) -> Result<VerifiedSettlement, String> {
    let (contract, contracts_micros) = lifecycle_facts(lifecycle).map_err(|(_, reason)| reason)?;
    match (lifecycle.activity_type, optrd) {
        (OptionLifecycleActivityType::Expiration, None) => Ok(VerifiedSettlement {
            contract,
            option_qty_delta_micros: contracts_micros,
            settlement: None,
        }),
        (OptionLifecycleActivityType::Expiration, Some(_)) => {
            Err("an expiration must not carry a settlement trade".to_string())
        }
        (_, None) => Err("exercise/assignment requires its settlement trade".to_string()),
        (_, Some(o)) => {
            let settlement = optrd_satisfies_facts(lifecycle, &contract, contracts_micros, o)?;
            Ok(VerifiedSettlement {
                contract,
                option_qty_delta_micros: contracts_micros,
                settlement: Some(settlement),
            })
        }
    }
}

/// Every economic fact an `OPTRD` row must satisfy against the lifecycle
/// event. `Err` carries the first violated fact (diagnostic only).
pub fn optrd_satisfies_facts(
    lifecycle: &NewOptionLifecycleActivity,
    contract: &OptionContractIdentity,
    contracts_signed_micros: i64,
    optrd: &NewOptionLifecycleActivity,
) -> Result<SettlementEvidence, String> {
    if optrd.activity_type != OptionLifecycleActivityType::PairedTrade {
        return Err("not an OPTRD row".to_string());
    }
    if !is_executed(optrd) {
        return Err("OPTRD status is not executed".to_string());
    }
    if optrd.activity_date != lifecycle.activity_date {
        return Err("OPTRD date differs from the lifecycle date".to_string());
    }
    if optrd.underlying_symbol_raw.as_deref() != Some(contract.underlying()) {
        return Err("OPTRD underlying differs from the contract underlying".to_string());
    }
    let expect_positive = expected_share_sign_positive(lifecycle.activity_type, contract.is_call())
        .ok_or_else(|| "lifecycle kind has no settlement trade".to_string())?;

    let shares = decimal("OPTRD qty", &optrd.qty_raw)?;
    let contracts_abs = i128::from(contracts_signed_micros).abs();
    let expected_shares_abs = contracts_abs
        .checked_mul(i128::from(STANDARD_OPTION_MULTIPLIER))
        .ok_or_else(|| "expected share quantity overflows".to_string())?;
    if i128::from(shares).abs() != expected_shares_abs || shares == 0 {
        return Err("OPTRD qty is not contracts x 100".to_string());
    }
    if (shares > 0) != expect_positive {
        return Err("OPTRD share direction contradicts the event/right".to_string());
    }

    let price_raw = optrd
        .price_raw
        .as_deref()
        .ok_or_else(|| "OPTRD has no price".to_string())?;
    let price = decimal("OPTRD price", price_raw)?;
    if price != contract.strike_micros() {
        return Err("OPTRD price differs from the OCC strike".to_string());
    }

    let net = decimal("OPTRD net_amount", &optrd.net_amount_raw)?;
    // cash = -(signed shares) x strike; shares and strike are micros scale.
    let cash_abs = i128::from(shares)
        .abs()
        .checked_mul(i128::from(price))
        .ok_or_else(|| "expected cash overflows".to_string())?
        / MICROS;
    let expected_net = if shares > 0 { -cash_abs } else { cash_abs };
    if i128::from(net) != expected_net {
        return Err("OPTRD net_amount is not the signed strike cash".to_string());
    }
    Ok(SettlementEvidence {
        underlying_qty_delta_micros: shares,
        cash_delta_micros: net,
    })
}

/// Correlate one lifecycle event.
///
/// `optrds` is every ingested `OPTRD` row of the account; `excluded_optrd_ids`
/// holds `OPTRD` activity ids that must not be (re)used by this event -- ids
/// already consumed by an applied adjustment, and ids that are the same-id
/// partner of a DIFFERENT lifecycle event.
pub fn correlate_lifecycle_event(
    lifecycle: &NewOptionLifecycleActivity,
    optrds: &[NewOptionLifecycleActivity],
    excluded_optrd_ids: &BTreeSet<String>,
) -> CorrelationOutcome {
    let (contract, contracts_micros) = match lifecycle_facts(lifecycle) {
        Ok(v) => v,
        Err((underlying, reason)) => return pending_evidence(underlying, reason),
    };
    let underlying = contract.underlying().to_string();

    if lifecycle.activity_type == OptionLifecycleActivityType::Expiration {
        return CorrelationOutcome::Ready {
            underlying_symbol: underlying,
            basis: CorrelationBasis::NotRequired,
            optrd_activity_id: None,
        };
    }

    let usable: Vec<&NewOptionLifecycleActivity> = optrds
        .iter()
        .filter(|o| !excluded_optrd_ids.contains(&o.activity_id))
        .collect();
    let facts_ok = |o: &&NewOptionLifecycleActivity| {
        optrd_satisfies_facts(lifecycle, &contract, contracts_micros, o).is_ok()
    };

    // 1. explicit provider group identity.
    if let Some(group) = lifecycle.provenance.group_id.as_deref() {
        let explicit: Vec<&NewOptionLifecycleActivity> = usable
            .iter()
            .copied()
            .filter(|o| o.provenance.group_id.as_deref() == Some(group))
            .filter(|o| facts_ok(o))
            .collect();
        match explicit.len() {
            0 => {}
            1 => {
                return CorrelationOutcome::Ready {
                    underlying_symbol: underlying,
                    basis: CorrelationBasis::ExplicitGroupId,
                    optrd_activity_id: Some(explicit[0].activity_id.clone()),
                }
            }
            n => {
                return CorrelationOutcome::PendingAmbiguous {
                    underlying_symbol: underlying,
                    candidates: n,
                    reason: "multiple OPTRD rows share the explicit group id and satisfy the \
                             facts"
                        .to_string(),
                }
            }
        }
    }

    // 2. same activity id, only when the row also satisfies every fact.
    if let Some(same) = usable
        .iter()
        .copied()
        .find(|o| o.activity_id == lifecycle.activity_id)
        .filter(|o| facts_ok(o))
    {
        return CorrelationOutcome::Ready {
            underlying_symbol: underlying,
            basis: CorrelationBasis::SameActivityId,
            optrd_activity_id: Some(same.activity_id.clone()),
        };
    }

    // 3. deterministic evidence: exactly one unconsumed candidate.
    let candidates: Vec<&NewOptionLifecycleActivity> =
        usable.iter().copied().filter(|o| facts_ok(o)).collect();
    match candidates.len() {
        0 => pending_evidence(
            Some(underlying),
            "no OPTRD row satisfies the contract's economic facts",
        ),
        1 => CorrelationOutcome::Ready {
            underlying_symbol: underlying,
            basis: CorrelationBasis::UniqueEvidence,
            optrd_activity_id: Some(candidates[0].activity_id.clone()),
        },
        n => CorrelationOutcome::PendingAmbiguous {
            underlying_symbol: underlying,
            candidates: n,
            reason: format!("{n} OPTRD rows satisfy the contract's economic facts"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_db::option_lifecycle_activity::OptionLifecycleRawProvenance;

    const CALL: &str = "AAPL230721C00150000";
    const PUT: &str = "AAPL230721P00150000";
    const DATE: &str = "2023-07-21";

    fn prov(group: Option<&str>) -> OptionLifecycleRawProvenance {
        OptionLifecycleRawProvenance {
            group_id: group.map(str::to_string),
            ref_id: None,
            status: Some("executed".to_string()),
            description: None,
            raw_json: None,
        }
    }

    fn lifecycle(
        id: &str,
        ty: OptionLifecycleActivityType,
        symbol: &str,
        qty: &str,
        group: Option<&str>,
    ) -> NewOptionLifecycleActivity {
        NewOptionLifecycleActivity {
            activity_id: id.to_string(),
            broker_account_id: "alpaca:acct".to_string(),
            engine_id: "e".to_string(),
            mode: "paper".to_string(),
            activity_type: ty,
            option_symbol: Some(symbol.to_string()),
            underlying_symbol_raw: None,
            activity_date: DATE.to_string(),
            qty_raw: qty.to_string(),
            price_raw: None,
            net_amount_raw: "0".to_string(),
            ingested_at_utc: chrono::Utc::now(),
            provenance: prov(group),
            state_seed: None,
        }
    }

    fn optrd(
        id: &str,
        underlying: &str,
        qty: &str,
        price: &str,
        net: &str,
        group: Option<&str>,
    ) -> NewOptionLifecycleActivity {
        NewOptionLifecycleActivity {
            activity_id: id.to_string(),
            broker_account_id: "alpaca:acct".to_string(),
            engine_id: "e".to_string(),
            mode: "paper".to_string(),
            activity_type: OptionLifecycleActivityType::PairedTrade,
            option_symbol: None,
            underlying_symbol_raw: Some(underlying.to_string()),
            activity_date: DATE.to_string(),
            qty_raw: qty.to_string(),
            price_raw: Some(price.to_string()),
            net_amount_raw: net.to_string(),
            ingested_at_utc: chrono::Utc::now(),
            provenance: prov(group),
            state_seed: None,
        }
    }

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    // The four economic directions (contracts x 100 shares at strike 150).
    // (kind, symbol, lifecycle qty, OPTRD qty, OPTRD net)
    fn directions() -> [(
        OptionLifecycleActivityType,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ); 4] {
        use OptionLifecycleActivityType::*;
        [
            (Exercise, CALL, "-2", "200", "-30000"), // long call: receive shares, pay
            (Exercise, PUT, "-2", "-200", "30000"),  // long put: deliver shares, receive
            (Assignment, CALL, "2", "-200", "30000"), // short call: deliver, receive
            (Assignment, PUT, "2", "200", "-30000"), // short put: receive shares, pay
        ]
    }

    #[test]
    fn all_four_directions_correlate_by_same_id_when_facts_hold() {
        for (ty, sym, lq, tq, net) in directions() {
            let l = lifecycle("X1", ty, sym, lq, None);
            let t = optrd("X1", "AAPL", tq, "150", net, None);
            assert_eq!(
                correlate_lifecycle_event(&l, &[t], &none()),
                CorrelationOutcome::Ready {
                    underlying_symbol: "AAPL".to_string(),
                    basis: CorrelationBasis::SameActivityId,
                    optrd_activity_id: Some("X1".to_string()),
                },
                "{ty:?} {sym}"
            );
        }
    }

    #[test]
    fn expiration_needs_no_paired_trade() {
        let l = lifecycle(
            "E1",
            OptionLifecycleActivityType::Expiration,
            CALL,
            "-1",
            None,
        );
        assert_eq!(
            correlate_lifecycle_event(&l, &[], &none()),
            CorrelationOutcome::Ready {
                underlying_symbol: "AAPL".to_string(),
                basis: CorrelationBasis::NotRequired,
                optrd_activity_id: None,
            }
        );
    }

    #[test]
    fn different_ids_correlate_through_an_explicit_group_id() {
        let l = lifecycle(
            "L1",
            OptionLifecycleActivityType::Exercise,
            CALL,
            "-2",
            Some("G1"),
        );
        let t = optrd("T9", "AAPL", "200", "150", "-30000", Some("G1"));
        assert_eq!(
            correlate_lifecycle_event(&l, &[t], &none()),
            CorrelationOutcome::Ready {
                underlying_symbol: "AAPL".to_string(),
                basis: CorrelationBasis::ExplicitGroupId,
                optrd_activity_id: Some("T9".to_string()),
            }
        );
    }

    #[test]
    fn a_unique_evidence_match_correlates_when_ids_and_groups_are_absent() {
        let l = lifecycle(
            "L1",
            OptionLifecycleActivityType::Exercise,
            CALL,
            "-2",
            None,
        );
        let t = optrd("T9", "AAPL", "200", "150", "-30000", None);
        assert_eq!(
            correlate_lifecycle_event(&l, &[t], &none()),
            CorrelationOutcome::Ready {
                underlying_symbol: "AAPL".to_string(),
                basis: CorrelationBasis::UniqueEvidence,
                optrd_activity_id: Some("T9".to_string()),
            }
        );
    }

    #[test]
    fn two_indistinguishable_candidates_are_ambiguous_never_first_wins() {
        let l = lifecycle(
            "L1",
            OptionLifecycleActivityType::Exercise,
            CALL,
            "-2",
            None,
        );
        let a = optrd("T1", "AAPL", "200", "150", "-30000", None);
        let b = optrd("T2", "AAPL", "200", "150", "-30000", None);
        assert!(matches!(
            correlate_lifecycle_event(&l, &[a, b], &none()),
            CorrelationOutcome::PendingAmbiguous { candidates: 2, .. }
        ));
    }

    #[test]
    fn no_matching_trade_is_pending_evidence() {
        let l = lifecycle(
            "L1",
            OptionLifecycleActivityType::Exercise,
            CALL,
            "-2",
            None,
        );
        assert!(matches!(
            correlate_lifecycle_event(&l, &[], &none()),
            CorrelationOutcome::PendingEvidence { .. }
        ));
    }

    #[test]
    fn a_same_id_trade_that_violates_any_fact_is_never_trusted() {
        let l = lifecycle(
            "X1",
            OptionLifecycleActivityType::Exercise,
            CALL,
            "-2",
            None,
        );
        for (label, t) in [
            (
                "wrong underlying",
                optrd("X1", "MSFT", "200", "150", "-30000", None),
            ),
            (
                "wrong share count",
                optrd("X1", "AAPL", "100", "150", "-15000", None),
            ),
            (
                "wrong direction",
                optrd("X1", "AAPL", "-200", "150", "30000", None),
            ),
            (
                "wrong strike price",
                optrd("X1", "AAPL", "200", "140", "-28000", None),
            ),
            (
                "wrong cash",
                optrd("X1", "AAPL", "200", "150", "-29999", None),
            ),
            (
                "cash sign flipped",
                optrd("X1", "AAPL", "200", "150", "30000", None),
            ),
        ] {
            assert!(
                matches!(
                    correlate_lifecycle_event(&l, &[t], &none()),
                    CorrelationOutcome::PendingEvidence { .. }
                ),
                "{label} must stay pending"
            );
        }
        let mut wrong_date = optrd("X1", "AAPL", "200", "150", "-30000", None);
        wrong_date.activity_date = "2023-07-22".to_string();
        assert!(matches!(
            correlate_lifecycle_event(&l, &[wrong_date], &none()),
            CorrelationOutcome::PendingEvidence { .. }
        ));
        let mut not_executed = optrd("X1", "AAPL", "200", "150", "-30000", None);
        not_executed.provenance.status = Some("pending".to_string());
        assert!(matches!(
            correlate_lifecycle_event(&l, &[not_executed], &none()),
            CorrelationOutcome::PendingEvidence { .. }
        ));
    }

    #[test]
    fn excluded_trades_are_never_reused() {
        let l = lifecycle(
            "L1",
            OptionLifecycleActivityType::Exercise,
            CALL,
            "-2",
            None,
        );
        let t = optrd("T1", "AAPL", "200", "150", "-30000", None);
        let mut excluded = BTreeSet::new();
        excluded.insert("T1".to_string());
        assert!(matches!(
            correlate_lifecycle_event(&l, &[t], &excluded),
            CorrelationOutcome::PendingEvidence { .. }
        ));
    }

    #[test]
    fn lifecycle_row_defects_are_pending_never_applied() {
        use OptionLifecycleActivityType::*;
        let t = optrd("X1", "AAPL", "200", "150", "-30000", None);
        // Exercise removing a SHORT (positive qty) is not a valid exercise.
        assert!(matches!(
            correlate_lifecycle_event(
                &lifecycle("X1", Exercise, CALL, "2", None),
                &[t.clone()],
                &none()
            ),
            CorrelationOutcome::PendingEvidence { .. }
        ));
        // Fractional contracts.
        assert!(matches!(
            correlate_lifecycle_event(
                &lifecycle("X1", Exercise, CALL, "-1.5", None),
                &[t.clone()],
                &none()
            ),
            CorrelationOutcome::PendingEvidence { .. }
        ));
        // Not executed.
        let mut l = lifecycle("X1", Exercise, CALL, "-2", None);
        l.provenance.status = Some("canceled".to_string());
        assert!(matches!(
            correlate_lifecycle_event(&l, &[t.clone()], &none()),
            CorrelationOutcome::PendingEvidence { .. }
        ));
        // Lifecycle row carrying cash.
        let mut l = lifecycle("X1", Exercise, CALL, "-2", None);
        l.net_amount_raw = "5".to_string();
        assert!(matches!(
            correlate_lifecycle_event(&l, &[t.clone()], &none()),
            CorrelationOutcome::PendingEvidence { .. }
        ));
        // Overflowing quantity is refused, not wrapped.
        assert!(matches!(
            correlate_lifecycle_event(
                &lifecycle("X1", Exercise, CALL, "-99999999999999999999", None),
                &[t],
                &none()
            ),
            CorrelationOutcome::PendingEvidence { .. }
        ));
    }

    #[test]
    fn the_option_contract_symbol_is_never_confused_with_the_underlying() {
        // An OPTRD whose underlying field holds the OCC contract symbol is
        // not a settlement trade for that underlying.
        let l = lifecycle(
            "X1",
            OptionLifecycleActivityType::Exercise,
            CALL,
            "-2",
            None,
        );
        let t = optrd("X1", CALL, "200", "150", "-30000", None);
        assert!(matches!(
            correlate_lifecycle_event(&l, &[t], &none()),
            CorrelationOutcome::PendingEvidence { .. }
        ));
    }
}
