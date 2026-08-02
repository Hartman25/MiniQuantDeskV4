// core-rs/crates/mqk-daemon/src/routes/dynamic_selection_evidence.rs
//
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 7C Part 4: read-only durable
// dynamic-selection plan evidence truth.
//
// GET /api/v1/dynamic-selection/status
// GET /api/v1/dynamic-selection/plans?limit=&run_id=
// GET /api/v1/dynamic-selection/plans/:plan_id
//
// GET-only: no route in this file ever starts/stops/mutates a run, a mode,
// or a host. `approved_for_live` is always `false`. Every field here is
// either committed run-scoped truth (read from
// `AppState::dynamic_selection_runtime_snapshot`) or a `preview_*`-prefixed
// fresh env-config evaluation — the two are never mixed, mirroring
// `DynamicSelectionReadinessProjection`'s existing contract. Historical
// reads use the Part 1 bounded fetch seam and the Part 3 read-side
// validator — never an unbounded read, never evidence synthesized from the
// current environment.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::dynamic_selection_evidence_validator::{
    validate_dynamic_selection_evidence, verify_signal_journal_attribution,
    DynamicSelectionEvidenceValidationState, DYNAMIC_SELECTION_EVIDENCE_READ_BOUND,
};
use crate::state::AppState;
use mqk_db::BoundedDynamicSelectionPlanFetch;

// ---------------------------------------------------------------------------
// Shared row/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
    detail: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct DynamicSelectionSymbolRow {
    pub symbol: String,
    pub selected_strategy_id: Option<String>,
    pub disposition: String,
    pub reason_code: String,
    pub exact_reason_code: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct DynamicSelectionCandidateRow {
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    pub selected: bool,
    pub disposition: String,
    pub reason_code: String,
    pub exact_reason_code: String,
    pub canonical_score_decimal: Option<String>,
    pub scanner_rank: Option<i32>,
}

fn symbol_row_from_record(rec: &mqk_db::DynamicSelectionPlanSymbolRecord) -> DynamicSelectionSymbolRow {
    DynamicSelectionSymbolRow {
        symbol: rec.symbol.clone(),
        selected_strategy_id: rec.selected_strategy_id.clone(),
        disposition: rec.disposition.clone(),
        reason_code: rec.reason_code.clone(),
        exact_reason_code: rec.exact_reason_code.clone(),
    }
}

fn candidate_row_from_record(
    rec: &mqk_db::DynamicSelectionPlanCandidateRecord,
) -> DynamicSelectionCandidateRow {
    DynamicSelectionCandidateRow {
        symbol: rec.symbol.clone(),
        strategy_id: rec.strategy_id.clone(),
        timeframe_secs: rec.timeframe_secs,
        selected: rec.selected,
        disposition: rec.disposition.clone(),
        reason_code: rec.reason_code.clone(),
        exact_reason_code: rec.exact_reason_code.clone(),
        canonical_score_decimal: rec.canonical_score_decimal.clone(),
        scanner_rank: rec.scanner_rank,
    }
}

fn validation_code(state: &DynamicSelectionEvidenceValidationState) -> &'static str {
    state.code()
}

fn validation_detail(state: &DynamicSelectionEvidenceValidationState) -> Option<String> {
    match state {
        DynamicSelectionEvidenceValidationState::Valid
        | DynamicSelectionEvidenceValidationState::Missing
        | DynamicSelectionEvidenceValidationState::LiveApprovalViolation => None,
        DynamicSelectionEvidenceValidationState::Incomplete { detail }
        | DynamicSelectionEvidenceValidationState::CandidateMismatch { detail }
        | DynamicSelectionEvidenceValidationState::RuntimeMismatch { detail } => {
            Some(detail.clone())
        }
        DynamicSelectionEvidenceValidationState::IdentityMismatch {
            stored_plan_id,
            recomputed_plan_id,
        } => Some(format!(
            "stored plan_id={stored_plan_id} recomputed plan_id={recomputed_plan_id}"
        )),
        DynamicSelectionEvidenceValidationState::RunMismatch {
            expected_run_id,
            stored_run_id,
        } => Some(format!(
            "expected run_id={expected_run_id} stored run_id={stored_run_id}"
        )),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/dynamic-selection/status
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct DynamicSelectionStatusResponse {
    /// `"idle"` | `"starting"` | `"running"` | `"degraded"`.
    pub lifecycle: &'static str,
    pub active_run_id: Option<Uuid>,

    // Committed truth (from `dynamic_selection_runtime_snapshot`) — every
    // field below is `None`/`false`/empty when no commitment exists yet.
    pub committed_configured_mode: Option<String>,
    pub committed_effective_mode: Option<String>,
    pub committed_live_lock_applied: bool,
    /// Always `false`.
    pub approved_for_live: bool,
    pub disposition: Option<String>,
    pub committed_plan_id: Option<Uuid>,
    pub committed_source_kind: Option<String>,
    pub committed_source_identity: Option<String>,
    pub committed_plan_truth_state: Option<String>,
    pub evidence_persisted: bool,
    /// The durable read-side validation state re-checked fresh against the
    /// current DB row (`valid` | `missing` | `incomplete` |
    /// `identity_mismatch` | `run_mismatch` | `candidate_mismatch` |
    /// `runtime_mismatch` | `live_approval_violation`), or `null` when no
    /// committed plan_id exists to validate.
    pub evidence_validation_state: Option<&'static str>,
    pub evidence_validation_detail: Option<String>,
    pub selected: Vec<DynamicSelectionSymbolRow>,
    pub refused: Vec<DynamicSelectionSymbolRow>,
    pub host_pool_present: bool,
    pub host_pool_count: u64,
    pub validation_blockers: Vec<String>,

    // Preview (fresh env-config evaluation) — never proof of an active
    // run's binding.
    pub preview_configured_mode: String,
    pub preview_effective_mode: String,
    pub preview_live_lock_applied: bool,

    pub checked_at_utc: String,
}

pub(crate) async fn dynamic_selection_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let checked_at_utc = Utc::now().to_rfc3339();
    let lifecycle = st.local_runtime_lifecycle_label().await;
    let active_run_id = st.local_runtime_owning_run_id().await;

    let mode_resolution = crate::dynamic_selection_mode::resolve_dynamic_selection_mode_from_env();
    let preview =
        crate::dynamic_selection_mode::effective_mode(&mode_resolution, st.deployment_mode(), st.runtime_selection().broker_kind);

    let mut response = DynamicSelectionStatusResponse {
        lifecycle,
        active_run_id,
        committed_configured_mode: None,
        committed_effective_mode: None,
        committed_live_lock_applied: false,
        approved_for_live: false,
        disposition: None,
        committed_plan_id: None,
        committed_source_kind: None,
        committed_source_identity: None,
        committed_plan_truth_state: None,
        evidence_persisted: false,
        evidence_validation_state: None,
        evidence_validation_detail: None,
        selected: vec![],
        refused: vec![],
        host_pool_present: false,
        host_pool_count: 0,
        validation_blockers: vec![],
        preview_configured_mode: preview.configured_mode.as_str().to_string(),
        preview_effective_mode: preview.effective_mode.as_str().to_string(),
        preview_live_lock_applied: preview.live_lock_applied,
        checked_at_utc: checked_at_utc.clone(),
    };

    let Some(snapshot) = st.dynamic_selection_runtime_snapshot().await else {
        return (StatusCode::OK, Json(response)).into_response();
    };

    response.committed_configured_mode = Some(snapshot.configured_mode.as_str().to_string());
    response.committed_effective_mode = Some(snapshot.effective_mode.as_str().to_string());
    response.committed_live_lock_applied = snapshot.live_lock_applied;
    response.disposition = Some(format!("{:?}", snapshot.disposition));
    response.host_pool_present = snapshot.host_pool_present;
    response.host_pool_count = snapshot.selected_pair_count() as u64;
    response.evidence_persisted = snapshot.evidence_persisted;

    let Some(plan_id) = snapshot.plan_id else {
        return (StatusCode::OK, Json(response)).into_response();
    };
    response.committed_plan_id = Some(plan_id);

    let Some(db) = st.db.as_ref() else {
        response.validation_blockers = vec!["db_unavailable".to_string()];
        return (StatusCode::OK, Json(response)).into_response();
    };

    let expected_bindings: Vec<(String, String, i64)> = snapshot.selected_pairs.clone();
    let validation = validate_dynamic_selection_evidence(
        db,
        plan_id,
        snapshot.run_id,
        Some(&expected_bindings),
    )
    .await;
    response.evidence_validation_state = Some(validation_code(&validation));
    response.evidence_validation_detail = validation_detail(&validation);
    if !validation.is_valid() {
        if let Some(detail) = &response.evidence_validation_detail {
            response.validation_blockers.push(detail.clone());
        } else {
            response.validation_blockers.push(validation_code(&validation).to_string());
        }
    }

    // Part 3: verify signal-journal attribution for the selected bindings —
    // folded into validation_blockers on contradiction (never silently
    // dropped, never promoted to a hard truth_state change on its own).
    if !expected_bindings.is_empty() {
        if let Err(detail) =
            verify_signal_journal_attribution(db, snapshot.run_id, &expected_bindings, 500).await
        {
            response.validation_blockers.push(detail);
        }
    }

    match mqk_db::fetch_dynamic_selection_plan_for_read(
        db,
        plan_id,
        DYNAMIC_SELECTION_EVIDENCE_READ_BOUND,
    )
    .await
    {
        Ok(BoundedDynamicSelectionPlanFetch::Complete(header, symbols, _candidates)) => {
            response.committed_source_kind = Some(header.source_kind.clone());
            response.committed_source_identity = Some(header.source_identity.clone());
            response.committed_plan_truth_state = Some(header.truth_state.clone());
            for s in &symbols {
                if s.disposition == "selected" {
                    response.selected.push(symbol_row_from_record(s));
                } else {
                    response.refused.push(symbol_row_from_record(s));
                }
            }
        }
        Ok(BoundedDynamicSelectionPlanFetch::CandidateLimitExceeded { plan, observed_at_least }) => {
            response.committed_source_kind = Some(plan.source_kind.clone());
            response.committed_source_identity = Some(plan.source_identity.clone());
            response.committed_plan_truth_state = Some(plan.truth_state.clone());
            response.validation_blockers.push(format!(
                "candidate count exceeds the read bound; observed at least {observed_at_least}"
            ));
        }
        Ok(BoundedDynamicSelectionPlanFetch::NotFound) => {
            response.validation_blockers.push("evidence_row_not_found".to_string());
        }
        Err(err) => {
            tracing::warn!(error = %err, plan_id = %plan_id, "dynamic_selection_status_plan_query_failed");
            response.validation_blockers.push("evidence_query_failed".to_string());
        }
    }

    (StatusCode::OK, Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/dynamic-selection/plans?limit=&run_id=
// ---------------------------------------------------------------------------

const DEFAULT_PLANS_LIST_LIMIT: i64 = 20;
const MAX_PLANS_LIST_LIMIT: i64 = 100;

#[derive(Debug, Deserialize)]
pub(crate) struct PlansListParams {
    pub limit: Option<i64>,
    pub run_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DynamicSelectionPlanSummaryRow {
    pub plan_id: Uuid,
    pub run_id: Uuid,
    pub disposition: String,
    pub effective_mode: String,
    pub truth_state: String,
    pub selected_count: i32,
    pub candidate_count: i32,
    pub created_at_utc: String,
    pub validation_state: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct DynamicSelectionPlansListResponse {
    pub run_id: Option<Uuid>,
    pub plans: Vec<DynamicSelectionPlanSummaryRow>,
    pub checked_at_utc: String,
}

pub(crate) async fn dynamic_selection_plans(
    State(st): State<Arc<AppState>>,
    Query(params): Query<PlansListParams>,
) -> impl IntoResponse {
    let checked_at_utc = Utc::now().to_rfc3339();

    let explicit_run_id = match params.run_id.as_deref().map(|s| s.parse::<Uuid>()) {
        None => None,
        Some(Ok(id)) => Some(id),
        Some(Err(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "invalid_request",
                    detail: "run_id query parameter is not a valid UUID".to_string(),
                }),
            )
                .into_response();
        }
    };

    let Some(run_id) = explicit_run_id.or(st.local_runtime_owning_run_id().await) else {
        return (
            StatusCode::OK,
            Json(DynamicSelectionPlansListResponse {
                run_id: None,
                plans: vec![],
                checked_at_utc,
            }),
        )
            .into_response();
    };

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(DynamicSelectionPlansListResponse {
                run_id: Some(run_id),
                plans: vec![],
                checked_at_utc,
            }),
        )
            .into_response();
    };

    let limit = params.limit.unwrap_or(DEFAULT_PLANS_LIST_LIMIT).clamp(1, MAX_PLANS_LIST_LIMIT);

    match mqk_db::fetch_recent_dynamic_selection_plans(db, run_id, limit).await {
        Ok(records) => {
            let mut plans = Vec::with_capacity(records.len());
            for rec in &records {
                let validation =
                    validate_dynamic_selection_evidence(db, rec.plan_id, run_id, None).await;
                plans.push(DynamicSelectionPlanSummaryRow {
                    plan_id: rec.plan_id,
                    run_id: rec.run_id,
                    disposition: rec.disposition.clone(),
                    effective_mode: rec.effective_mode.clone(),
                    truth_state: rec.truth_state.clone(),
                    selected_count: rec.selected_count,
                    candidate_count: rec.candidate_count,
                    created_at_utc: rec.created_at_utc.to_rfc3339(),
                    validation_state: validation_code(&validation),
                });
            }
            (
                StatusCode::OK,
                Json(DynamicSelectionPlansListResponse {
                    run_id: Some(run_id),
                    plans,
                    checked_at_utc,
                }),
            )
                .into_response()
        }
        Err(err) => {
            tracing::warn!(error = %err, run_id = %run_id, "dynamic_selection_plans_query_failed");
            (
                StatusCode::OK,
                Json(DynamicSelectionPlansListResponse {
                    run_id: Some(run_id),
                    plans: vec![],
                    checked_at_utc,
                }),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/dynamic-selection/plans/:plan_id
// ---------------------------------------------------------------------------

const INVALID_PLAN_ID_MESSAGE: &str = "plan_id path parameter is not a valid UUID";

#[derive(Debug, Serialize)]
pub(crate) struct DynamicSelectionPlanDetailResponse {
    pub found: bool,
    pub plan_id: Uuid,
    pub run_id: Option<Uuid>,
    pub disposition: Option<String>,
    pub effective_mode: Option<String>,
    pub configured_mode: Option<String>,
    pub live_lock_applied: Option<bool>,
    pub approved_for_live: Option<bool>,
    pub truth_state: Option<String>,
    pub blockers: Vec<String>,
    pub created_at_utc: Option<String>,
    pub symbols: Vec<DynamicSelectionSymbolRow>,
    pub candidates: Vec<DynamicSelectionCandidateRow>,
    pub validation_state: &'static str,
    pub validation_detail: Option<String>,
    pub checked_at_utc: String,
}

pub(crate) async fn dynamic_selection_plan_by_id(
    State(st): State<Arc<AppState>>,
    Path(plan_id_raw): Path<String>,
) -> impl IntoResponse {
    let checked_at_utc = Utc::now().to_rfc3339();

    let Ok(plan_id) = plan_id_raw.parse::<Uuid>() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                error: "invalid_request",
                detail: INVALID_PLAN_ID_MESSAGE.to_string(),
            }),
        )
            .into_response();
    };

    let not_found = || DynamicSelectionPlanDetailResponse {
        found: false,
        plan_id,
        run_id: None,
        disposition: None,
        effective_mode: None,
        configured_mode: None,
        live_lock_applied: None,
        approved_for_live: None,
        truth_state: None,
        blockers: vec![],
        created_at_utc: None,
        symbols: vec![],
        candidates: vec![],
        validation_state: "missing",
        validation_detail: None,
        checked_at_utc: checked_at_utc.clone(),
    };

    let Some(db) = st.db.as_ref() else {
        return (StatusCode::OK, Json(not_found())).into_response();
    };

    let fetch = match mqk_db::fetch_dynamic_selection_plan_for_read(
        db,
        plan_id,
        DYNAMIC_SELECTION_EVIDENCE_READ_BOUND,
    )
    .await
    {
        Ok(f) => f,
        Err(err) => {
            tracing::warn!(error = %err, plan_id = %plan_id, "dynamic_selection_plan_by_id_query_failed");
            let mut resp = not_found();
            resp.validation_state = "incomplete";
            return (StatusCode::OK, Json(resp)).into_response();
        }
    };

    let (header, symbols, candidates) = match fetch {
        BoundedDynamicSelectionPlanFetch::NotFound => {
            return (StatusCode::OK, Json(not_found())).into_response();
        }
        BoundedDynamicSelectionPlanFetch::CandidateLimitExceeded { plan, .. } => {
            let mut resp = not_found();
            resp.found = true;
            resp.run_id = Some(plan.run_id);
            resp.truth_state = Some(plan.truth_state.clone());
            resp.validation_state = "incomplete";
            return (StatusCode::OK, Json(resp)).into_response();
        }
        BoundedDynamicSelectionPlanFetch::Complete(header, symbols, candidates) => {
            (header, symbols, candidates)
        }
    };

    let validation = validate_dynamic_selection_evidence(db, plan_id, header.run_id, None).await;

    (
        StatusCode::OK,
        Json(DynamicSelectionPlanDetailResponse {
            found: true,
            plan_id,
            run_id: Some(header.run_id),
            disposition: Some(header.disposition.clone()),
            effective_mode: Some(header.effective_mode.clone()),
            configured_mode: Some(header.configured_mode.clone()),
            live_lock_applied: Some(header.live_lock_applied),
            approved_for_live: Some(header.approved_for_live),
            truth_state: Some(header.truth_state.clone()),
            blockers: header.blockers.clone(),
            created_at_utc: Some(header.created_at_utc.to_rfc3339()),
            symbols: symbols.iter().map(symbol_row_from_record).collect(),
            candidates: candidates.iter().map(candidate_row_from_record).collect(),
            validation_state: validation_code(&validation),
            validation_detail: validation_detail(&validation),
            checked_at_utc,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{shared_test_locks::strategy_fleet_env_test_lock, BrokerKind, DeploymentMode};
    use axum::body::to_bytes;

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// No DB, no committed run: lifecycle must be `idle`, every committed
    /// field neutral/absent, and the preview fields must still reflect the
    /// current (unset -> off) env config -- never mixed with committed
    /// truth, mirroring `DynamicSelectionReadinessProjection`'s contract.
    #[tokio::test]
    async fn status_with_no_db_and_no_committed_run_is_all_neutral() {
        let _guard = strategy_fleet_env_test_lock().lock().await;
        std::env::remove_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
        );

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        assert!(state.db.is_none(), "precondition: no DB configured");

        let resp = dynamic_selection_status(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;

        assert_eq!(body["lifecycle"], "idle");
        assert!(body["active_run_id"].is_null());
        assert!(body["committed_configured_mode"].is_null());
        assert!(body["committed_effective_mode"].is_null());
        assert_eq!(body["committed_live_lock_applied"], false);
        assert_eq!(body["approved_for_live"], false);
        assert!(body["disposition"].is_null());
        assert!(body["committed_plan_id"].is_null());
        assert_eq!(body["evidence_persisted"], false);
        assert!(body["evidence_validation_state"].is_null());
        assert!(body["selected"].as_array().unwrap().is_empty());
        assert!(body["refused"].as_array().unwrap().is_empty());
        assert_eq!(body["host_pool_present"], false);
        assert_eq!(body["host_pool_count"], 0);
        assert_eq!(body["preview_configured_mode"], "off");
        assert_eq!(body["preview_effective_mode"], "off");
    }

    /// The `limit` query parameter must clamp into `[1, 100]` -- never an
    /// unbounded read, and never zero (a caller-supplied `0` must still
    /// return at least one row's worth of bound, not silently become an
    /// empty-forever query).
    #[tokio::test]
    async fn plans_list_with_no_db_returns_empty_and_never_errors_on_bad_limit() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        assert!(state.db.is_none(), "precondition: no DB configured");
        let run_id = Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            b"test.routes.dynamic_selection_evidence.plans_list_bad_limit",
        );

        let resp = dynamic_selection_plans(
            State(Arc::clone(&state)),
            Query(PlansListParams {
                limit: Some(0),
                run_id: Some(run_id.to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["run_id"], run_id.to_string());
        assert!(
            body["plans"].as_array().unwrap().is_empty(),
            "no DB pool means no rows, never an error"
        );
    }

    #[tokio::test]
    async fn plans_list_rejects_a_malformed_run_id() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let resp = dynamic_selection_plans(
            State(state),
            Query(PlansListParams {
                limit: None,
                run_id: Some("not-a-uuid".to_string()),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// An unknown `plan_id` (no DB, so nothing could ever be found) must
    /// return `found: false` and `validation_state: "missing"` -- never a
    /// fabricated "valid" result and never an error for a simple absence.
    #[tokio::test]
    async fn plan_detail_for_unknown_id_with_no_db_is_missing_not_an_error() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let plan_id = Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            b"test.routes.dynamic_selection_evidence.plan_detail_unknown_id",
        );

        let resp = dynamic_selection_plan_by_id(State(state), Path(plan_id.to_string()))
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["found"], false);
        assert_eq!(body["plan_id"], plan_id.to_string());
        assert_eq!(body["validation_state"], "missing");
    }

    /// End-to-end: a real row written through the Part 1 store (via the
    /// same insert function Part 2's writer calls) must be surfaced through
    /// this route with `found: true`, its real fields, and a fresh
    /// `validation_state` of `"valid"` (no `expected_bindings` supplied for
    /// a pure historical detail read, so identity/run/candidate coherence
    /// alone must be enough to pass).
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL"]
    async fn plan_detail_for_a_real_persisted_row_is_found_and_valid() {
        if std::env::var(mqk_db::ENV_DB_URL).is_err() {
            eprintln!("skipped: requires MQK_DATABASE_URL");
            return;
        }
        let pool = mqk_db::testkit_db_pool().await.expect("test pool");

        let run_id = Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            b"test.routes.dynamic_selection_evidence.plan_detail_real_row",
        );
        let _ = sqlx::query(
            "delete from sys_dynamic_selection_plan_candidates where plan_id in \
             (select plan_id from sys_dynamic_selection_plans where run_id = $1)",
        )
        .bind(run_id)
        .execute(&pool)
        .await;
        let _ = sqlx::query(
            "delete from sys_dynamic_selection_plan_symbols where plan_id in \
             (select plan_id from sys_dynamic_selection_plans where run_id = $1)",
        )
        .bind(run_id)
        .execute(&pool)
        .await;
        let _ = sqlx::query("delete from sys_dynamic_selection_plans where run_id = $1")
            .bind(run_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(&pool)
            .await;

        mqk_db::insert_run(
            &pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "test-routes-dynamic-selection-evidence".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: chrono::Utc::now(),
                git_hash: "test".to_string(),
                config_hash: "test".to_string(),
                config_json: serde_json::json!({}),
                host_fingerprint: "test".to_string(),
            },
        )
        .await
        .expect("fixture run insert");

        // The real production writer always mints plan_id via
        // derive_dynamic_selection_plan_id from the actual resolved plan --
        // mirror that here (rather than an arbitrary UUID) so the
        // read-side validator's fresh identity recomputation matches, the
        // same way a genuine write would.
        let source_plan = mqk_portfolio::DynamicSelectionPlan {
            context: mqk_portfolio::DynamicSelectionContext {
                run_id: run_id.to_string(),
                schema_version: "dynamic-strategy-symbol-selection-v1".to_string(),
                configured_mode: mqk_portfolio::DynamicSelectionMode::Shadow,
                effective_mode: mqk_portfolio::DynamicSelectionMode::Shadow,
                live_lock_applied: false,
                source_kind: "watchlist_v2".to_string(),
                source_identity: "watchlist".to_string(),
                market_date: "2099-04-01".to_string(),
            },
            symbol_results: vec![],
            truth_state: "computed".to_string(),
            blockers: vec![],
        };
        let plan_id =
            crate::dynamic_selection_dispatch_authority::derive_dynamic_selection_plan_id(
                &source_plan,
            );
        let plan = mqk_db::NewDynamicSelectionPlan {
            plan_id,
            run_id,
            library_schema_version: "dynamic-strategy-symbol-selection-v1".to_string(),
            context_schema_version: "dynamic-strategy-symbol-selection-v1".to_string(),
            configured_mode: "shadow".to_string(),
            effective_mode: "shadow".to_string(),
            live_lock_applied: false,
            approved_for_live: false,
            source_kind: "watchlist_v2".to_string(),
            source_identity: "watchlist".to_string(),
            market_date: "2099-04-01".to_string(),
            disposition: "shadow_allowed".to_string(),
            truth_state: "computed".to_string(),
            blockers: vec![],
            symbol_count: 0,
            candidate_count: 0,
            selected_count: 0,
            refused_count: 0,
            writer_version: "test".to_string(),
            created_at_utc: chrono::Utc::now(),
            symbols: vec![],
            candidates: vec![],
        };
        let outcome = mqk_db::insert_dynamic_selection_plan(&pool, plan)
            .await
            .expect("insert should succeed");
        assert_eq!(outcome, mqk_db::InsertDynamicSelectionPlanOutcome::Inserted);

        let state = Arc::new(AppState::new_with_db_and_operator_auth(
            pool.clone(),
            crate::state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let resp = dynamic_selection_plan_by_id(State(state), Path(plan_id.to_string()))
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["found"], true);
        assert_eq!(body["run_id"], run_id.to_string());
        assert_eq!(body["disposition"], "shadow_allowed");
        assert_eq!(body["truth_state"], "computed");
        assert_eq!(body["validation_state"], "valid");

        let _ = sqlx::query("delete from sys_dynamic_selection_plans where run_id = $1")
            .bind(run_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn plan_detail_rejects_a_malformed_plan_id() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let resp = dynamic_selection_plan_by_id(State(state), Path("not-a-uuid".to_string()))
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
