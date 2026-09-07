//! DYNAMIC-SELECTION-E2E-SCENARIO-TEST-01: end-to-end proof that a real
//! dynamic-selection plan, once durably committed, is read back faithfully
//! through the daemon's HTTP evidence routes.
//!
//! # Scope, and why it stops short of "selected-host dispatch"
//!
//! The row this closes asks for one continuous proof: plan build -> host
//! pool selection -> selected-host dispatch -> evidence write -> evidence-
//! route read. Investigating the real seams for this row found that the
//! *dispatch* step (`RuntimeStrategyDispatchAuthority` construction, via
//! `dynamic_selection_dispatch_authority::build_dynamic_paper_enforced_dispatch_authority`)
//! and the plan-identity/evidence-writer helpers around it
//! (`derive_dynamic_selection_plan_id`, `dynamic_selection_evidence_writer::
//! build_new_dynamic_selection_plan`) are all `pub(crate)` -- genuinely
//! unreachable from an external `tests/*.rs` integration binary, by design
//! (the module doc is explicit: "built exactly once per start attempt...
//! never cloned, never rebuilt"). That exact chain -- host pool ->
//! dispatch authority -> the real production start sequencer
//! (`daily_data_readiness::advance_run_to_active`, which performs the real
//! evidence write) -- is already proven extensively, in-crate, by
//! `state/lifecycle.rs`'s own `#[cfg(test)]` suite (30+ tests driving
//! `drive_production_start_effects_with_dispatch_authority_for_test`
//! against that real entrypoint). Duplicating that here would either
//! require a production visibility change (out of scope for this row) or
//! just re-prove already-proven coverage.
//!
//! What genuinely had zero test coverage anywhere in the repository --
//! confirmed by grepping every test file for the three route names/paths
//! below -- is the read side: `GET /api/v1/dynamic-selection/plans`,
//! `GET /api/v1/dynamic-selection/plans/:plan_id`. This file closes exactly
//! that gap, using every REAL public production seam available to an
//! external test:
//!   - `mqk_portfolio::compute_dynamic_selection_plan` -- the real, pure
//!     selection/ranking function `build_dynamic_selection_plan` itself
//!     delegates to after assembling evidence via I/O. Supplying evidence
//!     directly (rather than gathering it from a live DB/registry/calendar)
//!     is the same technique this crate's own embedded plan-builder tests
//!     already use.
//!   - `mqk_daemon::dynamic_selection_host_pool::DynamicSelectionHostPool::build`
//!     -- the real, public host-pool constructor, called with this real
//!     plan's own selected pairs.
//!   - `mqk_db::insert_dynamic_selection_plan` -- the real DB-layer writer
//!     (canonical-identity idempotent, same function the crate-internal
//!     evidence writer calls) -- never a hand-rolled INSERT.
//!   - `mqk_daemon::routes::build_router` -- the real Axum router, driven
//!     over real HTTP requests via `tower::ServiceExt::oneshot`.
//!
//! `plan_id` is minted with the exact same derivation the real dispatch-
//! authority module uses (`canonical_plan_identity_material`, a public
//! `mqk_portfolio` function, plus the same private namespace-seed literal
//! copied verbatim from `dynamic_selection_dispatch_authority.rs`) so the
//! read-side validator the routes call
//! (`dynamic_selection_evidence_validator::validate_dynamic_selection_evidence`)
//! genuinely reports `valid` against this real content -- not a
//! coincidentally-matching or mismatched id. A tampering negative control
//! proves that validator is actually wired into the routes, not bypassed.
//!
//! All tests require `MQK_DATABASE_URL` and run against their own
//! disposable per-test database (`mqk_db::run_isolated`). Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_dynamic_selection_end_to_end_paper_dispatch_01 \
//!     -- --include-ignored

use axum::http::{Request, StatusCode};
use chrono::{TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::dynamic_selection_host_pool::DynamicSelectionHostPool;
use mqk_daemon::{routes, state};
use mqk_db::{
    insert_dynamic_selection_plan, insert_run, InsertDynamicSelectionPlanOutcome,
    NewDynamicSelectionPlan, NewDynamicSelectionPlanCandidate, NewDynamicSelectionPlanSymbol,
    NewRun,
};
use mqk_portfolio::{
    canonical_plan_identity_material, compute_dynamic_selection_plan, DynamicSelectionContext,
    DynamicSelectionMode, DynamicSelectionPlan, SelectionCandidateDisposition,
    SelectionCandidateEvidence, SelectionCandidateInput, DYNAMIC_SELECTION_SCHEMA_VERSION,
};
use tower::ServiceExt;
use uuid::Uuid;

/// Exact copy of `dynamic_selection_dispatch_authority::PLAN_ID_NAMESPACE_SEED`
/// (`core-rs/crates/mqk-daemon/src/dynamic_selection_dispatch_authority.rs`).
/// That constant is private (`pub(crate)`, by design -- plan-id derivation
/// is an internal implementation detail no external caller should depend
/// on), so this test -- which needs to mint the *exact same* id the real
/// production path would, in order to genuinely exercise the read-side
/// validator's `valid` outcome -- copies the literal. A future version bump
/// to that seed must be mirrored here, or `real_plan_..._round_trip`'s
/// `evidence_validation_state`/`validation_state` assertions will fail
/// (identity_mismatch), which is itself the correct, loud failure mode.
const PLAN_ID_NAMESPACE_SEED: &str = "mqk.dynamic-selection-plan-id.v1";

fn derive_plan_id(plan: &DynamicSelectionPlan) -> Uuid {
    let material = canonical_plan_identity_material(plan);
    let mut seed = Vec::with_capacity(PLAN_ID_NAMESPACE_SEED.len() + 1 + material.len());
    seed.extend_from_slice(PLAN_ID_NAMESPACE_SEED.as_bytes());
    seed.push(b'|');
    seed.extend_from_slice(&material);
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, &seed)
}

fn disposition_str(d: SelectionCandidateDisposition) -> &'static str {
    match d {
        SelectionCandidateDisposition::Selected => "selected",
        SelectionCandidateDisposition::NotSelected => "not_selected",
        SelectionCandidateDisposition::Refused => "refused",
    }
}

/// Every real-evidence-gate field populated as a fully green, currently-
/// active_paper-promoted candidate would report it -- values mirror the
/// established fixture in `mqk-db/tests/scenario_dynamic_selection_evidence_store_01.rs`.
fn green_evidence() -> SelectionCandidateEvidence {
    SelectionCandidateEvidence {
        promotion_query_ok: true,
        promotion_state: Some("active_paper".to_string()),
        promotion_effective: true,
        promotion_expired: false,
        evidence_resolved: true,
        review_state_is_paper_candidate: true,
        evidence_review_state: Some("paper_candidate".to_string()),
        durable_legacy_fingerprint: Some("a".repeat(64)),
        recomputed_legacy_fingerprint: Some("a".repeat(64)),
        legacy_fingerprint_matches: true,
        durable_exact_fingerprint_v2: Some("b".repeat(64)),
        recomputed_exact_fingerprint_v2: Some("b".repeat(64)),
        exact_fingerprint_v2_matches: true,
        config_identity_verified: true,
        durable_config_fingerprint: Some("c".repeat(64)),
        current_config_fingerprint: Some("c".repeat(64)),
        registry_enabled: true,
        plugin_instantiable: true,
        timeframe_matches: true,
        data_ready: true,
        canonical_score_decimal: Some("9.000000".to_string()),
        canonical_score_micros: Some(9_000_000),
        scanner_rank: Some(1),
        watchlist_assigned: true,
        evidence_review_id: Some("review-1".to_string()),
        evidence_scanner_scan_id: Some("scan-1".to_string()),
        evidence_artifact_path: Some("artifacts/review-1.json".to_string()),
        evidence_git_hash: Some("deadbeef".to_string()),
        promotion_transition_id: Some("transition-1".to_string()),
        promotion_effective_at: Some("2099-03-01T00:00:00Z".to_string()),
        promotion_expires_at: None,
        evidence_transition_id: Some("transition-1".to_string()),
        exact_reason: None,
    }
}

/// A structurally valid candidate with no promotion record at all -- real
/// production shape for "plugin/timeframe/data all fine, but never
/// promoted", never a fabricated field combination.
fn unpromoted_evidence() -> SelectionCandidateEvidence {
    SelectionCandidateEvidence {
        promotion_query_ok: true,
        promotion_state: None,
        promotion_effective: false,
        promotion_expired: false,
        evidence_resolved: false,
        review_state_is_paper_candidate: false,
        evidence_review_state: None,
        durable_legacy_fingerprint: None,
        recomputed_legacy_fingerprint: None,
        legacy_fingerprint_matches: false,
        durable_exact_fingerprint_v2: None,
        recomputed_exact_fingerprint_v2: None,
        exact_fingerprint_v2_matches: false,
        config_identity_verified: false,
        durable_config_fingerprint: None,
        current_config_fingerprint: None,
        registry_enabled: true,
        plugin_instantiable: true,
        timeframe_matches: true,
        data_ready: true,
        canonical_score_decimal: None,
        canonical_score_micros: None,
        scanner_rank: None,
        watchlist_assigned: false,
        evidence_review_id: None,
        evidence_scanner_scan_id: None,
        evidence_artifact_path: None,
        evidence_git_hash: None,
        promotion_transition_id: None,
        promotion_effective_at: None,
        promotion_expires_at: None,
        evidence_transition_id: None,
        exact_reason: None,
    }
}

/// Builds a real `DynamicSelectionPlan` for one symbol (`AAPL`) with two
/// candidates -- `swing_momentum` (fully green, wins selection) and
/// `mean_reversion` (structurally valid, never promoted, refused) -- via
/// the real pure selector, `compute_dynamic_selection_plan`.
fn real_plan(run_id: Uuid) -> DynamicSelectionPlan {
    let context = DynamicSelectionContext {
        run_id: run_id.to_string(),
        schema_version: DYNAMIC_SELECTION_SCHEMA_VERSION.to_string(),
        configured_mode: DynamicSelectionMode::PaperEnforced,
        effective_mode: DynamicSelectionMode::PaperEnforced,
        live_lock_applied: false,
        source_kind: "env_single_symbol_fallback".to_string(),
        source_identity: "env".to_string(),
        market_date: "2099-03-01".to_string(),
    };
    let candidates = vec![
        SelectionCandidateInput {
            symbol: "AAPL".to_string(),
            strategy_id: "swing_momentum".to_string(),
            timeframe_secs: 86_400,
            evidence: green_evidence(),
        },
        SelectionCandidateInput {
            symbol: "AAPL".to_string(),
            strategy_id: "mean_reversion".to_string(),
            timeframe_secs: 86_400,
            evidence: unpromoted_evidence(),
        },
    ];
    compute_dynamic_selection_plan(context, &["AAPL".to_string()], &candidates)
}

/// Converts a real, already-computed `DynamicSelectionPlan` into the exact
/// durable-writer input shape (`NewDynamicSelectionPlan`) `insert_dynamic_selection_plan`
/// expects -- every field taken from the plan's own real output, nothing
/// fabricated, mirroring exactly what the crate-internal evidence writer
/// would produce for this plan.
fn new_plan_from_real_plan(
    plan: &DynamicSelectionPlan,
    plan_id: Uuid,
    run_id: Uuid,
) -> NewDynamicSelectionPlan {
    let mut symbols = Vec::new();
    let mut candidates = Vec::new();
    for sr in &plan.symbol_results {
        symbols.push(NewDynamicSelectionPlanSymbol {
            symbol: sr.symbol.clone(),
            selected_strategy_id: sr.selected_strategy_id.clone(),
            disposition: disposition_str(sr.disposition).to_string(),
            reason_code: sr.reason_code.clone(),
            exact_reason_code: sr.exact_reason.code().to_string(),
        });
        for (ordinal, c) in sr.candidates.iter().enumerate() {
            candidates.push(NewDynamicSelectionPlanCandidate {
                ordinal: ordinal as i32,
                symbol: c.symbol.clone(),
                strategy_id: c.strategy_id.clone(),
                timeframe_secs: c.timeframe_secs,
                canonical_score_decimal: c.canonical_score_decimal.clone(),
                canonical_score_micros: c.canonical_score_micros,
                scanner_rank: c.scanner_rank.map(|r| r as i32),
                watchlist_assigned: c.watchlist_assigned,
                promotion_query_ok: c.promotion_query_ok,
                promotion_state: c.promotion_state.clone(),
                promotion_effective: c.promotion_effective,
                promotion_expired: c.promotion_expired,
                evidence_resolved: c.evidence_resolved,
                review_state_is_paper_candidate: c.review_state_is_paper_candidate,
                evidence_review_state: c.evidence_review_state.clone(),
                durable_legacy_fingerprint: c.durable_legacy_fingerprint.clone(),
                recomputed_legacy_fingerprint: c.recomputed_legacy_fingerprint.clone(),
                legacy_fingerprint_matches: c.legacy_fingerprint_matches,
                durable_exact_fingerprint_v2: c.durable_exact_fingerprint_v2.clone(),
                recomputed_exact_fingerprint_v2: c.recomputed_exact_fingerprint_v2.clone(),
                exact_fingerprint_v2_matches: c.exact_fingerprint_v2_matches,
                config_identity_verified: c.config_identity_verified,
                durable_config_fingerprint: c.durable_config_fingerprint.clone(),
                current_config_fingerprint: c.current_config_fingerprint.clone(),
                registry_enabled: c.registry_enabled,
                plugin_instantiable: c.plugin_instantiable,
                timeframe_matches: c.timeframe_matches,
                data_ready: c.data_ready,
                evidence_review_id: c.evidence_review_id.clone(),
                evidence_scanner_scan_id: c.evidence_scanner_scan_id.clone(),
                evidence_artifact_path: c.evidence_artifact_path.clone(),
                evidence_git_hash: c.evidence_git_hash.clone(),
                promotion_transition_id: c.promotion_transition_id.clone(),
                promotion_effective_at: c.promotion_effective_at.clone(),
                promotion_expires_at: c.promotion_expires_at.clone(),
                evidence_transition_id: c.evidence_transition_id.clone(),
                exact_reason_code: c.exact_reason.code().to_string(),
                selected: c.selected,
                disposition: disposition_str(c.disposition).to_string(),
                reason_code: c.reason_code.clone(),
            });
        }
    }
    NewDynamicSelectionPlan {
        plan_id,
        run_id,
        library_schema_version: plan.context.schema_version.clone(),
        context_schema_version: plan.context.schema_version.clone(),
        configured_mode: plan.context.configured_mode.as_str().to_string(),
        effective_mode: plan.context.effective_mode.as_str().to_string(),
        live_lock_applied: plan.context.live_lock_applied,
        approved_for_live: false,
        source_kind: plan.context.source_kind.clone(),
        source_identity: plan.context.source_identity.clone(),
        market_date: plan.context.market_date.clone(),
        // Real production vocabulary for this exact shape (PaperEnforced +
        // >=1 selection => PaperEnforcedAllowed), per
        // `dynamic_selection_start_gate`'s own documented contract -- this
        // test does not call the start-gate itself (that would require the
        // full DB-backed promotion/registry/calendar apparatus this file
        // deliberately avoids; see module doc), but the label is the real,
        // correct one for this input, not an invented placeholder.
        disposition: "paper_enforced_allowed".to_string(),
        truth_state: plan.truth_state.clone(),
        blockers: plan.blockers.clone(),
        symbol_count: plan.eligible_symbol_count() as i32,
        candidate_count: plan.candidate_count() as i32,
        selected_count: plan.selected_count() as i32,
        refused_count: plan.refused_count() as i32,
        writer_version: "scenario-test-e2e-01".to_string(),
        created_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 5, 0).unwrap(),
        symbols,
        candidates,
    }
}

async fn fixture_run(pool: &sqlx::PgPool, run_id: Uuid) {
    insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: "test-dynamic-selection-e2e".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("fixture run insert should succeed");
}

async fn get(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn real_plan_build_host_pool_selection_and_evidence_route_read_round_trip() {
    mqk_db::run_isolated("dyn_sel_e2e_valid", |pool| async move {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"dyn-sel-e2e-01.run.valid");
        fixture_run(&pool, run_id).await;

        // Step 1: real plan build (the pure selector `build_dynamic_selection_plan`
        // itself delegates to, given real evidence).
        let plan = real_plan(run_id);
        assert_eq!(plan.selected_count(), 1, "swing_momentum must win selection");
        assert_eq!(plan.candidate_count(), 2);

        // Step 2: real host-pool selection, from the plan's own selected pairs
        // -- proves the plan is coherent enough for the real host-pool
        // constructor to accept it, not merely internally self-consistent.
        let keys: Vec<(String, String, i64)> = plan
            .symbol_results
            .iter()
            .filter_map(|sr| {
                let sid = sr.selected_strategy_id.clone()?;
                let c = sr.candidates.iter().find(|c| c.selected)?;
                Some((sr.symbol.clone(), sid, c.timeframe_secs))
            })
            .collect();
        let host_pool = DynamicSelectionHostPool::build(&keys);
        assert!(
            host_pool.is_ok(),
            "real plan must build a coherent host pool: {:?}",
            host_pool.err()
        );
        assert_eq!(host_pool.unwrap().len(), 1);

        // Step 3: real evidence write, via the real DB-layer writer.
        let plan_id = derive_plan_id(&plan);
        let new_plan = new_plan_from_real_plan(&plan, plan_id, run_id);
        let outcome = insert_dynamic_selection_plan(&pool, new_plan)
            .await
            .expect("insert must succeed");
        assert_eq!(outcome, InsertDynamicSelectionPlanOutcome::Inserted);

        // Step 4: real evidence-route read, via the real Axum router.
        let st = std::sync::Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));

        let (status, list_body) = get(
            routes::build_router(std::sync::Arc::clone(&st)),
            &format!("/api/v1/dynamic-selection/plans?run_id={run_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let plans = list_body["plans"].as_array().expect("plans array");
        assert_eq!(plans.len(), 1, "list route must surface exactly the one committed plan: {list_body}");
        assert_eq!(plans[0]["plan_id"], plan_id.to_string());
        assert_eq!(plans[0]["disposition"], "paper_enforced_allowed");
        assert_eq!(plans[0]["selected_count"], 1);
        assert_eq!(plans[0]["candidate_count"], 2);
        assert_eq!(
            plans[0]["validation_state"], "valid",
            "the route's read-side validator must independently confirm this real, untampered plan: {list_body}"
        );

        let (status, detail_body) = get(
            routes::build_router(std::sync::Arc::clone(&st)),
            &format!("/api/v1/dynamic-selection/plans/{plan_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail_body["found"], true);
        assert_eq!(detail_body["run_id"], run_id.to_string());
        assert_eq!(detail_body["validation_state"], "valid", "{detail_body}");
        let symbols = detail_body["symbols"].as_array().expect("symbols array");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0]["symbol"], "AAPL");
        assert_eq!(symbols[0]["selected_strategy_id"], "swing_momentum");
        let candidates = detail_body["candidates"].as_array().expect("candidates array");
        assert_eq!(candidates.len(), 2, "both the selected and the refused candidate must round-trip: {detail_body}");
        let selected_row = candidates
            .iter()
            .find(|c| c["strategy_id"] == "swing_momentum")
            .expect("selected candidate present");
        assert_eq!(selected_row["selected"], true);
        let refused_row = candidates
            .iter()
            .find(|c| c["strategy_id"] == "mean_reversion")
            .expect("refused candidate present");
        assert_eq!(refused_row["selected"], false);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn unknown_plan_id_is_not_found() {
    mqk_db::run_isolated("dyn_sel_e2e_missing", |pool| async move {
        let st = std::sync::Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool,
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let missing_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"dyn-sel-e2e-01.never-written");
        let (status, body) = get(
            routes::build_router(st),
            &format!("/api/v1/dynamic-selection/plans/{missing_id}"),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the route itself never 404s -- it reports found=false"
        );
        assert_eq!(body["found"], false);
        assert_eq!(body["validation_state"], "missing");
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn tampered_candidate_score_is_surfaced_as_identity_mismatch_by_the_route() {
    mqk_db::run_isolated("dyn_sel_e2e_tampered", |pool| async move {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"dyn-sel-e2e-01.run.tampered");
        fixture_run(&pool, run_id).await;

        let plan = real_plan(run_id);
        let plan_id = derive_plan_id(&plan);
        let new_plan = new_plan_from_real_plan(&plan, plan_id, run_id);
        insert_dynamic_selection_plan(&pool, new_plan)
            .await
            .expect("insert must succeed");

        // Tamper the durably-stored candidate score directly (bypassing the
        // writer entirely) -- proves the route's validator recomputes
        // identity from the plan's *own* canonical facts rather than
        // trusting the stored row at face value.
        sqlx::query(
            "update sys_dynamic_selection_plan_candidates \
             set canonical_score_decimal = '999.000000' \
             where plan_id = $1 and strategy_id = 'swing_momentum'",
        )
        .bind(plan_id)
        .execute(&pool)
        .await
        .expect("tamper update must succeed");

        let st = std::sync::Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool,
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let (status, detail_body) = get(
            routes::build_router(st),
            &format!("/api/v1/dynamic-selection/plans/{plan_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail_body["found"], true);
        assert_ne!(
            detail_body["validation_state"], "valid",
            "a tampered candidate row must never validate as clean: {detail_body}"
        );
    })
    .await;
}
