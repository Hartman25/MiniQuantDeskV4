//! STRATEGY-PROMOTION-REGISTRY-01F: bounded end-to-end closure proof.
//!
//! One consolidated proof, run against the isolated local test DB, that
//! walks the full operator-facing lifecycle through the real daemon router
//! (in-process `axum`/`tower` requests via `.oneshot()` — no real daemon
//! process is started, no broker/provider/network call is made anywhere in
//! this file):
//!
//! 1. Register a test strategy in `sys_strategy_registry`.
//! 2. Write a committed-shape `paper_candidate` review-artifact fixture
//!    (same `write_review_artifacts` function the CLI uses).
//! 3. Transition no-state -> shadow_approved -> paper_approved ->
//!    active_paper, each via the real `POST /api/v1/strategy/promotions/
//!    transition` route (evidence independently validated by the route,
//!    never trusted from the request).
//! 4. Prove current-state and history readback (`GET .../check`,
//!    `GET .../history`) after every transition.
//! 5. Prove no paper outbox row can be created before `active_paper`.
//! 6. Prove exactly one synthetic outbox row is created once
//!    `active_paper` is reached and every other existing gate
//!    (registered+enabled, armed, active run) is satisfied.
//! 7. Transition `active_paper -> demoted`.
//! 8. Prove a new decision (different `decision_id`) is refused after
//!    demotion.
//! 9. Clean up only this test's own rows.
//!
//! Requires `MQK_DATABASE_URL` and is marked `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_strategy_promotion_closure_proof_01f \
//!     -- --include-ignored --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_backtest::{
    write_review_artifacts, ReviewManifest, ReviewRunOutput, ReviewSummary,
    StrategyScanReviewDecision, StrategyScanReviewState,
};
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    routes, state,
};
use tower::ServiceExt;
use uuid::Uuid;

const SYMBOL: &str = "AAPL";
const TIMEFRAME_SECS: i64 = 86400;
const TRANSITION_ROUTE: &str = "/api/v1/strategy/promotions/transition";

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_strategy_promotion_closure_proof_01f \
             -- --include-ignored --nocapture"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    pool
}

/// Writes a committed-shape `paper_candidate` review artifact fixture via
/// the exact same `write_review_artifacts` function the CLI's
/// `mqk backtest review-scan` command calls — this is a real artifact
/// directory, not a hand-rolled JSON blob.
fn write_paper_candidate_fixture(out_dir: &std::path::Path, strategy_id: &str) -> PathBuf {
    let decision = StrategyScanReviewDecision {
        symbol: SYMBOL.to_string(),
        timeframe: "1D".to_string(),
        strategy_id: strategy_id.to_string(),
        scanner_rank: Some(1),
        scanner_score: Some(9.5),
        review_state: StrategyScanReviewState::PaperCandidate,
        reason_codes: vec!["eligible_paper_candidate".to_string()],
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    let review_id = Uuid::new_v4();
    let manifest = ReviewManifest {
        schema_version: 1,
        review_id: review_id.to_string(),
        scanner_scan_id: "closure-proof-scan".to_string(),
        source_artifact_dir: "fixture-source-not-on-disk".to_string(),
        created_at_utc: Utc::now().to_rfc3339(),
        git_hash: "closure-proof-fixture".to_string(),
        policy_min_bars_used: 252,
        policy_min_trade_count: 5,
        policy_min_total_return_pct: 0.0,
        policy_min_alpha_pct: 0.0,
        policy_max_drawdown_pct: 25.0,
        policy_min_profit_factor: 1.05,
        candidate_count: 1,
        blocked_count: 0,
        needs_review_count: 0,
        watchlist_candidate_count: 0,
        paper_candidate_count: 1,
        rejected_count: 0,
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    let summary = ReviewSummary {
        scanner_scan_id: "closure-proof-scan".to_string(),
        review_id: review_id.to_string(),
        candidate_count: 1,
        blocked_count: 0,
        needs_review_count: 0,
        watchlist_candidate_count: 0,
        paper_candidate_count: 1,
        rejected_count: 0,
        top_paper_candidates: vec![decision.clone()],
        top_watchlist_candidates: Vec::new(),
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    let output = ReviewRunOutput {
        review_id,
        manifest,
        decisions: vec![decision],
        summary,
    };
    write_review_artifacts(out_dir, &output).expect("write closure-proof fixture")
}

/// PROMOTION-WALKFORWARD-GATE-WIRING-01: a fully self-consistent,
/// registry-anchored P7C evidence bundle for `strategy_id`, written under
/// `root`. Mirrors `mqk-promotion/tests/common`'s exact artifact shapes
/// (also duplicated, for the same "no cross-crate test visibility" reason,
/// in `scenario_strategy_promotion_routes_01.rs`).
struct ResearchEvidenceFixture {
    trial_id: String,
    evidence_dir: PathBuf,
    judge_path: PathBuf,
}

fn write_research_evidence_fixture(root: &std::path::Path, seed: &str) -> ResearchEvidenceFixture {
    use sha2::{Digest, Sha256};

    let trial_id = format!("trial_{seed}");
    let experiment_id = format!("exp_{seed}");
    let economic_eval_id = format!("econ_eval_{seed}");

    let evidence_dir = root.join(format!("research_evidence_{seed}"));
    std::fs::create_dir_all(&evidence_dir).expect("create research evidence dir");

    let daily_csv = b"date,net_daily_return\n2021-01-01,0.0010\n2021-01-02,0.0021\n".to_vec();
    let daily_sha = hex::encode(Sha256::digest(&daily_csv));
    std::fs::write(evidence_dir.join("economic_daily_returns.csv"), &daily_csv)
        .expect("write daily returns csv");

    let economic_json = format!(
        r#"{{"protocol":{{"protocol_id":"economic_walk_forward_v1"}},"aggregate":{{"folds_used":3}},"holdout":{{"status":"reserved_not_evaluated"}},"execution_pricing":{{"pricing_model_id":"rust_conservative_bar_range_v1"}},"weight_to_share":{{"weight_to_share_protocol_id":"weight_to_share_v1"}},"outputs":{{"economic_daily_returns_csv":{{"sha256":"{daily_sha}"}}}},"ids":{{"economic_eval_id":"{economic_eval_id}"}},"folds":[{{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}}]}}"#
    );
    let economic_sha = hex::encode(Sha256::digest(economic_json.as_bytes()));
    std::fs::write(evidence_dir.join("economic_walk_forward.json"), &economic_json)
        .expect("write economic artifact");

    let judge_json = format!(
        r#"{{"schema_version":"multiple_testing_judge_v1","protocol":{{"protocol_id":"research_multiple_testing_judge_v1"}},"comparison_scope":{{"experiment_id":"{experiment_id}"}},"judge_status":"evaluated","holdout":{{"status":"reserved_not_evaluated"}},"included_trial_ids":["{trial_id}"],"input_economic_result_ids":["{economic_eval_id}"],"input_artifacts":[{{"trial_id":"{trial_id}","economic_walk_forward_json_sha256":"{economic_sha}","economic_daily_returns_csv_sha256":"{daily_sha}"}}],"dsr_results":[{{"trial_id":"{trial_id}","evaluable":true,"deflated_sharpe_ratio":0.85}}],"pbo_result":{{"status":"evaluated","pbo":0.15}}}}"#
    );
    let judge_sha = hex::encode(Sha256::digest(judge_json.as_bytes()));
    let judge_path = root.join(format!("judge_{seed}.json"));
    std::fs::write(&judge_path, &judge_json).expect("write judge artifact");

    let registry_db_path = root.join("research_registry.sqlite3");
    let conn = rusqlite::Connection::open(&registry_db_path).expect("open registry db");
    conn.execute_batch(
        "
        create table if not exists research_trials (
            trial_id text primary key,
            experiment_id text not null,
            hypothesis_id text not null
        );
        create table if not exists research_attempts (
            attempt_id text primary key,
            trial_id text not null,
            status text not null,
            result_id text
        );
        create table if not exists research_judge_artifacts (
            judge_artifact_sha256 text primary key,
            judge_id text not null,
            experiment_id text not null,
            hypothesis_id text,
            canonical_judge_json text
        );
        ",
    )
    .expect("create registry schema");
    conn.execute(
        "insert into research_trials (trial_id, experiment_id, hypothesis_id) values (?1, ?2, ?3)",
        rusqlite::params![trial_id, experiment_id, format!("hyp_{seed}")],
    )
    .expect("insert research_trials row");
    conn.execute(
        "insert into research_attempts (attempt_id, trial_id, status, result_id) \
         values (?1, ?2, 'succeeded', ?3)",
        rusqlite::params![format!("{trial_id}:att0001"), trial_id, economic_eval_id],
    )
    .expect("insert research_attempts row");
    conn.execute(
        "insert into research_judge_artifacts \
         (judge_artifact_sha256, judge_id, experiment_id, hypothesis_id, canonical_judge_json) \
         values (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            judge_sha,
            format!("judge_{seed}"),
            experiment_id,
            Option::<String>::None,
            judge_json,
        ],
    )
    .expect("insert research_judge_artifacts row");
    drop(conn);

    ResearchEvidenceFixture {
        trial_id,
        evidence_dir,
        judge_path,
    }
}

async fn seed_registry(pool: &sqlx::PgPool, strategy_id: &str) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("Closure Proof Strategy {strategy_id}"),
            enabled: true,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry failed");
}

async fn seed_active_run(st: &Arc<state::AppState>) -> Uuid {
    let pool = st.db.as_ref().expect("db configured");
    let run_id = Uuid::new_v4();
    let now = Utc::now();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({"source": "scenario_strategy_promotion_closure_proof_01f"}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    mqk_db::heartbeat_run(pool, run_id, now)
        .await
        .expect("heartbeat_run");
    st.inject_running_loop_for_test(run_id).await;
    run_id
}

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON body");
    (status, json)
}

fn transition_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(TRANSITION_ROUTE)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get_req(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn make_decision(decision_id: &str, strategy_id: &str) -> InternalStrategyDecision {
    InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: SYMBOL.to_string(),
        timeframe_secs: TIMEFRAME_SECS,
        side: "buy".to_string(),
        qty: 10,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    }
}

async fn outbox_row_count(pool: &sqlx::PgPool, decision_id: &str) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM oms_outbox WHERE idempotency_key = $1")
        .bind(decision_id)
        .fetch_one(pool)
        .await
        .expect("count outbox rows");
    row.0
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn closure_proof_full_lifecycle_through_real_routes() {
    let pool = make_db_pool().await;
    let strategy_id = unique_id("closureproof");
    let root = std::env::temp_dir().join(format!("mqk_closure_proof_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("create fixture root");
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", &root);
    // PROMOTION-WALKFORWARD-GATE-WIRING-01: trusted config for the P7C
    // research-evidence gate (Gate 4c), required in addition to the
    // review-artifact evidence above for the shadow_approved transition
    // below.
    std::env::set_var(
        "MQK_RESEARCH_REGISTRY_DB",
        root.join("research_registry.sqlite3"),
    );
    std::env::set_var("MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT", &root);
    std::env::set_var("MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO", "0.0");
    std::env::set_var("MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING", "1.0");

    // --- Step 1: register the strategy. -----------------------------------
    seed_registry(&pool, &strategy_id).await;

    // --- Step 2: write a real paper_candidate review artifact fixture. ----
    let review_dir = write_paper_candidate_fixture(&root, &strategy_id);
    let research = write_research_evidence_fixture(&root, &strategy_id);

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // --- Step 3a: no-state -> shadow_approved, via the real route. --------
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": strategy_id,
            "symbol": SYMBOL,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "shadow_approved",
            "review_dir": review_dir.to_str().unwrap(),
            "research_trial_id": research.trial_id,
            "research_evidence_dir": research.evidence_dir.to_str().unwrap(),
            "research_judge_artifact_path": research.judge_path.to_str().unwrap(),
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "closure-proof-operator",
            "reason": "closure proof step 1",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "shadow_approved transition must succeed: {json}"
    );
    assert_eq!(json["disposition"], "transitioned");
    println!("[closure-proof] step 1: no-state -> shadow_approved: {json}");

    // Readback: current state + history after step 1.
    let check_uri = format!(
        "/api/v1/strategy/promotions/check?strategy_id={strategy_id}&symbol={SYMBOL}&timeframe_secs={TIMEFRAME_SECS}"
    );
    let (_, check1) = call(routes::build_router(Arc::clone(&st)), get_req(&check_uri)).await;
    assert_eq!(check1["current_state"], "shadow_approved");
    assert_eq!(check1["tradable_paper"], false);
    assert_eq!(check1["tradable_live"], false);
    println!("[closure-proof] readback after step 1: {check1}");

    // --- Step 3b: shadow_approved -> paper_approved. -----------------------
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": strategy_id,
            "symbol": SYMBOL,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "paper_approved",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "closure-proof-operator",
            "reason": "closure proof step 2",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "paper_approved transition must succeed: {json}"
    );
    println!("[closure-proof] step 2: shadow_approved -> paper_approved: {json}");

    // --- Step 4a: no outbox row can be created before active_paper. -------
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");
    let _run_id = seed_active_run(&st).await;

    let pre_active_decision_id = unique_id("dec_pre_active");
    let pre_active_outcome = submit_internal_strategy_decision(
        &st,
        make_decision(&pre_active_decision_id, &strategy_id),
    )
    .await;
    assert!(
        !pre_active_outcome.accepted,
        "paper_approved (not yet active) must never create an outbox row"
    );
    assert_eq!(pre_active_outcome.disposition, "promotion_not_active");
    assert_eq!(outbox_row_count(&pool, &pre_active_decision_id).await, 0);
    println!("[closure-proof] step 3: confirmed no outbox row before active_paper");

    // --- Step 3c: paper_approved -> active_paper. --------------------------
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": strategy_id,
            "symbol": SYMBOL,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "active_paper",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "closure-proof-operator",
            "reason": "closure proof step 4",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "active_paper transition must succeed: {json}"
    );
    println!("[closure-proof] step 4: paper_approved -> active_paper: {json}");

    let (_, check2) = call(routes::build_router(Arc::clone(&st)), get_req(&check_uri)).await;
    assert_eq!(check2["current_state"], "active_paper");
    assert_eq!(check2["tradable_paper"], true);
    assert_eq!(check2["tradable_live"], false);
    println!("[closure-proof] readback after step 4: {check2}");

    let history_uri = format!(
        "/api/v1/strategy/promotions/history?strategy_id={strategy_id}&symbol={SYMBOL}&timeframe_secs={TIMEFRAME_SECS}"
    );
    let (_, history1) = call(routes::build_router(Arc::clone(&st)), get_req(&history_uri)).await;
    let history_rows = history1["rows"].as_array().expect("history rows array");
    assert_eq!(
        history_rows.len(),
        3,
        "history must show all 3 transitions so far"
    );
    println!(
        "[closure-proof] history after step 4 ({} rows): {history1}",
        history_rows.len()
    );

    // --- Step 5: exactly one synthetic outbox row after active_paper. -----
    let active_decision_id = unique_id("dec_active");
    let active_outcome =
        submit_internal_strategy_decision(&st, make_decision(&active_decision_id, &strategy_id))
            .await;
    assert!(
        active_outcome.accepted,
        "active_paper + every other gate satisfied must create an outbox row; disposition={:?} blockers={:?}",
        active_outcome.disposition, active_outcome.blockers
    );
    assert_eq!(outbox_row_count(&pool, &active_decision_id).await, 1);
    println!("[closure-proof] step 5: one synthetic outbox row created for decision_id={active_decision_id}");

    // --- Step 6: active_paper -> demoted. -----------------------------------
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": strategy_id,
            "symbol": SYMBOL,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "demoted",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "closure-proof-operator",
            "reason": "closure proof step 6",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "demoted transition must succeed: {json}"
    );
    println!("[closure-proof] step 6: active_paper -> demoted: {json}");

    // --- Step 7: demotion blocks a NEW decision. ---------------------------
    let post_demote_decision_id = unique_id("dec_post_demote");
    let post_demote_outcome = submit_internal_strategy_decision(
        &st,
        make_decision(&post_demote_decision_id, &strategy_id),
    )
    .await;
    assert!(
        !post_demote_outcome.accepted,
        "demoted must block a new decision"
    );
    assert_eq!(post_demote_outcome.disposition, "promotion_demoted");
    assert_eq!(outbox_row_count(&pool, &post_demote_decision_id).await, 0);
    println!("[closure-proof] step 7: confirmed demotion blocks a new decision");

    // Final history readback: all 4 transitions remain visible.
    let (_, history2) = call(routes::build_router(Arc::clone(&st)), get_req(&history_uri)).await;
    let final_rows = history2["rows"].as_array().expect("history rows array");
    assert_eq!(
        final_rows.len(),
        4,
        "history must show all 4 transitions, newest first"
    );
    assert_eq!(final_rows[0]["new_state"], "demoted");
    println!(
        "[closure-proof] final history ({} rows): {history2}",
        final_rows.len()
    );

    // --- Cleanup: only this test's own rows. --------------------------------
    sqlx::query("DELETE FROM oms_outbox WHERE idempotency_key = $1")
        .bind(&active_decision_id)
        .execute(&pool)
        .await
        .expect("cleanup oms_outbox");
    sqlx::query("DELETE FROM sys_strategy_promotion_transitions WHERE strategy_id = $1")
        .bind(&strategy_id)
        .execute(&pool)
        .await
        .expect("cleanup sys_strategy_promotion_transitions");
    sqlx::query("DELETE FROM sys_strategy_registry WHERE strategy_id = $1")
        .bind(&strategy_id)
        .execute(&pool)
        .await
        .expect("cleanup sys_strategy_registry");
    sqlx::query("DELETE FROM runs WHERE run_id = $1")
        .bind(_run_id)
        .execute(&pool)
        .await
        .expect("cleanup runs");
    let _ = std::fs::remove_dir_all(&root);

    println!(
        "[closure-proof] cleanup complete; no broker/provider/network call was made at any step"
    );
}
