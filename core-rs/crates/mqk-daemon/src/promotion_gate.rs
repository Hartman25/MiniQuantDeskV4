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

use chrono::Utc;
use sqlx::PgPool;

use mqk_db::{evaluate_promotion_tradability, fetch_current_promotion_state, PromotionReasonCode};

/// Outcome of evaluating whether `(strategy_id, symbol, timeframe_secs)` is
/// currently authorized to create a new PAPER outbox row.
#[derive(Debug, Clone)]
pub struct PromotionGateOutcome {
    /// `true` only when the identity's current promotion state is
    /// `active_paper` and not expired as of evaluation time.
    pub paper_tradable: bool,
    /// Stable machine-readable reason code (see `mqk_db::PromotionReasonCode`).
    pub reason_code: PromotionReasonCode,
    /// Human-readable blocker text. Empty when `paper_tradable` is `true`.
    pub blocker: String,
}

/// Evaluate the paper-promotion gate for one exact identity.
///
/// Callers must have already established DB presence and produced their
/// own `unavailable`/`no_db` disposition before calling this — it takes
/// `&PgPool` (not `Option<&PgPool>`) and always performs exactly one query.
pub async fn evaluate_paper_promotion_gate(
    db: &PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) -> PromotionGateOutcome {
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
    let (paper_tradable, reason_code) = evaluate_promotion_tradability(record.as_ref(), now_utc);
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
