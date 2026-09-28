//! D2 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): the
//! distinct, idempotent options-lifecycle accounting transaction D1's raw
//! evidence ledger requires before it means anything economically.
//!
//! Composes three independently-proven pieces (mirrors B6's
//! `state/crypto_fee_ingestion.rs` composition pattern exactly):
//! - `mqk_db::option_lifecycle_activity` (D1): durable raw evidence +
//!   OPEXC/OPASN <-> OPTRD pairing lookup + idempotent-apply marker.
//! - `mqk_portfolio::option_lifecycle::classify_option_lifecycle_event`
//!   (the existing Wave D4 model): pure classification into a
//!   `PairedLifecycleEffect` or a fail-closed `Pending`.
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
//! No production caller wired in this patch -- Alpaca options capability
//! does not exist in this codebase yet.

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
    /// A quantity/price field in the ledger did not parse as a decimal
    /// number.
    MalformedDecimal { field: &'static str, raw: String },
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
fn parse_decimal_to_micros(field: &'static str, raw: &str) -> Result<i64, PendingApplyReason> {
    let raw_trim = raw.trim();
    let (sign, unsigned) = match raw_trim.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, raw_trim),
    };
    let parts: Vec<&str> = unsigned.splitn(2, '.').collect();
    let whole: i64 = parts[0]
        .parse()
        .map_err(|_| PendingApplyReason::MalformedDecimal {
            field,
            raw: raw.to_string(),
        })?;
    let frac_micros: i64 = match parts.get(1) {
        None => 0,
        Some(frac) => {
            if frac.is_empty() || frac.len() > 6 || !frac.chars().all(|c| c.is_ascii_digit()) {
                return Err(PendingApplyReason::MalformedDecimal {
                    field,
                    raw: raw.to_string(),
                });
            }
            format!("{frac:0<6}")
                .parse()
                .map_err(|_| PendingApplyReason::MalformedDecimal {
                    field,
                    raw: raw.to_string(),
                })?
        }
    };
    Ok(sign * (whole * 1_000_000 + frac_micros))
}

/// Run one apply attempt for `lifecycle_activity_id` (an `OPEXC`/`OPASN`/
/// `OPEXP` activity already durably ingested by D1). Idempotent: a second
/// call for the same `lifecycle_activity_id` after a successful apply
/// returns `AlreadyApplied` with zero new mutation.
pub async fn apply_option_lifecycle_activity(
    pool: &PgPool,
    engine_id: &str,
    mode: &str,
    lifecycle_activity_id: &str,
    terms: &OptionContractTerms,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<ApplyOptionLifecycleOutcome> {
    if let Some(existing) =
        fetch_applied_option_lifecycle_effect(pool, lifecycle_activity_id).await?
    {
        return Ok(ApplyOptionLifecycleOutcome::AlreadyApplied(existing));
    }

    let Some(raw) = fetch_option_lifecycle_activity(pool, lifecycle_activity_id).await? else {
        anyhow::bail!(
            "apply_option_lifecycle_activity: activity {lifecycle_activity_id} not found in \
             sys_option_lifecycle_activity_ledger -- D1 ingestion must run first"
        );
    };

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

    if matches!(
        kind,
        OptionLifecycleEventKind::Exercise | OptionLifecycleEventKind::Assignment
    ) {
        let Some(optrd) =
            find_paired_trade_activity(pool, &raw.option_symbol, &raw.activity_date).await?
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
    }

    let contracts_micros = match parse_decimal_to_micros("qty_raw", &raw.qty_raw) {
        Ok(v) => v.abs(),
        Err(reason) => return Ok(ApplyOptionLifecycleOutcome::Pending { reason }),
    };

    let evidence = LifecycleBrokerEvidence {
        kind,
        option_symbol: raw.option_symbol.clone(),
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
            let applied = AppliedOptionLifecycleEffect {
                lifecycle_activity_id: lifecycle_activity_id.to_string(),
                engine_id: engine_id.to_string(),
                mode: mode.to_string(),
                option_symbol: effect.option_symbol.clone(),
                underlying_symbol: Some(effect.underlying_symbol.clone()),
                option_contracts_removed_raw: effect.option_contracts_removed.to_string(),
                underlying_shares_delivered_raw: effect
                    .underlying_shares_delivered
                    .map(|q| q.to_string()),
                cash_effect_micros: effect.cash_effect_micros,
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
                    let existing =
                        fetch_applied_option_lifecycle_effect(pool, lifecycle_activity_id)
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
