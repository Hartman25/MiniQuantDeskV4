//! MULTI-STRATEGY-CONFLICT-POLICY-01 Phase D: read-only conflict-policy API proof.
//!
//! Proves GET /api/v1/strategy/conflict/{status,plans,plans/:plan_id} never
//! fabricate a value, distinguish db_unavailable/query_failed/not_found/
//! active, reject malformed input with a bounded message that never echoes
//! the raw caller-supplied value, and never mutate any row.
//!
//! In-process (no-DB) proofs run always; DB-backed proofs require
//! `MQK_DATABASE_URL` and are `#[ignore]`, run with `--include-ignored
//! --test-threads=1`.
//!
//! No broker/provider/network call anywhere in this file. No order
//! submitted, cancelled, or replaced.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).expect("body is not valid JSON")
    };
    (status, json)
}

fn get(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn post(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn make_router_no_db() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

fn router_with_pool(pool: sqlx::PgPool) -> axum::Router {
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

async fn test_pool() -> Option<sqlx::PgPool> {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return None;
    }
    match mqk_db::testkit_db_pool().await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn fixed_run_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.runtime-strategy-conflict-api.v1|{seed}").as_bytes(),
    )
}

fn fixed_plan_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.runtime-strategy-conflict-api.plan.v1|{seed}").as_bytes(),
    )
}

async fn cleanup(pool: &sqlx::PgPool, run_id: Uuid) {
    let _ = sqlx::query(
        "delete from sys_runtime_strategy_conflict_candidates where plan_id in \
         (select plan_id from sys_runtime_strategy_conflict_plans where run_id = $1)",
    )
    .bind(run_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("delete from sys_runtime_strategy_conflict_plans where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

async fn seed_run(pool: &sqlx::PgPool, run_id: Uuid) {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert_run failed");
}

/// Deterministic per-`seed` bar timestamp so multiple `seed_plan` calls
/// under the same `run_id` (e.g. two plans in one list-scoping test) mint
/// genuinely distinct, recompute-consistent plans rather than colliding on
/// an identical economic candidate.
fn seed_variant_bar_end_ts(seed: &str) -> i64 {
    900 + seed.bytes().map(i64::from).sum::<i64>()
}

/// Seeds one fully self-consistent (FINAL-IDENTITY-AND-READ-AUTHORITY-
/// REPAIR-01 Defect 3/4) conflict plan -- a lone valid sell candidate,
/// which the real pure resolver always treats as a single-candidate-group
/// passthrough (`disposition=passthrough`,
/// `reason_code=single_candidate_passthrough`) regardless of side. Mints
/// `plan_id`/`cycle_id` via the exact same shared identity function the
/// runtime and read-side validator use, so this fixture is never silently
/// invalidated by the validator's own cycle-id recomputation. Returns the
/// computed `plan_id` for the caller to use in subsequent lookups.
async fn seed_plan(pool: &sqlx::PgPool, run_id: Uuid, seed: &str) -> Uuid {
    let bar_end_ts = seed_variant_bar_end_ts(seed);
    let input = mqk_portfolio::ConflictCandidateInput {
        ordinal: 0,
        symbol: "AAPL".to_string(),
        strategy_id: "strategy_a".to_string(),
        timeframe_secs: 300,
        side: "sell".to_string(),
        qty: 5,
        current_qty: 20,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
        bar_symbol: Some("AAPL".to_string()),
        bar_strategy_id: Some("strategy_a".to_string()),
        bar_timeframe: Some("5m".to_string()),
        bar_end_ts: Some(bar_end_ts),
        close_micros: Some(0),
    };
    let plan_id_str = mqk_daemon::runtime_strategy_conflict::compute_conflict_cycle_id(
        run_id,
        "2099-02-01",
        mqk_daemon::runtime_strategy_conflict_mode::ConflictPolicyMode::Shadow,
        mqk_daemon::runtime_strategy_conflict_mode::ConflictPolicyMode::Shadow,
        &[input],
    );
    let plan_id: Uuid = plan_id_str
        .parse()
        .expect("compute_conflict_cycle_id returns a valid uuid");

    mqk_db::insert_runtime_strategy_conflict_plan(
        pool,
        mqk_db::NewRuntimeStrategyConflictPlan {
            plan_id,
            cycle_id: plan_id,
            run_id,
            mode: "shadow".to_string(),
            configured_mode: "shadow".to_string(),
            market_date: "2099-02-01".to_string(),
            policy_schema_version: mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION.to_string(),
            symbol_group_count: 1,
            candidate_count: 1,
            selected_count: 1,
            refused_count: 0,
            truth_state: "computed".to_string(),
            blockers: vec![],
            created_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 12, 5, 0).unwrap(),
            candidates: vec![mqk_db::NewRuntimeStrategyConflictCandidate {
                ordinal: 0,
                symbol: "AAPL".to_string(),
                strategy_id: "strategy_a".to_string(),
                timeframe_secs: 300,
                side: "sell".to_string(),
                qty: 5,
                current_qty: 20,
                order_type: "market".to_string(),
                time_in_force: "day".to_string(),
                limit_price: None,
                proposed_target_qty: Some(15),
                bar_present: true,
                bar_symbol: Some("AAPL".to_string()),
                bar_strategy_id: Some("strategy_a".to_string()),
                bar_timeframe: Some("5m".to_string()),
                bar_end_ts: Some(bar_end_ts),
                close_micros: Some(0),
                selected: true,
                disposition: "passthrough".to_string(),
                reason_code: "single_candidate_passthrough".to_string(),
            }],
        },
    )
    .await
    .expect("seed_plan insert should succeed");

    plan_id
}

// ---------------------------------------------------------------------------
// No-DB proofs (always run)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_reports_db_unavailable_without_pool() {
    let router = make_router_no_db();
    let (status, body) = call(router, get("/api/v1/strategy/conflict/status")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "db_unavailable");
    assert_eq!(body["approved_for_live"], false);
    assert_eq!(body["mode_effective"], "off");
}

#[tokio::test]
async fn plans_list_reports_db_unavailable_without_pool() {
    let router = make_router_no_db();
    let (status, body) = call(router, get("/api/v1/strategy/conflict/plans")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "db_unavailable");
    assert_eq!(body["plans"], serde_json::json!([]));
}

#[tokio::test]
async fn plan_by_id_reports_db_unavailable_without_pool() {
    let router = make_router_no_db();
    let plan_id = Uuid::new_v4();
    let (status, body) = call(
        router,
        get(&format!("/api/v1/strategy/conflict/plans/{plan_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "db_unavailable");
    assert_eq!(body["plan"], serde_json::Value::Null);
}

#[tokio::test]
async fn invalid_plan_id_is_bounded_and_does_not_echo_input() {
    let router = make_router_no_db();
    let suspicious = "not-a-uuid-injected-marker-xyz123";
    let (status, body) = call(
        router,
        get(&format!("/api/v1/strategy/conflict/plans/{suspicious}")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        !detail.contains("injected-marker-xyz123"),
        "detail must not echo raw input: {detail}"
    );
    assert!(detail.contains("plan_id"));
}

#[tokio::test]
async fn invalid_run_id_query_param_is_bounded() {
    let router = make_router_no_db();
    let (status, body) = call(
        router,
        get("/api/v1/strategy/conflict/status?run_id=not-a-uuid"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(!detail.contains("not-a-uuid"));
}

#[tokio::test]
async fn mutation_methods_are_rejected() {
    let router = make_router_no_db();
    let (status, _) = call(router, post("/api/v1/strategy/conflict/status")).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn invalid_configuration_env_value_is_surfaced() {
    std::env::set_var("MQK_STRATEGY_CONFLICT_POLICY_MODE", "totally_bogus_value");
    let router = make_router_no_db();
    let (status, body) = call(router, get("/api/v1/strategy/conflict/status")).await;
    std::env::remove_var("MQK_STRATEGY_CONFLICT_POLICY_MODE");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mode_effective"], "off");
    assert_eq!(body["invalid_configuration"], "totally_bogus_value");
}

#[tokio::test]
async fn default_off_mode_reports_truthfully() {
    std::env::remove_var("MQK_STRATEGY_CONFLICT_POLICY_MODE");
    let router = make_router_no_db();
    let (_, body) = call(router, get("/api/v1/strategy/conflict/status")).await;
    assert_eq!(body["mode_configured"], "off");
    assert_eq!(body["mode_effective"], "off");
    assert_eq!(body["approved_for_live"], false);
    assert!(body["current_runtime_limitation"]
        .as_str()
        .unwrap_or_default()
        .contains("one strategy host"));
}

/// Seeds a plan whose sole candidate carries a `reason_code` outside the
/// closed vocabulary `mqk_portfolio::conflict_policy` can ever produce --
/// evidence corruption the DB's own CHECK constraints do not (and cannot,
/// short of duplicating the whole closed reason vocabulary as a CHECK)
/// catch, but the shared read-side validator (Defect 6) must.
async fn seed_malformed_plan(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    plan_id: Uuid,
    created_at_utc: chrono::DateTime<Utc>,
) {
    mqk_db::insert_runtime_strategy_conflict_plan(
        pool,
        mqk_db::NewRuntimeStrategyConflictPlan {
            plan_id,
            cycle_id: plan_id,
            run_id,
            mode: "shadow".to_string(),
            configured_mode: "shadow".to_string(),
            market_date: "2099-02-01".to_string(),
            policy_schema_version: mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION.to_string(),
            symbol_group_count: 1,
            candidate_count: 1,
            selected_count: 1,
            refused_count: 0,
            truth_state: "computed".to_string(),
            blockers: vec![],
            created_at_utc,
            candidates: vec![mqk_db::NewRuntimeStrategyConflictCandidate {
                ordinal: 0,
                symbol: "AAPL".to_string(),
                strategy_id: "strategy_a".to_string(),
                timeframe_secs: 300,
                side: "buy".to_string(),
                qty: 10,
                current_qty: 0,
                order_type: "market".to_string(),
                time_in_force: "day".to_string(),
                limit_price: None,
                proposed_target_qty: Some(10),
                bar_present: true,
                bar_symbol: Some("AAPL".to_string()),
                bar_strategy_id: Some("strategy_a".to_string()),
                bar_timeframe: Some("5m".to_string()),
                bar_end_ts: Some(1_000),
                close_micros: Some(100_000_000),
                selected: true,
                // Not a member of the closed reason-code vocabulary --
                // the DB accepts free text here, so only the shared
                // read-side validator can catch this.
                disposition: "selected".to_string(),
                reason_code: "totally_made_up_reason_not_in_vocabulary".to_string(),
            }],
        },
    )
    .await
    .expect("seed_malformed_plan insert should succeed");
}

// ---------------------------------------------------------------------------
// DB-backed proofs
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn status_surfaces_invalid_evidence_and_never_falls_back_to_older_plan() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("invalid_evidence_status");
    cleanup(&pool, run_id).await;
    seed_run(&pool, run_id).await;

    let older_valid_plan_id = fixed_plan_id("invalid_evidence_status_older");
    mqk_db::insert_runtime_strategy_conflict_plan(
        &pool,
        mqk_db::NewRuntimeStrategyConflictPlan {
            plan_id: older_valid_plan_id,
            cycle_id: older_valid_plan_id,
            run_id,
            mode: "shadow".to_string(),
            configured_mode: "shadow".to_string(),
            market_date: "2099-02-01".to_string(),
            policy_schema_version: mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION.to_string(),
            symbol_group_count: 1,
            candidate_count: 1,
            selected_count: 1,
            refused_count: 0,
            truth_state: "computed".to_string(),
            blockers: vec![],
            created_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 12, 0, 0).unwrap(),
            candidates: vec![mqk_db::NewRuntimeStrategyConflictCandidate {
                ordinal: 0,
                symbol: "MSFT".to_string(),
                strategy_id: "strategy_a".to_string(),
                timeframe_secs: 300,
                side: "sell".to_string(),
                qty: 5,
                current_qty: 20,
                order_type: "market".to_string(),
                time_in_force: "day".to_string(),
                limit_price: None,
                proposed_target_qty: Some(15),
                bar_present: true,
                bar_symbol: Some("MSFT".to_string()),
                bar_strategy_id: Some("strategy_a".to_string()),
                bar_timeframe: Some("5m".to_string()),
                bar_end_ts: Some(900),
                close_micros: Some(0),
                selected: true,
                disposition: "selected".to_string(),
                reason_code: "risk_reducing_candidate_selected".to_string(),
            }],
        },
    )
    .await
    .expect("older valid plan insert should succeed");

    let malformed_plan_id = fixed_plan_id("invalid_evidence_status_newer");
    seed_malformed_plan(
        &pool,
        run_id,
        malformed_plan_id,
        Utc.with_ymd_and_hms(2099, 2, 1, 12, 10, 0).unwrap(),
    )
    .await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!("/api/v1/strategy/conflict/status?run_id={run_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "invalid_evidence");
    assert_eq!(
        body["latest_plan_id"],
        serde_json::Value::Null,
        "must never fall back to the older valid plan"
    );
    assert!(!body["evidence_blockers"]
        .as_array()
        .expect("evidence_blockers must be an array")
        .is_empty());

    cleanup(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn plan_by_id_returns_invalid_evidence_for_a_malformed_plan() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("invalid_evidence_detail");
    cleanup(&pool, run_id).await;
    seed_run(&pool, run_id).await;
    let plan_id = fixed_plan_id("invalid_evidence_detail");
    seed_malformed_plan(
        &pool,
        run_id,
        plan_id,
        Utc.with_ymd_and_hms(2099, 2, 1, 12, 0, 0).unwrap(),
    )
    .await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!("/api/v1/strategy/conflict/plans/{plan_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "invalid_evidence");
    assert_eq!(body["plan"], serde_json::Value::Null);
    assert_eq!(body["candidates"], serde_json::json!([]));
    assert!(!body["evidence_blockers"]
        .as_array()
        .expect("evidence_blockers must be an array")
        .is_empty());

    cleanup(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn plans_list_excludes_malformed_rows_and_reports_the_count() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("invalid_evidence_list");
    cleanup(&pool, run_id).await;
    seed_run(&pool, run_id).await;
    seed_plan(&pool, run_id, "invalid_evidence_list_valid").await;
    seed_malformed_plan(
        &pool,
        run_id,
        fixed_plan_id("invalid_evidence_list_malformed"),
        Utc.with_ymd_and_hms(2099, 2, 1, 12, 5, 0).unwrap(),
    )
    .await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!("/api/v1/strategy/conflict/plans?run_id={run_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active");
    assert_eq!(
        body["plans"].as_array().unwrap().len(),
        1,
        "the malformed row must never be silently mixed into the active list"
    );
    assert_eq!(body["excluded_malformed_count"], 1);
    assert_eq!(
        body["excluded_vanished_count"], 0,
        "a malformed-but-present row must never be miscounted as vanished"
    );

    cleanup(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn plan_by_id_returns_seeded_plan_with_candidates() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("plan_by_id");
    cleanup(&pool, run_id).await;
    seed_run(&pool, run_id).await;
    let plan_id = seed_plan(&pool, run_id, "plan_by_id").await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!("/api/v1/strategy/conflict/plans/{plan_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["plan"]["plan_id"], plan_id.to_string());
    assert_eq!(body["plan"]["approved_for_live"], false);
    assert_eq!(body["candidates"][0]["symbol"], "AAPL");
    assert_eq!(body["candidates"][0]["selected"], true);
    assert_eq!(
        body["candidates"][0]["reason_code"],
        "single_candidate_passthrough"
    );
    // Defect 7: full 0057 provenance must be exposed read-only, not just
    // bar_end_ts.
    assert_eq!(body["candidates"][0]["order_type"], "market");
    assert_eq!(body["candidates"][0]["time_in_force"], "day");
    assert_eq!(
        body["candidates"][0]["limit_price"],
        serde_json::Value::Null
    );
    assert_eq!(body["candidates"][0]["bar_present"], true);
    assert_eq!(body["candidates"][0]["bar_symbol"], "AAPL");
    assert_eq!(body["candidates"][0]["bar_strategy_id"], "strategy_a");
    assert_eq!(body["candidates"][0]["bar_timeframe"], "5m");
    assert_eq!(body["candidates"][0]["close_micros"], 0);

    cleanup(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn plan_by_id_unknown_uuid_is_not_found_distinct_from_query_failed() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let router = router_with_pool(pool.clone());
    let unknown = Uuid::new_v4();
    let (status, body) = call(
        router,
        get(&format!("/api/v1/strategy/conflict/plans/{unknown}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "not_found");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn status_surfaces_latest_plan_for_resolved_run() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("status_latest");
    cleanup(&pool, run_id).await;
    seed_run(&pool, run_id).await;
    let plan_id = seed_plan(&pool, run_id, "status_latest").await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!("/api/v1/strategy/conflict/status?run_id={run_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["latest_plan_id"], plan_id.to_string());
    assert_eq!(body["latest_plan_candidate_count"], 1);
    assert_eq!(body["latest_plan_selected_count"], 1);

    cleanup(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn plans_list_respects_limit_and_run_scoping() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("plans_list");
    cleanup(&pool, run_id).await;
    seed_run(&pool, run_id).await;
    seed_plan(&pool, run_id, "plans_list_a").await;
    seed_plan(&pool, run_id, "plans_list_b").await;

    let router = router_with_pool(pool.clone());
    let (status, body) = call(
        router,
        get(&format!(
            "/api/v1/strategy/conflict/plans?run_id={run_id}&limit=1"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["plans"].as_array().unwrap().len(), 1);

    cleanup(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn repeated_gets_perform_zero_writes() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("zero_writes");
    cleanup(&pool, run_id).await;
    seed_run(&pool, run_id).await;
    let plan_id = seed_plan(&pool, run_id, "zero_writes").await;

    let count_before: i64 = sqlx::query_scalar(
        "select count(*) from sys_runtime_strategy_conflict_plans where run_id = $1",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    for _ in 0..5 {
        let router = router_with_pool(pool.clone());
        let _ = call(
            router,
            get(&format!("/api/v1/strategy/conflict/plans/{plan_id}")),
        )
        .await;
        let router = router_with_pool(pool.clone());
        let _ = call(
            router,
            get(&format!("/api/v1/strategy/conflict/status?run_id={run_id}")),
        )
        .await;
        let router = router_with_pool(pool.clone());
        let _ = call(
            router,
            get(&format!("/api/v1/strategy/conflict/plans?run_id={run_id}")),
        )
        .await;
    }

    let count_after: i64 = sqlx::query_scalar(
        "select count(*) from sys_runtime_strategy_conflict_plans where run_id = $1",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        count_before, count_after,
        "GET routes must perform zero writes"
    );

    cleanup(&pool, run_id).await;
}
