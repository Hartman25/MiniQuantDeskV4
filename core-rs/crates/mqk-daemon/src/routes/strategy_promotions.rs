//! STRATEGY-PROMOTION-REGISTRY-01C: strategy paper-promotion control
//! surface.
//!
//! Routes:
//!   GET  /api/v1/strategy/promotions           — current state, all identities (public)
//!   GET  /api/v1/strategy/promotions/history   — full history, one identity (public)
//!   GET  /api/v1/strategy/promotions/check     — tradability check, one identity (public)
//!   POST /api/v1/strategy/promotions/transition — operator transition mutation (operator auth)
//!
//! Safety invariants:
//! - `registered + enabled` (`sys_strategy_registry.enabled`) is never
//!   treated as promotion approval anywhere in this module.
//! - The mutation route never trusts a caller's claim that a candidate is
//!   `paper_candidate`: evidence-requiring transitions independently read,
//!   canonicalize, root-bound, and validate the actual review artifact
//!   content (same canonicalize+root-prefix pattern as the sibling
//!   `GET /api/v1/strategy-scans/review-artifact` route in
//!   `routes/strategy_scans.rs`).
//! - `tradable_live` is hard-coded `false` everywhere in this module. No
//!   code path here can set it `true`.
//! - No order, outbox, broker, run-start, or arm route is called or
//!   referenced anywhere in this module.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::api_types::{
    StrategyPromotionCheckResponse, StrategyPromotionHistoryResponse, StrategyPromotionRow,
    StrategyPromotionTransitionRequest, StrategyPromotionTransitionResponse,
    StrategyPromotionsResponse,
};
use crate::promotion_evidence_validation::validate_paper_candidate_evidence;
use crate::state::AppState;
use mqk_db::{
    evaluate_promotion_tradability, fetch_all_current_promotions, fetch_current_promotion_state,
    fetch_promotion_history, insert_strategy_promotion_transition_serialized,
    is_known_promotion_state, is_legal_transition, resolve_evidence_lineage,
    transition_requires_evidence, InsertStrategyPromotionTransitionArgs,
    StrategyPromotionTransitionRecord, TransitionInsertOutcome,
};

const CANONICAL_ROUTE_PROMOTIONS: &str = "/api/v1/strategy/promotions";
const CANONICAL_ROUTE_HISTORY: &str = "/api/v1/strategy/promotions/history";
const CANONICAL_ROUTE_CHECK: &str = "/api/v1/strategy/promotions/check";
const BACKEND: &str = "postgres.sys_strategy_promotion_transitions";
const HISTORY_LIMIT: i64 = 200;

/// STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase C): the maximum
/// amount `effective_at_utc` may be ahead of the daemon's own clock at
/// transition-creation time. Scheduled future promotions are not a
/// supported feature of this registry -- this tolerance exists only to
/// absorb ordinary clock skew/request latency between an operator's
/// client and this daemon, not to let an operator queue a promotion for
/// a future date. `evaluate_promotion_tradability` independently
/// re-checks `effective_at_utc <= now` at every read/gate evaluation as
/// defense-in-depth.
const EFFECTIVE_AT_CLOCK_SKEW_TOLERANCE: chrono::Duration = chrono::Duration::minutes(5);

async fn to_row(
    db: &sqlx::PgPool,
    record: &StrategyPromotionTransitionRecord,
    now_utc: DateTime<Utc>,
) -> StrategyPromotionRow {
    let (tradable_paper, reason_code) = evaluate_promotion_tradability(Some(record), now_utc);
    // STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase D): resolve the
    // exact evidence-bearing transition durably rather than surfacing
    // this row's own possibly-null evidence_* columns directly -- a
    // record reached via a non-evidence-bearing transition (e.g.
    // `active_paper` via `paper_approved`) must still expose the
    // evidence that originally authorized it.
    let evidence = match resolve_evidence_lineage(db, record).await {
        Ok(ev) => ev,
        Err(err) => {
            tracing::warn!(
                "resolve_evidence_lineage failed for transition_id={}: {err}",
                record.transition_id
            );
            None
        }
    };
    StrategyPromotionRow {
        transition_id: record.transition_id.to_string(),
        strategy_id: record.strategy_id.clone(),
        symbol: record.symbol.clone(),
        timeframe_secs: record.timeframe_secs,
        config_fingerprint: record.config_fingerprint.clone(),
        config_identity_status: record.config_identity_status.clone(),
        previous_state: record.previous_state.clone(),
        new_state: record.new_state.clone(),
        evidence_transition_id: evidence.as_ref().map(|e| e.transition_id.to_string()),
        evidence_review_id: evidence.as_ref().and_then(|e| e.evidence_review_id.clone()),
        evidence_scanner_scan_id: evidence
            .as_ref()
            .and_then(|e| e.evidence_scanner_scan_id.clone()),
        evidence_git_hash: evidence.as_ref().and_then(|e| e.evidence_git_hash.clone()),
        evidence_artifact_path: evidence
            .as_ref()
            .and_then(|e| e.evidence_artifact_path.clone()),
        evidence_fingerprint: evidence
            .as_ref()
            .and_then(|e| e.evidence_fingerprint.clone()),
        effective_at_utc: record.effective_at_utc.to_rfc3339(),
        expires_at_utc: record.expires_at_utc.map(|t| t.to_rfc3339()),
        initiated_by: record.initiated_by.clone(),
        reason: record.reason.clone(),
        created_at_utc: record.created_at_utc.to_rfc3339(),
        tradable_paper,
        // Hard-coded: no code path in this module can produce `true`.
        tradable_live: false,
        reason_code: reason_code.code().to_string(),
        blockers: Vec::new(),
    }
}

async fn to_rows(
    db: &sqlx::PgPool,
    records: &[StrategyPromotionTransitionRecord],
    now_utc: DateTime<Utc>,
) -> Vec<StrategyPromotionRow> {
    let mut rows = Vec::with_capacity(records.len());
    for record in records {
        rows.push(to_row(db, record, now_utc).await);
    }
    rows
}

fn normalize_strategy_id(raw: &str) -> String {
    raw.trim().to_string()
}

fn normalize_symbol(raw: &str) -> String {
    raw.trim().to_ascii_uppercase()
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/promotions
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_promotions(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(StrategyPromotionsResponse {
                canonical_route: CANONICAL_ROUTE_PROMOTIONS.to_string(),
                backend: BACKEND.to_string(),
                truth_state: "no_db".to_string(),
                rows: Vec::new(),
            }),
        );
    };

    let now_utc = Utc::now();
    match fetch_all_current_promotions(db).await {
        Ok(records) => {
            let rows = to_rows(db, &records, now_utc).await;
            (
                StatusCode::OK,
                Json(StrategyPromotionsResponse {
                    canonical_route: CANONICAL_ROUTE_PROMOTIONS.to_string(),
                    backend: BACKEND.to_string(),
                    truth_state: "active".to_string(),
                    rows,
                }),
            )
        }
        Err(err) => {
            tracing::warn!("fetch_all_current_promotions failed: {err}");
            (
                StatusCode::OK,
                Json(StrategyPromotionsResponse {
                    canonical_route: CANONICAL_ROUTE_PROMOTIONS.to_string(),
                    backend: BACKEND.to_string(),
                    truth_state: "query_failed".to_string(),
                    rows: Vec::new(),
                }),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/promotions/history
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct PromotionIdentityQuery {
    strategy_id: Option<String>,
    symbol: Option<String>,
    timeframe_secs: Option<i64>,
}

struct ParsedIdentity {
    strategy_id: String,
    symbol: String,
    timeframe_secs: i64,
}

fn parse_identity_query(q: &PromotionIdentityQuery) -> Result<ParsedIdentity, String> {
    let strategy_id = q
        .strategy_id
        .as_deref()
        .map(normalize_strategy_id)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "strategy_id query parameter is required".to_string())?;
    let symbol = q
        .symbol
        .as_deref()
        .map(normalize_symbol)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "symbol query parameter is required".to_string())?;
    let timeframe_secs = q.timeframe_secs.filter(|v| *v > 0).ok_or_else(|| {
        "timeframe_secs query parameter is required and must be positive".to_string()
    })?;
    Ok(ParsedIdentity {
        strategy_id,
        symbol,
        timeframe_secs,
    })
}

pub(crate) async fn strategy_promotion_history(
    State(st): State<Arc<AppState>>,
    Query(query): Query<PromotionIdentityQuery>,
) -> Response {
    let identity = match parse_identity_query(&query) {
        Ok(i) => i,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(StrategyPromotionHistoryResponse {
                    canonical_route: CANONICAL_ROUTE_HISTORY.to_string(),
                    backend: BACKEND.to_string(),
                    truth_state: "invalid_request".to_string(),
                    strategy_id: query.strategy_id.unwrap_or_default(),
                    symbol: query.symbol.unwrap_or_default(),
                    timeframe_secs: query.timeframe_secs.unwrap_or(0),
                    rows: Vec::new(),
                    blockers: vec![msg],
                }),
            )
                .into_response();
        }
    };

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(StrategyPromotionHistoryResponse {
                canonical_route: CANONICAL_ROUTE_HISTORY.to_string(),
                backend: BACKEND.to_string(),
                truth_state: "no_db".to_string(),
                strategy_id: identity.strategy_id,
                symbol: identity.symbol,
                timeframe_secs: identity.timeframe_secs,
                rows: Vec::new(),
                blockers: vec![
                    "durable execution DB truth is unavailable on this daemon".to_string()
                ],
            }),
        )
            .into_response();
    };

    let now_utc = Utc::now();
    match fetch_promotion_history(
        db,
        &identity.strategy_id,
        &identity.symbol,
        identity.timeframe_secs,
        HISTORY_LIMIT,
    )
    .await
    {
        Ok(records) => {
            let rows = to_rows(db, &records, now_utc).await;
            (
                StatusCode::OK,
                Json(StrategyPromotionHistoryResponse {
                    canonical_route: CANONICAL_ROUTE_HISTORY.to_string(),
                    backend: BACKEND.to_string(),
                    truth_state: "active".to_string(),
                    strategy_id: identity.strategy_id,
                    symbol: identity.symbol,
                    timeframe_secs: identity.timeframe_secs,
                    rows,
                    blockers: Vec::new(),
                }),
            )
                .into_response()
        }
        Err(err) => {
            tracing::warn!("fetch_promotion_history failed: {err}");
            (
                StatusCode::OK,
                Json(StrategyPromotionHistoryResponse {
                    canonical_route: CANONICAL_ROUTE_HISTORY.to_string(),
                    backend: BACKEND.to_string(),
                    truth_state: "query_failed".to_string(),
                    strategy_id: identity.strategy_id,
                    symbol: identity.symbol,
                    timeframe_secs: identity.timeframe_secs,
                    rows: Vec::new(),
                    blockers: vec![format!("promotion history query failed: {err}")],
                }),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/promotions/check
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_promotion_check(
    State(st): State<Arc<AppState>>,
    Query(query): Query<PromotionIdentityQuery>,
) -> Response {
    let identity = match parse_identity_query(&query) {
        Ok(i) => i,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(StrategyPromotionCheckResponse {
                    canonical_route: CANONICAL_ROUTE_CHECK.to_string(),
                    backend: BACKEND.to_string(),
                    truth_state: "invalid_request".to_string(),
                    strategy_id: query.strategy_id.unwrap_or_default(),
                    symbol: query.symbol.unwrap_or_default(),
                    timeframe_secs: query.timeframe_secs.unwrap_or(0),
                    current_state: None,
                    config_identity_status: None,
                    evidence_transition_id: None,
                    evidence_review_id: None,
                    evidence_scanner_scan_id: None,
                    evidence_git_hash: None,
                    evidence_artifact_path: None,
                    evidence_fingerprint: None,
                    tradable_paper: false,
                    tradable_live: false,
                    reason_code: "promotion_identity_mismatch".to_string(),
                    blockers: vec![msg],
                }),
            )
                .into_response();
        }
    };

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(StrategyPromotionCheckResponse {
                canonical_route: CANONICAL_ROUTE_CHECK.to_string(),
                backend: BACKEND.to_string(),
                truth_state: "no_db".to_string(),
                strategy_id: identity.strategy_id,
                symbol: identity.symbol,
                timeframe_secs: identity.timeframe_secs,
                current_state: None,
                config_identity_status: None,
                evidence_transition_id: None,
                evidence_review_id: None,
                evidence_scanner_scan_id: None,
                evidence_git_hash: None,
                evidence_artifact_path: None,
                evidence_fingerprint: None,
                tradable_paper: false,
                tradable_live: false,
                reason_code: "promotion_db_unavailable".to_string(),
                blockers: vec![
                    "durable execution DB truth is unavailable on this daemon".to_string()
                ],
            }),
        )
            .into_response();
    };

    match fetch_current_promotion_state(
        db,
        &identity.strategy_id,
        &identity.symbol,
        identity.timeframe_secs,
    )
    .await
    {
        Ok(record) => {
            let now_utc = Utc::now();
            let (tradable_paper, reason_code) =
                evaluate_promotion_tradability(record.as_ref(), now_utc);
            let evidence = match record.as_ref() {
                Some(r) => resolve_evidence_lineage(db, r).await.unwrap_or_else(|err| {
                    tracing::warn!(
                        "resolve_evidence_lineage failed for transition_id={}: {err}",
                        r.transition_id
                    );
                    None
                }),
                None => None,
            };
            (
                StatusCode::OK,
                Json(StrategyPromotionCheckResponse {
                    canonical_route: CANONICAL_ROUTE_CHECK.to_string(),
                    backend: BACKEND.to_string(),
                    truth_state: "active".to_string(),
                    strategy_id: identity.strategy_id,
                    symbol: identity.symbol,
                    timeframe_secs: identity.timeframe_secs,
                    current_state: record.as_ref().map(|r| r.new_state.clone()),
                    config_identity_status: record
                        .as_ref()
                        .map(|r| r.config_identity_status.clone()),
                    evidence_transition_id: evidence.as_ref().map(|e| e.transition_id.to_string()),
                    evidence_review_id: evidence
                        .as_ref()
                        .and_then(|e| e.evidence_review_id.clone()),
                    evidence_scanner_scan_id: evidence
                        .as_ref()
                        .and_then(|e| e.evidence_scanner_scan_id.clone()),
                    evidence_git_hash: evidence.as_ref().and_then(|e| e.evidence_git_hash.clone()),
                    evidence_artifact_path: evidence
                        .as_ref()
                        .and_then(|e| e.evidence_artifact_path.clone()),
                    evidence_fingerprint: evidence
                        .as_ref()
                        .and_then(|e| e.evidence_fingerprint.clone()),
                    tradable_paper,
                    tradable_live: false,
                    reason_code: reason_code.code().to_string(),
                    blockers: Vec::new(),
                }),
            )
                .into_response()
        }
        Err(err) => {
            tracing::warn!("fetch_current_promotion_state failed: {err}");
            (
                StatusCode::OK,
                Json(StrategyPromotionCheckResponse {
                    canonical_route: CANONICAL_ROUTE_CHECK.to_string(),
                    backend: BACKEND.to_string(),
                    truth_state: "query_failed".to_string(),
                    strategy_id: identity.strategy_id,
                    symbol: identity.symbol,
                    timeframe_secs: identity.timeframe_secs,
                    current_state: None,
                    config_identity_status: None,
                    evidence_transition_id: None,
                    evidence_review_id: None,
                    evidence_scanner_scan_id: None,
                    evidence_git_hash: None,
                    evidence_artifact_path: None,
                    evidence_fingerprint: None,
                    tradable_paper: false,
                    tradable_live: false,
                    reason_code: "promotion_query_failed".to_string(),
                    blockers: vec![format!("promotion query failed: {err}")],
                }),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/strategy/promotions/transition (operator auth)
// ---------------------------------------------------------------------------

/// Bundled arguments for [`transition_response`] — avoids an
/// unwieldy positional-argument function signature.
struct TransitionResponseArgs {
    status: StatusCode,
    accepted: bool,
    disposition: &'static str,
    strategy_id: String,
    symbol: String,
    timeframe_secs: i64,
    previous_state: Option<String>,
    target_state: String,
    transition_id: Option<Uuid>,
    blockers: Vec<String>,
}

/// PROMOTION-EVIDENCE-LINEAGE-V3: everything Gate 4c/4d resolves that the
/// durable evidence-lineage write (Gate 5b, below) needs — bundled rather
/// than an unwieldy tuple, mirroring [`TransitionResponseArgs`].
struct PromotionLineageSource {
    oos_evidence: Box<mqk_promotion::VerifiedPromotionOosEvidence>,
    backtest_run_id: Uuid,
    research_judge_artifact_sha256: String,
    stress_protocol_version: String,
    stress_artifact_sha256: String,
    robustness_protocol_version: String,
    finalized_robustness_artifact_sha256: String,
    promotion_policy_fingerprint: String,
}

fn transition_response(args: TransitionResponseArgs) -> Response {
    (
        args.status,
        Json(StrategyPromotionTransitionResponse {
            accepted: args.accepted,
            disposition: args.disposition.to_string(),
            strategy_id: args.strategy_id,
            symbol: args.symbol,
            timeframe_secs: args.timeframe_secs,
            previous_state: args.previous_state,
            target_state: args.target_state,
            transition_id: args.transition_id.map(|t| t.to_string()),
            blockers: args.blockers,
        }),
    )
        .into_response()
}

pub(crate) async fn strategy_promotion_transition(
    State(st): State<Arc<AppState>>,
    Json(body): Json<StrategyPromotionTransitionRequest>,
) -> Response {
    let strategy_id = normalize_strategy_id(&body.strategy_id);
    let symbol = normalize_symbol(&body.symbol);
    let timeframe_secs = body.timeframe_secs;
    let target_state = body.target_state.trim().to_string();
    let initiated_by = body.initiated_by.trim().to_string();

    // Gate 0: field validation.
    let mut blockers = Vec::new();
    if strategy_id.is_empty() {
        blockers.push("strategy_id must not be blank".to_string());
    }
    if symbol.is_empty() {
        blockers.push("symbol must not be blank".to_string());
    }
    if symbol == "*" {
        blockers.push("wildcard symbol '*' is forbidden".to_string());
    }
    if timeframe_secs <= 0 {
        blockers.push("timeframe_secs must be positive".to_string());
    }
    if initiated_by.is_empty() {
        blockers.push("initiated_by must not be blank".to_string());
    }
    if !is_known_promotion_state(&target_state) {
        blockers.push(format!(
            "target_state '{target_state}' is not a known promotion state"
        ));
    }
    let effective_at_utc = match DateTime::parse_from_rfc3339(body.effective_at_utc.trim()) {
        Ok(dt) => Some(dt.with_timezone(&Utc)),
        Err(e) => {
            blockers.push(format!("effective_at_utc must be RFC3339: {e}"));
            None
        }
    };
    let expires_at_utc = match body
        .expires_at_utc
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => Some(None),
        Some(raw) => match DateTime::parse_from_rfc3339(raw) {
            Ok(dt) => Some(Some(dt.with_timezone(&Utc))),
            Err(e) => {
                blockers.push(format!("expires_at_utc must be RFC3339: {e}"));
                None
            }
        },
    };

    // STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase C): temporal
    // sanity checks, only meaningful once both timestamps parsed cleanly
    // above (a parse failure already pushed its own blocker and left the
    // corresponding `Option` as `None`, so these checks are skipped for a
    // malformed timestamp rather than double-reporting the same field).
    if let Some(effective_at) = effective_at_utc {
        let now = Utc::now();
        if effective_at - now > EFFECTIVE_AT_CLOCK_SKEW_TOLERANCE {
            blockers.push(format!(
                "effective_at_utc ({}) is more than {} minutes ahead of the daemon's current \
                 time ({}); scheduled future promotions are not supported -- this tolerance \
                 exists only to absorb ordinary clock skew",
                effective_at.to_rfc3339(),
                EFFECTIVE_AT_CLOCK_SKEW_TOLERANCE.num_minutes(),
                now.to_rfc3339(),
            ));
        }
        if let Some(Some(expires_at)) = expires_at_utc {
            if expires_at <= effective_at {
                blockers.push(format!(
                    "expires_at_utc ({}) must be later than effective_at_utc ({})",
                    expires_at.to_rfc3339(),
                    effective_at.to_rfc3339(),
                ));
            }
        }
    }

    if !blockers.is_empty() {
        return transition_response(TransitionResponseArgs {
            status: StatusCode::BAD_REQUEST,
            accepted: false,
            disposition: "rejected",
            strategy_id,
            symbol,
            timeframe_secs,
            previous_state: None,
            target_state,
            transition_id: None,
            blockers,
        });
    }
    let effective_at_utc = effective_at_utc.expect("validated above");
    let expires_at_utc = expires_at_utc.expect("validated above");

    // Deterministic transition_id: UUIDv5 over exactly the client-supplied,
    // replay-stable request content — strategy_id/symbol/timeframe_secs/
    // target_state/review_dir (raw, as supplied)/effective_at_utc/
    // expires_at_utc/initiated_by/reason. Deliberately EXCLUDES any
    // server-computed value (previous_state, evidence_fingerprint): those
    // can differ between the original call and an exact replay of the same
    // request (the first call already advanced current state, and re-
    // reading the same evidence file recomputes an identical fingerprint
    // anyway), which would silently break idempotency by minting a
    // different id for what the client considers "the same request".
    let review_dir_raw = body.review_dir.as_deref().unwrap_or("").trim().to_string();
    // PROMOTION-WALKFORWARD-GATE-WIRING-01: the three P7C fields, and
    // (PRODUCTION-PROMOTION-DB-E2E-01) `backtest_run_id`, are all
    // client-supplied request content that materially changes the
    // transition's evidentiary basis, exactly like `review_dir` above --
    // included in the deterministic seed so two requests differing only in
    // which Research trial/evidence or which Backtest run they claim can
    // never collide on the same `transition_id`. Without `backtest_run_id`
    // in this seed, a retry that is byte-identical except for a DIFFERENT
    // (unvalidated) `backtest_run_id` would be misdiagnosed as an exact
    // replay by Gate 1b above and short-circuited to `disposition:
    // "duplicate"` -- silently skipping Gate 4c/4d re-validation of the
    // second request's own claimed evidence entirely.
    let research_trial_id_raw = body
        .research_trial_id
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let research_evidence_dir_raw = body
        .research_evidence_dir
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let research_judge_artifact_path_raw = body
        .research_judge_artifact_path
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let backtest_run_id_raw = body
        .backtest_run_id
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let seed = format!(
        "mqk-strategy-promotion-transition.v1|strategy_id={strategy_id}|symbol={symbol}|\
         timeframe_secs={timeframe_secs}|target_state={target_state}|review_dir={review_dir_raw}|\
         research_trial_id={research_trial_id_raw}|research_evidence_dir={research_evidence_dir_raw}|\
         research_judge_artifact_path={research_judge_artifact_path_raw}|\
         backtest_run_id={backtest_run_id_raw}|\
         effective_at_utc={}|expires_at_utc={:?}|initiated_by={initiated_by}|reason={}",
        effective_at_utc.to_rfc3339(),
        expires_at_utc.map(|t| t.to_rfc3339()),
        body.reason.trim(),
    );
    let transition_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes());

    // Gate 1: DB must be present.
    let Some(db) = st.db.as_ref() else {
        return transition_response(TransitionResponseArgs {
            status: StatusCode::SERVICE_UNAVAILABLE,
            accepted: false,
            disposition: "unavailable",
            strategy_id,
            symbol,
            timeframe_secs,
            previous_state: None,
            target_state,
            transition_id: None,
            blockers: vec!["durable execution DB truth is unavailable on this daemon".to_string()],
        });
    };

    // Gate 1b: idempotency pre-check. If a transition with this exact
    // deterministic id already exists, this is a replay of an
    // already-accepted request — report "duplicate" using the already-
    // recorded row's own previous_state, without ever reaching Gate 3
    // (legality). This is required, not merely an optimization: by the
    // time a replay arrives, the identity's *actual* current state has
    // already moved past what it was when the original request was
    // accepted, so re-deriving previous_state fresh (Gate 2 below) and
    // running legality against it would spuriously reject an exact replay
    // as `illegal_transition` (e.g. a replayed `shadow_approved` request
    // arriving after the identity has already advanced to
    // `shadow_approved` would look like a `shadow_approved -> shadow_approved`
    // self-loop, which is never legal). This exact failure mode is why
    // this pre-check exists — see
    // docs/specs/strategy_promotion_registry_01f_closure_decision.md §7's
    // "Phase C" bug note for the original occurrence of this same defect.
    let existing = match mqk_db::fetch_promotion_transition_by_id(db, transition_id).await {
        Ok(row) => row,
        Err(err) => {
            return transition_response(TransitionResponseArgs {
                status: StatusCode::SERVICE_UNAVAILABLE,
                accepted: false,
                disposition: "unavailable",
                strategy_id,
                symbol,
                timeframe_secs,
                previous_state: None,
                target_state,
                transition_id: None,
                blockers: vec![format!("idempotency pre-check query failed: {err}")],
            });
        }
    };
    if let Some(existing) = existing {
        return transition_response(TransitionResponseArgs {
            status: StatusCode::OK,
            accepted: true,
            disposition: "duplicate",
            strategy_id,
            symbol,
            timeframe_secs,
            previous_state: existing.previous_state.clone(),
            target_state,
            transition_id: Some(transition_id),
            blockers: vec![
                "identical transition already recorded; no new row was created".to_string(),
            ],
        });
    }

    // Gate 2: compute actual current state (never trust a caller-supplied
    // previous_state — there is none in the request schema by design).
    // This is an optimistic, pre-lock read used only to decide legality
    // (Gate 3) and evidence requirements (Gate 4) below; it is never
    // trusted as the final word on the identity's parent transition --
    // `insert_strategy_promotion_transition_serialized` (Gate 5)
    // re-reads the true current state itself, inside a transaction-scoped
    // advisory lock, and rejects the insert with `transition_conflict` if
    // this optimistic read has gone stale by the time the lock is
    // acquired (STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 Phase B).
    let current =
        match fetch_current_promotion_state(db, &strategy_id, &symbol, timeframe_secs).await {
            Ok(c) => c,
            Err(err) => {
                return transition_response(TransitionResponseArgs {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    accepted: false,
                    disposition: "unavailable",
                    strategy_id,
                    symbol,
                    timeframe_secs,
                    previous_state: None,
                    target_state,
                    transition_id: None,
                    blockers: vec![format!("current promotion state query failed: {err}")],
                });
            }
        };
    let previous_state = current.as_ref().map(|c| c.new_state.clone());

    // Gate 3: transition legality.
    if !is_legal_transition(previous_state.as_deref(), &target_state) {
        let blocker = format!(
            "illegal transition: {} -> {target_state}",
            previous_state.as_deref().unwrap_or("no_state")
        );
        return transition_response(TransitionResponseArgs {
            status: StatusCode::CONFLICT,
            accepted: false,
            disposition: "illegal_transition",
            strategy_id,
            symbol,
            timeframe_secs,
            previous_state,
            target_state,
            transition_id: None,
            blockers: vec![blocker],
        });
    }

    // Gate 4: evidence validation, only when this transition requires it.
    let evidence = if transition_requires_evidence(previous_state.as_deref(), &target_state) {
        let review_dir = match body.review_dir.as_deref() {
            Some(d) if !d.trim().is_empty() => d,
            _ => {
                return transition_response(TransitionResponseArgs {
                    status: StatusCode::BAD_REQUEST,
                    accepted: false,
                    disposition: "evidence_invalid",
                    strategy_id,
                    symbol,
                    timeframe_secs,
                    previous_state,
                    target_state,
                    transition_id: None,
                    blockers: vec!["review_dir is required for this transition".to_string()],
                });
            }
        };
        match validate_paper_candidate_evidence(
            &st,
            review_dir,
            &strategy_id,
            &symbol,
            timeframe_secs,
        ) {
            Ok(ev) => Some(ev),
            Err(msg) => {
                return transition_response(TransitionResponseArgs {
                    status: StatusCode::BAD_REQUEST,
                    accepted: false,
                    disposition: "evidence_invalid",
                    strategy_id,
                    symbol,
                    timeframe_secs,
                    previous_state,
                    target_state,
                    transition_id: None,
                    blockers: vec![msg],
                });
            }
        }
    } else {
        None
    };

    // Gate 4c/4d (PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE):
    // resolve REAL canonical Backtest evidence AND verified Research
    // out-of-sample evidence for THIS SAME candidate, then route the
    // complete decision through the canonical `mqk_promotion::
    // evaluate_promotion` -- required ADDITIONALLY to (never instead of)
    // Gate 4's scanner/review evidence above. Runs in the exact same
    // `transition_requires_evidence` branch, so a transition that does not
    // require evidence at all is unaffected. Every gate below reads its
    // registry/artifact-root/policy-threshold config ONLY from trusted
    // daemon config (`st`) -- never from this request -- so nothing in
    // `body` can select an alternate registry, artifact root, or
    // acceptance threshold.
    //
    // Repairs the independent-review findings against `242cb7c3`:
    // (1) cross-candidate authority -- both gates below independently
    //     cross-check their resolved evidence's own strategy identity
    //     against `strategy_id`, never just trusting a caller-supplied id
    //     claim to imply the evidence describes the same candidate;
    // (2) parallel/partial promotion policy -- DSR/PBO and every other
    //     promotion gate (metrics, artifact lock, stress suite, provenance)
    //     are now decided in ONE place, `evaluate_promotion`, not
    //     re-implemented ad hoc in this route;
    // (3) missing durable Research lineage -- the verified DSR/PBO and
    //     Backtest run_id are persisted (see Gate 5 below), not merely
    //     logged;
    // (4) missing canonical backtest-evidence seam -- closed by
    //     PROMOTION-BACKTEST-EVIDENCE-SEAM-01 (`resolve_backtest_evidence`).
    let promotion_lineage = if transition_requires_evidence(previous_state.as_deref(), &target_state)
    {
        let (research_trial_id, research_evidence_dir, research_judge_artifact_path, backtest_run_id) =
            match (
                body.research_trial_id.as_deref(),
                body.research_evidence_dir.as_deref(),
                body.research_judge_artifact_path.as_deref(),
                body.backtest_run_id.as_deref(),
            ) {
                (Some(t), Some(d), Some(j), Some(b))
                    if !t.trim().is_empty()
                        && !d.trim().is_empty()
                        && !j.trim().is_empty()
                        && !b.trim().is_empty() =>
                {
                    (t, d, j, b)
                }
                _ => {
                    return transition_response(TransitionResponseArgs {
                        status: StatusCode::BAD_REQUEST,
                        accepted: false,
                        disposition: "evidence_invalid",
                        strategy_id,
                        symbol,
                        timeframe_secs,
                        previous_state,
                        target_state,
                        transition_id: None,
                        blockers: vec![
                            "research_trial_id, research_evidence_dir, \
                             research_judge_artifact_path, and backtest_run_id are all required \
                             for this transition (PROMOTION-WALKFORWARD-GATE-WIRING-01: verified \
                             Research out-of-sample evidence AND canonical Backtest evidence are \
                             both required in addition to review-artifact evidence)"
                                .to_string(),
                        ],
                    });
                }
            };

        let backtest_bundle = match crate::backtest_evidence_gate::evaluate_backtest_evidence_gate(
            &st,
            backtest_run_id,
            &strategy_id,
        ) {
            crate::backtest_evidence_gate::BacktestEvidenceGateOutcome::Passed { bundle } => bundle,
            crate::backtest_evidence_gate::BacktestEvidenceGateOutcome::Rejected { blockers } => {
                return transition_response(TransitionResponseArgs {
                    status: StatusCode::BAD_REQUEST,
                    accepted: false,
                    disposition: "evidence_invalid",
                    strategy_id,
                    symbol,
                    timeframe_secs,
                    previous_state,
                    target_state,
                    transition_id: None,
                    blockers,
                });
            }
        };

        let oos_evidence = match crate::research_evidence_gate::evaluate_research_evidence_gate(
            &st,
            &strategy_id,
            research_trial_id,
            research_evidence_dir,
            research_judge_artifact_path,
        ) {
            crate::research_evidence_gate::ResearchEvidenceGateOutcome::Passed { oos_evidence } => {
                oos_evidence
            }
            crate::research_evidence_gate::ResearchEvidenceGateOutcome::Rejected { blockers } => {
                return transition_response(TransitionResponseArgs {
                    status: StatusCode::BAD_REQUEST,
                    accepted: false,
                    disposition: "evidence_invalid",
                    strategy_id,
                    symbol,
                    timeframe_secs,
                    previous_state,
                    target_state,
                    transition_id: None,
                    blockers,
                });
            }
        };

        // Canonical promotion policy config -- trusted daemon config ONLY.
        let policy = match (
            st.promotion_min_sharpe,
            st.promotion_max_mdd,
            st.promotion_min_cagr,
            st.promotion_min_profit_factor,
            st.promotion_min_profitable_months_pct,
            st.research_min_deflated_sharpe_ratio,
            st.research_max_probability_backtest_overfitting,
        ) {
            (
                Some(min_sharpe),
                Some(max_mdd),
                Some(min_cagr),
                Some(min_profit_factor),
                Some(min_profitable_months_pct),
                Some(min_deflated_sharpe_ratio),
                Some(max_probability_backtest_overfitting),
            ) => mqk_promotion::PromotionConfig {
                min_sharpe,
                max_mdd,
                min_cagr,
                min_profit_factor,
                min_profitable_months_pct,
                min_deflated_sharpe_ratio,
                max_probability_backtest_overfitting,
            },
            _ => {
                return transition_response(TransitionResponseArgs {
                    status: StatusCode::BAD_REQUEST,
                    accepted: false,
                    disposition: "evidence_invalid",
                    strategy_id,
                    symbol,
                    timeframe_secs,
                    previous_state,
                    target_state,
                    transition_id: None,
                    blockers: vec![
                        "promotion policy rejected: this daemon does not have every \
                         MQK_PROMOTION_* / MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO / \
                         MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING threshold configured \
                         -- the canonical promotion policy cannot be evaluated"
                            .to_string(),
                    ],
                });
            }
        };

        let promotion_input = mqk_promotion::PromotionInput {
            initial_equity_micros: backtest_bundle.initial_equity_micros,
            report: backtest_bundle.report.clone(),
            stress_suite: Some(backtest_bundle.stress_suite.clone()),
            artifact_lock: Some(backtest_bundle.artifact_lock.clone()),
            oos_evidence: Some(*oos_evidence.clone()),
            robustness_evidence: Some(backtest_bundle.robustness_evidence.clone()),
        };
        let decision = mqk_promotion::evaluate_promotion(&policy, &promotion_input);
        if !decision.passed {
            return transition_response(TransitionResponseArgs {
                status: StatusCode::BAD_REQUEST,
                accepted: false,
                disposition: "evidence_invalid",
                strategy_id,
                symbol,
                timeframe_secs,
                previous_state,
                target_state,
                transition_id: None,
                blockers: decision.fail_reasons,
            });
        }

        tracing::info!(
            strategy_id = %strategy_id,
            symbol = %symbol,
            timeframe_secs,
            research_trial_id = %oos_evidence.trial_id(),
            research_economic_eval_id = %oos_evidence.economic_eval_id(),
            deflated_sharpe_ratio = oos_evidence.deflated_sharpe_ratio(),
            probability_of_backtest_overfitting = oos_evidence.probability_of_backtest_overfitting(),
            backtest_run_id = %backtest_bundle.run_id,
            "PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE: canonical evaluate_promotion \
             accepted verified Research + Backtest evidence for shadow_approved transition",
        );

        // PROMOTION-EVIDENCE-LINEAGE-V3: capture everything the durable
        // lineage write below needs -- every hash here reuses one already
        // verified above (research_evidence_gate's registry-anchored judge
        // hash, backtest_evidence_gate's content-hash-verified stress/
        // robustness artifacts) except promotion_policy_fingerprint, the
        // one genuinely new fingerprint (see PromotionConfig::
        // deterministic_fingerprint's own docs). Computed before
        // `oos_evidence` moves into the struct below.
        let research_judge_artifact_sha256 = oos_evidence.judge_artifact_sha256().to_string();
        Some(PromotionLineageSource {
            oos_evidence,
            backtest_run_id: backtest_bundle.run_id,
            research_judge_artifact_sha256,
            stress_protocol_version: backtest_bundle.stress_suite.protocol_version.clone(),
            stress_artifact_sha256: backtest_bundle.stress_artifact_sha256.clone(),
            robustness_protocol_version: backtest_bundle.robustness_evidence.protocol_version.clone(),
            finalized_robustness_artifact_sha256: backtest_bundle
                .finalized_robustness_artifact_sha256
                .clone(),
            promotion_policy_fingerprint: policy.deterministic_fingerprint(),
        })
    } else {
        None
    };

    // STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase D): derive
    // evidence lineage server-side, never from the request. A
    // freshly-validated evidence-bearing transition (Gate 4 above)
    // establishes itself as the new evidence root (`evidence_transition_id
    // == transition_id`); every other transition carries forward the
    // current record's own resolved evidence lineage unchanged.
    let evidence_transition_id = if evidence.is_some() {
        Some(transition_id)
    } else {
        current.as_ref().and_then(|c| c.evidence_transition_id)
    };
    let parent_transition_id = current.as_ref().map(|c| c.transition_id);

    let args = InsertStrategyPromotionTransitionArgs {
        transition_id,
        strategy_id: strategy_id.clone(),
        symbol: symbol.clone(),
        timeframe_secs,
        config_fingerprint: None,
        config_identity_status: "unavailable_in_current_runtime".to_string(),
        previous_state: previous_state.clone(),
        new_state: target_state.clone(),
        parent_transition_id,
        evidence_transition_id,
        evidence_review_id: evidence.as_ref().map(|e| e.review_id.clone()),
        evidence_scanner_scan_id: evidence.as_ref().map(|e| e.scanner_scan_id.clone()),
        evidence_git_hash: evidence.as_ref().map(|e| e.git_hash.clone()),
        evidence_artifact_path: evidence.as_ref().map(|e| e.artifact_path.clone()),
        evidence_fingerprint: evidence.as_ref().map(|e| e.fingerprint.clone()),
        // Defect B: newly written evidence stores legacy and v2
        // fingerprints together -- computed from the exact raw score token,
        // never the lossy `f64` the legacy fingerprint's serialization
        // already went through.
        evidence_fingerprint_v2: evidence.as_ref().map(|e| {
            crate::promotion_evidence_validation::compute_evidence_fingerprint_v2(
                &e.review_id,
                &e.scanner_scan_id,
                &e.git_hash,
                &strategy_id,
                &symbol,
                timeframe_secs,
                &e.review_state,
                e.scanner_score_token.as_deref(),
                e.scanner_rank,
                &e.reason_codes,
                &e.blockers,
                &e.warnings,
            )
        }),
        effective_at_utc,
        expires_at_utc,
        initiated_by,
        reason: body.reason.trim().to_string(),
        created_at_utc: Utc::now(), // allow: ops-metadata write timestamp, not lifecycle truth
    };

    // PROMOTION-LINEAGE-ATOMICITY-01: derive the durable evidence-lineage
    // write up front so it can be passed into the SAME atomic transaction
    // as the transition insert -- never a second, independently-failable
    // step after the transition is already committed.
    let evidence_lineage = promotion_lineage.as_ref().map(|src| mqk_db::PromotionEvidenceLineageV3 {
        research_trial_id: Some(src.oos_evidence.trial_id().to_string()),
        research_economic_eval_id: Some(src.oos_evidence.economic_eval_id().to_string()),
        research_deflated_sharpe_ratio: Some(src.oos_evidence.deflated_sharpe_ratio()),
        research_probability_backtest_overfitting: Some(
            src.oos_evidence.probability_of_backtest_overfitting(),
        ),
        backtest_run_id: Some(src.backtest_run_id),
        research_judge_artifact_sha256: Some(src.research_judge_artifact_sha256.clone()),
        stress_protocol_version: Some(src.stress_protocol_version.clone()),
        stress_artifact_sha256: Some(src.stress_artifact_sha256.clone()),
        robustness_protocol_version: Some(src.robustness_protocol_version.clone()),
        finalized_robustness_artifact_sha256: Some(
            src.finalized_robustness_artifact_sha256.clone(),
        ),
        promotion_policy_fingerprint: Some(src.promotion_policy_fingerprint.clone()),
    });

    // Gate 5: atomic, serialized insert (STRATEGY-PROMOTION-REGISTRY-
    // CLOSURE-REPAIR-01 Phase B) -- the only insert path this route uses.
    // `evidence_lineage` (Gate 5b) is written durably atomically with the
    // transition row itself inside this same call -- see
    // `insert_strategy_promotion_transition_serialized`'s own docs
    // (PROMOTION-LINEAGE-ATOMICITY-01).
    match insert_strategy_promotion_transition_serialized(db, &args, evidence_lineage.as_ref())
        .await
    {
        Ok(TransitionInsertOutcome::Inserted(_)) => {
            transition_response(TransitionResponseArgs {
                status: StatusCode::OK,
                accepted: true,
                disposition: "transitioned",
                strategy_id,
                symbol,
                timeframe_secs,
                previous_state,
                target_state,
                transition_id: Some(transition_id),
                blockers: Vec::new(),
            })
        }
        Ok(TransitionInsertOutcome::Duplicate(existing)) => {
            transition_response(TransitionResponseArgs {
                status: StatusCode::OK,
                accepted: true,
                disposition: "duplicate",
                strategy_id,
                symbol,
                timeframe_secs,
                previous_state: existing.previous_state.clone(),
                target_state,
                transition_id: Some(transition_id),
                blockers: vec![
                    "identical transition already recorded; no new row was created".to_string(),
                ],
            })
        }
        Ok(TransitionInsertOutcome::Conflict { current: actual }) => {
            let actual_state = actual.as_ref().map(|a| a.new_state.clone());
            transition_response(TransitionResponseArgs {
                status: StatusCode::CONFLICT,
                accepted: false,
                disposition: "transition_conflict",
                strategy_id,
                symbol,
                timeframe_secs,
                previous_state: actual_state.clone(),
                target_state,
                transition_id: None,
                blockers: vec![format!(
                    "a concurrent transition already advanced this identity past the expected \
                     parent state; expected previous_state={:?}, actual current state={:?} -- \
                     re-read current state and retry",
                    previous_state, actual_state
                )],
            })
        }
        Err(err) => transition_response(TransitionResponseArgs {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            accepted: false,
            disposition: "unavailable",
            strategy_id,
            symbol,
            timeframe_secs,
            previous_state,
            target_state,
            transition_id: None,
            blockers: vec![format!("transition insert failed: {err}")],
        }),
    }
}
