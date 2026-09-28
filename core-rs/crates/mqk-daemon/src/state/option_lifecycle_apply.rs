//! D2 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): the
//! distinct, idempotent options-lifecycle accounting transaction D1's raw
//! evidence ledger requires before it means anything economically.
//!
//! Composes three independently-proven pieces (mirrors B6's
//! `state/crypto_fee_ingestion.rs` composition pattern exactly):
//! - `mqk_db::option_lifecycle_activity` (D1): durable raw evidence +
//!   OPEXC/OPASN <-> OPTRD pairing lookup (by the shared `activity_id` --
//!   the real provider-documented correlation evidence) + idempotent-apply
//!   marker.
//! - `mqk_portfolio::option_lifecycle::classify_option_lifecycle_event`
//!   (the existing Wave D4 model): pure classification into a
//!   `PairedLifecycleEffect` or a fail-closed `Pending`. Used only for its
//!   fail-closed contract checks (fractional contracts, non-positive
//!   strike) and `option_contracts_removed` -- see the D2 correction note
//!   below for why its `underlying_shares_delivered`/`cash_effect_micros`
//!   are never trusted directly.
//! - The caller's own already-known canonical option contract terms
//!   (strike/multiplier/right/underlying) -- this module deliberately does
//!   NOT parse Alpaca's raw OCC-style `option_symbol` string itself
//!   (a broker-wire-format concern this repo has no existing parser for);
//!   the caller is expected to resolve these from its own instrument/
//!   position registry, exactly as `LifecycleBrokerEvidence`'s own field
//!   contract already requires.
//!
//! For `Exercise`/`Assignment`, genuine resolution additionally requires
//! the paired `OPTRD` settlement activity to have actually been observed
//! (never merely inferred from the OPEXC/OPASN activity and the caller's
//! own contract knowledge alone) -- and, as a defense-in-depth cross-check,
//! that OPTRD's own reported price must agree with the caller-supplied
//! strike. A disagreement is treated exactly like any other unresolved
//! shape: `Pending`, never silently trusted either side.
//!
//! # D2 correction: signed economics from real provider evidence
//!
//! `classify_option_lifecycle_event`'s `PairedLifecycleEffect` never reads
//! `is_call` and reports `underlying_shares_delivered`/`cash_effect_micros`
//! as unsigned magnitudes by design (its own doc: "delivery direction is
//! captured by `is_call`/event kind at the caller's ledger-application
//! layer") -- the prior version of this module never did that, applying
//! the unsigned magnitude directly for all four call/put exercise/
//! assignment directions. Alpaca's own paired `OPTRD` activity already
//! carries the correct SIGNED `qty` (shares) and `net_amount` (cash) for
//! its exact economic direction -- real, broker-confirmed evidence, not a
//! derivation this module would otherwise have to get right on its own.
//! This module now uses OPTRD's own signed `qty_raw`/`net_amount_raw`
//! directly as `underlying_shares_delivered`/`cash_effect_micros`
//! (bullet: "preserve provider net_amount ... do not manufacture cash"),
//! and uses `is_call` + `kind` only to compute the expected share-delta
//! sign and cross-check it against OPTRD's actual reported sign -- a
//! disagreement is `Pending` (`PairedTradeDirectionMismatch`), exactly
//! like the existing strike cross-check, never silently trusted either
//! side. This is `is_call` genuinely participating in resulting economics
//! (as a proof gate), while the applied values are the broker's own.
//!
//! No production caller wired in this patch -- Alpaca options trading
//! capability does not exist in this codebase yet.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_db::option_lifecycle_activity::{
    fetch_applied_option_lifecycle_effect, fetch_option_lifecycle_activity,
    find_paired_trade_activity, insert_applied_option_lifecycle_effect_if_new,
    AppliedOptionLifecycleEffect, InsertAppliedOptionLifecycleEffectOutcome,
    OptionLifecycleActivityType,
};
use mqk_portfolio::option_lifecycle::{
    classify_option_lifecycle_event, LifecycleBrokerEvidence, LifecyclePendingReason,
    OptionLifecycleEventKind, OptionLifecycleResolution,
};
use mqk_schemas::QtyMicros;

/// Canonical option contract terms, already known to the caller from its
/// own instrument/position registry -- never derived by parsing a raw
/// broker symbol string in this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionContractTerms {
    pub strike_micros: i64,
    pub multiplier: i32,
    pub is_call: bool,
    pub underlying_symbol: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingApplyReason {
    /// The underlying classification (`mqk_portfolio::option_lifecycle`)
    /// itself could not resolve the evidence.
    Lifecycle(LifecyclePendingReason),
    /// `Exercise`/`Assignment` has no paired `OPTRD` settlement evidence
    /// ingested yet.
    PairedTradeEvidenceMissing,
    /// A paired `OPTRD` exists, but its reported price disagrees with the
    /// caller-supplied strike -- a genuine anomaly, never silently trusted.
    PairedTradeStrikeMismatch {
        strike_micros: i64,
        optrd_price_raw: String,
    },
    /// A paired `OPTRD` exists and its price matches, but its reported
    /// share-delta sign disagrees with what `is_call`/`kind` predict for
    /// this economic direction -- a genuine anomaly (e.g. a mis-configured
    /// `is_call`), never silently trusted.
    PairedTradeDirectionMismatch {
        expected_positive_share_delta: bool,
        optrd_qty_raw: String,
    },
    /// A quantity/price/net_amount field in the ledger did not parse as a
    /// decimal number.
    MalformedDecimal { field: &'static str, raw: String },
    /// A quantity/price/net_amount field parsed but its micros-scale value
    /// overflowed `i64` arithmetic -- fails closed rather than wrapping.
    DecimalOverflow { field: &'static str, raw: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOptionLifecycleOutcome {
    /// Genuinely newly applied this call.
    Applied(AppliedOptionLifecycleEffect),
    /// Already durably applied by an earlier call -- zero new mutation.
    AlreadyApplied(AppliedOptionLifecycleEffect),
    /// Not yet resolvable; no mutation. Callers must never treat this as a
    /// flat/settled position.
    Pending { reason: PendingApplyReason },
}

/// Exact-decimal-string-to-micros parse (1e-6 scale), no floating point.
/// Accepts a leading `-`. Mirrors the same pattern used by
/// `mqk-broker-ibkr::events::parse_exact_decimal_to_qty_micros`.
///
/// D2 correction: every arithmetic step is checked -- a value whose
/// micros-scale representation would overflow `i64` fails closed
/// (`DecimalOverflow`) rather than silently wrapping.
fn parse_decimal_to_micros(field: &'static str, raw: &str) -> Result<i64, PendingApplyReason> {
    let malformed = || PendingApplyReason::MalformedDecimal {
        field,
        raw: raw.to_string(),
    };
    let overflow = || PendingApplyReason::DecimalOverflow {
        field,
        raw: raw.to_string(),
    };

    let raw_trim = raw.trim();
    let (sign, unsigned) = match raw_trim.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, raw_trim),
    };
    let parts: Vec<&str> = unsigned.splitn(2, '.').collect();
    let whole: i64 = parts[0].parse().map_err(|_| malformed())?;
    let frac_micros: i64 = match parts.get(1) {
        None => 0,
        Some(frac) => {
            if frac.is_empty() || frac.len() > 6 || !frac.chars().all(|c| c.is_ascii_digit()) {
                return Err(malformed());
            }
            format!("{frac:0<6}").parse().map_err(|_| malformed())?
        }
    };
    let whole_micros = whole.checked_mul(1_000_000).ok_or_else(overflow)?;
    let magnitude = whole_micros.checked_add(frac_micros).ok_or_else(overflow)?;
    magnitude.checked_mul(sign).ok_or_else(overflow)
}

/// D2 correction: whether the account is expected to RECEIVE underlying
/// shares (a positive signed share delta) for this `(kind, is_call)`
/// combination -- the canonical four-direction sign convention:
/// - long call exercise (Exercise, call): holder buys -> receives shares (+)
/// - long put exercise (Exercise, put): holder sells -> delivers shares (-)
/// - short call assignment (Assignment, call): writer delivers/sells (-)
/// - short put assignment (Assignment, put): writer buys/receives (+)
///
/// Only ever called for `Exercise`/`Assignment` (the two kinds that carry a
/// paired trade at all).
fn expects_positive_share_delta(kind: OptionLifecycleEventKind, is_call: bool) -> bool {
    match kind {
        OptionLifecycleEventKind::Exercise => is_call,
        OptionLifecycleEventKind::Assignment => !is_call,
        OptionLifecycleEventKind::Expiration => {
            unreachable!("expects_positive_share_delta is never called for Expiration")
        }
    }
}

/// Run one apply attempt for `lifecycle_activity_id` (an `OPEXC`/`OPASN`/
/// `OPEXP` activity already durably ingested by D1). Idempotent: a second
/// call for the same `lifecycle_activity_id` after a successful apply
/// returns `AlreadyApplied` with zero new mutation.
///
/// D2 correction: `raw`'s own `(engine_id, mode)` (fetched under an
/// already-account-scoped query) must match the caller-supplied scope --
/// a caller passing a mismatched `engine_id`/`mode` for evidence that
/// genuinely belongs to a different engine/mode is a caller programming
/// error, refused with `Err` (never silently reattributed, never treated
/// as a resolvable `Pending`).
#[allow(clippy::too_many_arguments)]
pub async fn apply_option_lifecycle_activity(
    pool: &PgPool,
    broker_account_id: &str,
    engine_id: &str,
    mode: &str,
    lifecycle_activity_id: &str,
    lifecycle_activity_type: OptionLifecycleActivityType,
    terms: &OptionContractTerms,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<ApplyOptionLifecycleOutcome> {
    if let Some(existing) =
        fetch_applied_option_lifecycle_effect(pool, broker_account_id, lifecycle_activity_id)
            .await?
    {
        return Ok(ApplyOptionLifecycleOutcome::AlreadyApplied(existing));
    }

    let Some(raw) = fetch_option_lifecycle_activity(
        pool,
        broker_account_id,
        lifecycle_activity_id,
        lifecycle_activity_type,
    )
    .await?
    else {
        anyhow::bail!(
            "apply_option_lifecycle_activity: activity {lifecycle_activity_id} not found in \
             sys_option_lifecycle_activity_ledger -- D1 ingestion must run first"
        );
    };

    if raw.engine_id != engine_id || raw.mode != mode {
        anyhow::bail!(
            "apply_option_lifecycle_activity: refused -- activity {lifecycle_activity_id} \
             carries scope (engine_id={:?}, mode={:?}) which does not match the requested \
             scope (engine_id={engine_id:?}, mode={mode:?}); zero rows mutated",
            raw.engine_id,
            raw.mode,
        );
    }

    let kind = match raw.activity_type {
        OptionLifecycleActivityType::Exercise => OptionLifecycleEventKind::Exercise,
        OptionLifecycleActivityType::Assignment => OptionLifecycleEventKind::Assignment,
        OptionLifecycleActivityType::Expiration => OptionLifecycleEventKind::Expiration,
        OptionLifecycleActivityType::PairedTrade => {
            anyhow::bail!(
                "apply_option_lifecycle_activity: {lifecycle_activity_id} is an OPTRD paired- \
                 trade activity, not an OPEXC/OPASN/OPEXP lifecycle activity"
            )
        }
    };

    let option_symbol = raw.option_symbol.clone().expect(
        "OPEXC/OPASN/OPEXP rows always carry option_symbol -- enforced by the DB CHECK constraint",
    );

    // D2 correction: for Exercise/Assignment, the paired OPTRD's own signed
    // qty/net_amount are the authoritative evidence -- captured here and
    // substituted into the classified effect below, never derived from
    // strike*contracts*multiplier alone.
    let mut optrd_shares_delivered_micros: Option<i64> = None;
    let mut optrd_cash_effect_micros: Option<i64> = None;

    if matches!(
        kind,
        OptionLifecycleEventKind::Exercise | OptionLifecycleEventKind::Assignment
    ) {
        let Some(optrd) =
            find_paired_trade_activity(pool, broker_account_id, lifecycle_activity_id).await?
        else {
            return Ok(ApplyOptionLifecycleOutcome::Pending {
                reason: PendingApplyReason::PairedTradeEvidenceMissing,
            });
        };
        let optrd_price_raw = optrd
            .price_raw
            .clone()
            .expect("OPTRD rows always carry a price -- enforced by the DB CHECK constraint");
        let optrd_price_micros = match parse_decimal_to_micros("optrd_price", &optrd_price_raw) {
            Ok(v) => v,
            Err(reason) => return Ok(ApplyOptionLifecycleOutcome::Pending { reason }),
        };
        if optrd_price_micros != terms.strike_micros {
            return Ok(ApplyOptionLifecycleOutcome::Pending {
                reason: PendingApplyReason::PairedTradeStrikeMismatch {
                    strike_micros: terms.strike_micros,
                    optrd_price_raw,
                },
            });
        }

        let optrd_qty_micros = match parse_decimal_to_micros("optrd_qty", &optrd.qty_raw) {
            Ok(v) => v,
            Err(reason) => return Ok(ApplyOptionLifecycleOutcome::Pending { reason }),
        };
        let optrd_net_amount_micros =
            match parse_decimal_to_micros("optrd_net_amount", &optrd.net_amount_raw) {
                Ok(v) => v,
                Err(reason) => return Ok(ApplyOptionLifecycleOutcome::Pending { reason }),
            };

        // Defense-in-depth: is_call/kind predict the expected share-delta
        // sign; OPTRD's own reported sign must agree. A genuine zero
        // (structurally impossible for a real settlement) is treated as a
        // mismatch rather than silently picked as either direction.
        let expected_positive = expects_positive_share_delta(kind, terms.is_call);
        let actual_positive = optrd_qty_micros > 0;
        if optrd_qty_micros == 0 || expected_positive != actual_positive {
            return Ok(ApplyOptionLifecycleOutcome::Pending {
                reason: PendingApplyReason::PairedTradeDirectionMismatch {
                    expected_positive_share_delta: expected_positive,
                    optrd_qty_raw: optrd.qty_raw.clone(),
                },
            });
        }

        optrd_shares_delivered_micros = Some(optrd_qty_micros);
        optrd_cash_effect_micros = Some(optrd_net_amount_micros);
    }

    let contracts_micros = match parse_decimal_to_micros("qty_raw", &raw.qty_raw) {
        Ok(v) => match v.checked_abs() {
            Some(m) => m,
            None => {
                return Ok(ApplyOptionLifecycleOutcome::Pending {
                    reason: PendingApplyReason::DecimalOverflow {
                        field: "qty_raw",
                        raw: raw.qty_raw.clone(),
                    },
                })
            }
        },
        Err(reason) => return Ok(ApplyOptionLifecycleOutcome::Pending { reason }),
    };

    let evidence = LifecycleBrokerEvidence {
        kind,
        option_symbol: option_symbol.clone(),
        underlying_symbol: terms.underlying_symbol.clone(),
        contracts: QtyMicros::new(contracts_micros),
        multiplier: terms.multiplier,
        strike_micros: terms.strike_micros,
        is_call: terms.is_call,
    };

    match classify_option_lifecycle_event(&evidence) {
        OptionLifecycleResolution::Pending { reason } => Ok(ApplyOptionLifecycleOutcome::Pending {
            reason: PendingApplyReason::Lifecycle(reason),
        }),
        OptionLifecycleResolution::Resolved(effect) => {
            // D2 correction: for Exercise/Assignment, OPTRD's own signed
            // evidence (captured above, already direction-cross-checked)
            // replaces the classify() model's unsigned magnitudes --
            // Expiration has no paired trade and keeps classify()'s
            // None/None exactly as before.
            let underlying_shares_delivered_raw =
                optrd_shares_delivered_micros.map(|m| QtyMicros::new(m).to_string());
            let cash_effect_micros = optrd_cash_effect_micros.or(effect.cash_effect_micros);

            let applied = AppliedOptionLifecycleEffect {
                lifecycle_activity_id: lifecycle_activity_id.to_string(),
                broker_account_id: broker_account_id.to_string(),
                engine_id: engine_id.to_string(),
                mode: mode.to_string(),
                option_symbol: effect.option_symbol.clone(),
                underlying_symbol: Some(effect.underlying_symbol.clone()),
                option_contracts_removed_raw: effect.option_contracts_removed.to_string(),
                underlying_shares_delivered_raw,
                cash_effect_micros,
                applied_at_utc: now_utc,
            };
            match insert_applied_option_lifecycle_effect_if_new(pool, &applied).await? {
                InsertAppliedOptionLifecycleEffectOutcome::Applied => {
                    Ok(ApplyOptionLifecycleOutcome::Applied(applied))
                }
                InsertAppliedOptionLifecycleEffectOutcome::AlreadyApplied => {
                    // A concurrent caller won the race between our
                    // existence check above and this insert -- re-read the
                    // durable row rather than trusting our own locally-
                    // computed `applied` value, which may differ from
                    // whatever genuinely committed first.
                    let existing = fetch_applied_option_lifecycle_effect(
                        pool,
                        broker_account_id,
                        lifecycle_activity_id,
                    )
                    .await?
                    .expect(
                        "AlreadyApplied implies a row exists; a concurrent delete of this \
                         evidence-only table would itself be a repository bug",
                    );
                    Ok(ApplyOptionLifecycleOutcome::AlreadyApplied(existing))
                }
            }
        }
    }
}
