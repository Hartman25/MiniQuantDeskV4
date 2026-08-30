//! STRATEGY-PROMOTION-REGISTRY-01D: shared paper-promotion runtime gate.
//!
//! One evaluation mechanism, used identically by both strategy-originated
//! outbox write paths — the internal decision seam
//! (`decision::submit_internal_strategy_decision`) and the external signal
//! route (`routes::strategy::strategy_signal`) — so promotion enforcement
//! can never drift between them, mirroring the existing shared-gate pattern
//! already used for sector risk
//! (`capital_policy::sector_risk_gate::evaluate_sector_risk_gate`).
//!
//! # Runtime tradability rule
//!
//! `registered + enabled` (`sys_strategy_registry.enabled`) is never
//! sufficient for paper trading. Only an exact-identity
//! `(strategy_id, symbol, timeframe_secs)` match with current promotion
//! state `active_paper`, not expired, authorizes a new paper outbox row.
//!
//! # No live authorization
//!
//! This gate never checks or grants LIVE authorization. It answers exactly
//! one question — "is this identity currently paper-tradable?" — and
//! nothing else. No live-routing path calls this module.
//!
//! # Durable truth only
//!
//! This gate reads durable DB promotion truth via `mqk_db`. It never opens
//! or reads a scanner/review artifact file — evidence was already
//! independently validated once, at approval-transition time (see
//! `routes/strategy_promotions.rs`), and durable DB state is the only
//! truth read on every subsequent decision.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_db::{
    evaluate_promotion_tradability, fetch_current_promotion_state, PromotionReasonCode,
    StrategyPromotionTransitionRecord,
};

/// STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase E): the exact
/// runtime context this gate is being evaluated from. The gate no longer
/// relies solely on "no LIVE call site exists today" as its safety
/// argument — every caller must now say which mode it is evaluating in,
/// and only `Paper` may ever observe `active_paper` as tradable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionRunMode {
    /// The only mode in which `active_paper` may be paper-tradable.
    Paper,
    /// A LIVE or live-routing context. Denied unconditionally, regardless
    /// of the identity's actual promotion state.
    Live,
    /// Any context that is not affirmatively known to be `Paper` (e.g.
    /// `Backtest`, or any future mode this gate has not been explicitly
    /// taught about). Fails closed, identically to `Live`.
    Unknown,
}

impl From<crate::state::DeploymentMode> for PromotionRunMode {
    fn from(mode: crate::state::DeploymentMode) -> Self {
        match mode {
            crate::state::DeploymentMode::Paper => PromotionRunMode::Paper,
            crate::state::DeploymentMode::LiveShadow
            | crate::state::DeploymentMode::LiveCapital => PromotionRunMode::Live,
            crate::state::DeploymentMode::Backtest => PromotionRunMode::Unknown,
        }
    }
}

/// Outcome of evaluating whether `(strategy_id, symbol, timeframe_secs)` is
/// currently authorized to create a new PAPER outbox row.
#[derive(Debug, Clone)]
pub struct PromotionGateOutcome {
    /// `true` only when `mode == PromotionRunMode::Paper` AND the
    /// identity's current promotion state is `active_paper`, effective,
    /// and not expired as of evaluation time.
    pub paper_tradable: bool,
    /// Stable machine-readable reason code (see `mqk_db::PromotionReasonCode`).
    pub reason_code: PromotionReasonCode,
    /// Human-readable blocker text. Empty when `paper_tradable` is `true`.
    pub blocker: String,
}

/// RUNTIME-PROMOTION-EVIDENCE-BINDING-01 (C2): the ONE canonical comparison
/// primitive answering "is this durable promotion record ALSO authorized
/// for the exact strategy semantic configuration actually executing right
/// now" -- layered strictly on top of [`evaluate_promotion_tradability`]'s
/// durable-state-only truth, never a second independent implementation of
/// it. Reused identically by the runtime dispatch gate below AND by
/// `routes::strategy_promotions::to_row` (GET observability), so an
/// operator-facing `tradable_paper` can never claim `true` when this exact
/// primitive would refuse the same identity at dispatch time.
///
/// Requires, in order:
/// 1. durable state is `active_paper`, effective, and unexpired (delegated
///    to [`evaluate_promotion_tradability`] -- its own reason code is
///    returned unchanged the moment this is `false`, so every existing
///    denial reason for a non-active/expired/not-yet-effective identity is
///    completely unaffected by this function's additional check);
/// 2. `config_identity_status == "verified_v1"` AND `config_fingerprint` is
///    `Some` and exactly 64 lowercase hex -- a legacy `NULL`/unverified
///    row can never wildcard-match, regardless of `current_fingerprint`;
/// 3. `current_fingerprint` is `Some`, exactly 64 lowercase hex, and
///    byte-equal to the promoted fingerprint.
///
/// Any failure at step 2 or 3 returns `PromotionReasonCode::PromotionConfigMismatch`
/// -- the one, narrowest existing reason code that truthfully covers every
/// one of these refusal causes (legacy/unavailable identity, unresolvable
/// current config, or genuine drift).
pub fn evaluate_promotion_tradability_with_config_identity(
    record: Option<&StrategyPromotionTransitionRecord>,
    now_utc: DateTime<Utc>,
    current_fingerprint: Option<&str>,
) -> (bool, PromotionReasonCode) {
    let (durable_tradable, durable_reason) = evaluate_promotion_tradability(record, now_utc);
    if !durable_tradable {
        return (durable_tradable, durable_reason);
    }
    // `durable_tradable == true` only when `evaluate_promotion_tradability`
    // matched `PROMOTION_STATE_ACTIVE_PAPER` on a `Some` record -- `record`
    // is therefore always `Some` here.
    let record = record.expect("durable_tradable=true implies record is Some");

    // CANONICAL-CONFIG-IDENTITY-VERIFIER-01 (R7): the ONE canonical
    // verification predicate, never a second independent reimplementation
    // of "status == verified_v1 AND both fingerprints valid AND equal".
    if crate::strategy_config_identity::config_identity_is_verified(
        &record.config_identity_status,
        record.config_fingerprint.as_deref(),
        current_fingerprint,
    ) {
        (true, PromotionReasonCode::PromotionActive)
    } else {
        (false, PromotionReasonCode::PromotionConfigMismatch)
    }
}

/// How a caller of [`evaluate_paper_promotion_gate`] can (or cannot)
/// establish the semantic identity of the exact instance/config producing a
/// decision. Every real caller must say which of these it is — there is no
/// default, and no path from `ExternallyUnavailable` to `Fingerprint` other
/// than the caller genuinely having one to offer.
#[derive(Debug, Clone, Copy)]
pub enum SemanticProvenance<'a> {
    /// RUNTIME-PROMOTION-EVIDENCE-BINDING-01 (C2): the caller's own
    /// server-derived [`mqk_strategy::Strategy::semantic_fingerprint`] for
    /// the exact instance producing this decision -- `Some` from the
    /// internal dispatch path (the already-running host's own captured
    /// fingerprint); `None` when the caller attempted resolution but the
    /// identity itself could not be established (e.g. an unknown
    /// `strategy_id`, or a registered timeframe that disagrees with the
    /// request). `None` can never match any promoted fingerprint -- see
    /// [`evaluate_promotion_tradability_with_config_identity`].
    Fingerprint(Option<&'a str>),
    /// EXTERNAL-SIGNAL-SEMANTIC-PROVENANCE-FAIL-CLOSED-01: this call site
    /// has NO trusted provenance channel for the decision at all. A
    /// server-side reconstruction (re-instantiating a daemon-native
    /// strategy and reading its fingerprint) would prove only THIS
    /// daemon's own current configuration -- never the configuration or
    /// logic that actually produced an externally-submitted decision's
    /// side/qty -- so it must never be treated as a substitute for real
    /// provenance. Durable state is still evaluated honestly first: an
    /// identity that is not even `active_paper`/effective/unexpired reports
    /// its own real reason, never masked by a provenance-unavailable
    /// disposition that would misdiagnose an unrelated gate failure.
    ExternallyUnavailable,
}

/// Evaluate the paper-promotion gate for one exact identity.
///
/// STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase E): `mode` is a
/// required, explicit input — not inferred from "which function called
/// this." `PromotionRunMode::Live` and `PromotionRunMode::Unknown` are
/// denied unconditionally with `PromotionReasonCode::PromotionLiveNotAuthorized`
/// before the DB is even queried, so a paper promotion can never be
/// observed as tradable from a non-`Paper` runtime context, regardless of
/// what future call site might reuse this function.
///
/// See [`SemanticProvenance`] for `semantic_provenance`'s contract.
///
/// Callers must have already established DB presence and produced their
/// own `unavailable`/`no_db` disposition before calling this — it takes
/// `&PgPool` (not `Option<&PgPool>`) and always performs exactly one query
/// (and only when `mode == Paper`).
pub async fn evaluate_paper_promotion_gate(
    db: &PgPool,
    mode: PromotionRunMode,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    semantic_provenance: SemanticProvenance<'_>,
) -> PromotionGateOutcome {
    if mode != PromotionRunMode::Paper {
        return PromotionGateOutcome {
            paper_tradable: false,
            reason_code: PromotionReasonCode::PromotionLiveNotAuthorized,
            blocker: format!(
                "strategy '{strategy_id}' symbol '{symbol}' timeframe_secs={timeframe_secs}: \
                 the paper-promotion gate only ever authorizes trading in PAPER runtime mode \
                 (current mode: {mode:?}); a paper promotion state can never authorize a LIVE \
                 run or live-routing path"
            ),
        };
    }

    let record = match fetch_current_promotion_state(db, strategy_id, symbol, timeframe_secs).await
    {
        Ok(r) => r,
        Err(err) => {
            return PromotionGateOutcome {
                paper_tradable: false,
                reason_code: PromotionReasonCode::PromotionQueryFailed,
                blocker: format!("paper promotion query failed: {err}"),
            };
        }
    };

    let now_utc = Utc::now();
    let (paper_tradable, reason_code) = match semantic_provenance {
        SemanticProvenance::Fingerprint(fp) => {
            evaluate_promotion_tradability_with_config_identity(record.as_ref(), now_utc, fp)
        }
        SemanticProvenance::ExternallyUnavailable => {
            let (durable_tradable, durable_reason) =
                evaluate_promotion_tradability(record.as_ref(), now_utc);
            if durable_tradable {
                // Durable state alone says this identity is active/effective/
                // unexpired -- but this channel can never prove semantic
                // provenance for it. A genuinely different, narrower reason
                // than a real (mismatched) fingerprint comparison.
                (
                    false,
                    PromotionReasonCode::PromotionExternalSemanticProvenanceUnavailable,
                )
            } else {
                // The real, unrelated reason (not yet promoted, expired,
                // demoted, etc.) -- never masked by a provenance disposition.
                (false, durable_reason)
            }
        }
    };
    let blocker = if paper_tradable {
        String::new()
    } else {
        format!(
            "strategy '{strategy_id}' symbol '{symbol}' timeframe_secs={timeframe_secs} is not \
             paper-promoted for trading (reason: {}); registered+enabled in sys_strategy_registry \
             is not sufficient -- an explicit active_paper promotion transition is required via \
             POST /api/v1/strategy/promotions/transition",
            reason_code.code()
        )
    };

    PromotionGateOutcome {
        paper_tradable,
        reason_code,
        blocker,
    }
}
