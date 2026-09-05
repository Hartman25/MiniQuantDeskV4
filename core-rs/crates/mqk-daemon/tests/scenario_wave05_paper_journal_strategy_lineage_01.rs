//! WAVE05-PAPER-JOURNAL-STRATEGY-LINEAGE-01
//!
//! Proves that `GET /api/v1/paper/journal`'s fills lane recovers durable
//! strategy identity for each fill from the EXACT originating outbox row
//! (`fill_quality_telemetry.internal_order_id == oms_outbox.idempotency_key`,
//! unique via `uq_outbox_idempotency`) — never by symbol, timestamp
//! proximity, current strategy assignment, or current registry/promotion
//! state.
//!
//! All tests run against a real disposable Postgres database created and
//! torn down per-test via `mqk_db::run_isolated` (migrations applied
//! automatically). No `MQK_DATABASE_URL` / `--include-ignored` required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{DateTime, Duration, Utc};
use http_body_util::BodyExt;
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    routes, state,
};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    (status, body)
}

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

fn get(path: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn fetch_journal(st: &Arc<state::AppState>) -> serde_json::Value {
    let router = routes::build_router(Arc::clone(st));
    let (status, body) = call(router, get("/api/v1/paper/journal")).await;
    assert_eq!(status, StatusCode::OK, "paper/journal must return 200");
    parse_json(body)
}

/// Find the fills_lane row with the given internal_order_id, or panic with
/// the full lane dumped for diagnosis.
fn find_fill_row<'a>(journal: &'a serde_json::Value, internal_order_id: &str) -> &'a serde_json::Value {
    journal["fills_lane"]["rows"]
        .as_array()
        .expect("fills_lane.rows must be an array")
        .iter()
        .find(|r| r["internal_order_id"].as_str() == Some(internal_order_id))
        .unwrap_or_else(|| {
            panic!(
                "no fill row with internal_order_id={internal_order_id}; rows={:?}",
                journal["fills_lane"]["rows"]
            )
        })
}

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

fn fingerprint(byte: char) -> String {
    byte.to_string().repeat(64)
}

// ---------------------------------------------------------------------------
// DB fixtures (duplicated from scenario_internal_strategy_decision.rs's
// #[ignore] fixtures, parameterized by fingerprint and adapted to
// mqk_db::run_isolated instead of MQK_DATABASE_URL)
// ---------------------------------------------------------------------------

async fn seed_registry(pool: &sqlx::PgPool, strategy_id: &str, enabled: bool) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("Test Strategy {strategy_id}"),
            enabled,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry: upsert failed");
}

/// Seed a durable `active_paper` promotion for the exact
/// `(strategy_id, symbol, timeframe_secs)` identity, walking the full legal
/// transition graph, with an explicit `config_fingerprint` so callers can
/// control exactly what the Gate 3b promotion gate will accept.
async fn seed_active_paper_promotion(
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    config_fingerprint: &str,
) {
    let now = Utc::now();
    let seed = |suffix: &str| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!(
                "wave05-promo-seed:{strategy_id}:{symbol}:{timeframe_secs}:{config_fingerprint}:{suffix}"
            )
            .as_bytes(),
        )
    };
    let step = |transition_id: Uuid,
                previous_state: Option<&str>,
                new_state: &str,
                effective_at: chrono::DateTime<Utc>| {
        mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            config_fingerprint: Some(config_fingerprint.to_string()),
            config_identity_status: "verified_v1".to_string(),
            previous_state: previous_state.map(|s| s.to_string()),
            new_state: new_state.to_string(),
            parent_transition_id: None,
            evidence_transition_id: None,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_git_hash: None,
            evidence_artifact_path: None,
            evidence_fingerprint: None,
            evidence_fingerprint_v2: None,
            effective_at_utc: effective_at,
            expires_at_utc: None,
            initiated_by: "test-seed".to_string(),
            reason: "test seed".to_string(),
            created_at_utc: effective_at,
        }
    };
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(seed("1"), None, "shadow_approved", now),
    )
    .await
    .expect("seed shadow_approved");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(
            seed("2"),
            Some("shadow_approved"),
            "paper_approved",
            now + Duration::milliseconds(1),
        ),
    )
    .await
    .expect("seed paper_approved");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(
            seed("3"),
            Some("paper_approved"),
            "active_paper",
            now + Duration::milliseconds(2),
        ),
    )
    .await
    .expect("seed active_paper");
}

/// Walk the full legal re-promotion graph
/// (`active_paper -> demoted -> shadow_approved -> paper_approved ->
/// active_paper`) to land the identity back on `active_paper` with a NEW
/// `config_fingerprint`, simulating a later config change / re-promotion.
/// `sys_strategy_promotion_transitions_legal_graph` forbids `active_paper ->
/// active_paper` directly, so this is the only legal way to change the
/// fingerprint while keeping the identity paper-tradable. Used only to
/// prove the read path never re-derives historical attribution from
/// "current" promotion config.
async fn seed_promotion_config_drift(
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    new_config_fingerprint: &str,
) {
    let now = Utc::now();
    let seed = |suffix: &str| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!(
                "wave05-promo-drift:{strategy_id}:{symbol}:{timeframe_secs}:{new_config_fingerprint}:{suffix}"
            )
            .as_bytes(),
        )
    };
    let step = |transition_id: Uuid,
                previous_state: Option<&str>,
                new_state: &str,
                effective_at: chrono::DateTime<Utc>| {
        mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            config_fingerprint: Some(new_config_fingerprint.to_string()),
            config_identity_status: "verified_v1".to_string(),
            previous_state: previous_state.map(|s| s.to_string()),
            new_state: new_state.to_string(),
            parent_transition_id: None,
            evidence_transition_id: None,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_git_hash: None,
            evidence_artifact_path: None,
            evidence_fingerprint: None,
            evidence_fingerprint_v2: None,
            effective_at_utc: effective_at,
            expires_at_utc: None,
            initiated_by: "test-seed-drift".to_string(),
            reason: "simulate config drift after order placement".to_string(),
            created_at_utc: effective_at,
        }
    };
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(seed("1"), Some("active_paper"), "demoted", now + Duration::milliseconds(10)),
    )
    .await
    .expect("seed drift: demoted");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(
            seed("2"),
            Some("demoted"),
            "shadow_approved",
            now + Duration::milliseconds(11),
        ),
    )
    .await
    .expect("seed drift: shadow_approved");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(
            seed("3"),
            Some("shadow_approved"),
            "paper_approved",
            now + Duration::milliseconds(12),
        ),
    )
    .await
    .expect("seed drift: paper_approved");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(
            seed("4"),
            Some("paper_approved"),
            "active_paper",
            now + Duration::milliseconds(13),
        ),
    )
    .await
    .expect("seed drift: active_paper (new fingerprint)");
}

/// Seed a RUNNING run in the DB and wire up the local loop handle.
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
            config_json: serde_json::json!({"source": "scenario_wave05_paper_journal_strategy_lineage_01"}),
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

fn make_decision(
    decision_id: &str,
    strategy_id: &str,
    strategy_semantic_fingerprint: &str,
) -> InternalStrategyDecision {
    InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: "AAPL".to_string(),
        timeframe_secs: 86400,
        strategy_semantic_fingerprint: strategy_semantic_fingerprint.to_string(),
        side: "buy".to_string(),
        qty: 10,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    }
}

/// Insert a fill_quality_telemetry row whose `internal_order_id` is the
/// exact `decision_id`/`idempotency_key` of the order it fills.
async fn insert_fill_for_order(pool: &sqlx::PgPool, run_id: Uuid, internal_order_id: &str, symbol: &str) {
    let now = Utc::now();
    mqk_db::insert_fill_quality_telemetry(
        pool,
        &mqk_db::NewFillQualityTelemetry {
            telemetry_id: Uuid::new_v4(),
            run_id,
            internal_order_id: internal_order_id.to_string(),
            broker_order_id: Some(format!("broker-{internal_order_id}")),
            broker_fill_id: Some(format!("fill-{internal_order_id}")),
            broker_message_id: unique_id("msg"),
            symbol: symbol.to_string(),
            side: "buy".to_string(),
            ordered_qty: 10,
            fill_qty: 10,
            fill_price_micros: 150_000_000,
            reference_price_micros: None,
            slippage_bps: None,
            submit_ts_utc: Some(now),
            fill_received_at_utc: now,
            submit_to_fill_ms: Some(50),
            fill_kind: "final_fill".to_string(),
            provenance_ref: format!("oms_inbox:{}", unique_id("bmid")),
            created_at_utc: now,
        },
    )
    .await
    .expect("insert_fill_quality_telemetry failed");
}

async fn arm(pool: &sqlx::PgPool) {
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(pool)
        .await
        .expect("cleanup sys_arm_state");
    mqk_db::persist_arm_state(pool, "ARMED", None)
        .await
        .expect("persist arm state");
}

// ---------------------------------------------------------------------------
// Test 1 — internal strategy fill round trip
// ---------------------------------------------------------------------------

/// A durably-persisted internal strategy decision's fingerprint survives the
/// decision -> outbox -> fill -> journal round trip exactly.
#[tokio::test]
async fn wl01_internal_strategy_fill_round_trip() {
    mqk_db::run_isolated("wl01_round_trip", |pool| async move {
        let sid = unique_id("strat_rt");
        let fp_a = fingerprint('a');
        seed_registry(&pool, &sid, true).await;
        seed_active_paper_promotion(&pool, &sid, "AAPL", 86400, &fp_a).await;
        arm(&pool).await;

        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;

        let dec_id = unique_id("dec_rt");
        let out =
            submit_internal_strategy_decision(&st, make_decision(&dec_id, &sid, &fp_a)).await;
        assert!(
            out.accepted,
            "decision must be accepted; disposition={:?} blockers={:?}",
            out.disposition, out.blockers
        );

        insert_fill_for_order(&pool, run_id, &dec_id, "AAPL").await;

        let journal = fetch_journal(&st).await;
        assert_eq!(journal["fills_lane"]["truth_state"], "active");
        let row = find_fill_row(&journal, &dec_id);
        assert_eq!(
            row["strategy_id"].as_str(),
            Some(sid.as_str()),
            "row must carry strategy_id A; row={row}"
        );
        assert_eq!(
            row["strategy_semantic_fingerprint"].as_str(),
            Some(fp_a.as_str()),
            "row must carry fingerprint A; row={row}"
        );
        assert_eq!(row["strategy_attribution_state"], "attributed");
    })
    .await;
}

// ---------------------------------------------------------------------------
// Test 2 — two strategies, same symbol: exact-order attribution, not
// symbol/latest inference
// ---------------------------------------------------------------------------

/// Strategy A and strategy B both trade AAPL with different decisions,
/// order IDs, and fingerprints. A fill exists only for A's order. The
/// journal must attribute that fill to A — never to B, and never by
/// resolving "the" strategy for AAPL or "the latest" strategy order.
#[tokio::test]
async fn wl02_two_strategies_same_symbol_exact_order_attribution() {
    mqk_db::run_isolated("wl02_two_strategies", |pool| async move {
        let sid_a = unique_id("strat_a");
        let sid_b = unique_id("strat_b");
        let fp_a = fingerprint('a');
        let fp_b = fingerprint('b');
        seed_registry(&pool, &sid_a, true).await;
        seed_registry(&pool, &sid_b, true).await;
        seed_active_paper_promotion(&pool, &sid_a, "AAPL", 86400, &fp_a).await;
        seed_active_paper_promotion(&pool, &sid_b, "AAPL", 86400, &fp_b).await;
        arm(&pool).await;

        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;

        let dec_id_a = unique_id("dec_a");
        let dec_id_b = unique_id("dec_b");
        let out_a =
            submit_internal_strategy_decision(&st, make_decision(&dec_id_a, &sid_a, &fp_a)).await;
        assert!(out_a.accepted, "strategy A decision must be accepted: {out_a:?}");
        let out_b =
            submit_internal_strategy_decision(&st, make_decision(&dec_id_b, &sid_b, &fp_b)).await;
        assert!(out_b.accepted, "strategy B decision must be accepted: {out_b:?}");

        // Only A's order fills.
        insert_fill_for_order(&pool, run_id, &dec_id_a, "AAPL").await;

        let journal = fetch_journal(&st).await;
        assert_eq!(journal["fills_lane"]["truth_state"], "active");
        let rows = journal["fills_lane"]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "exactly one fill exists; rows={rows:?}");

        let row = find_fill_row(&journal, &dec_id_a);
        assert_eq!(
            row["strategy_id"].as_str(),
            Some(sid_a.as_str()),
            "fill must be attributed to strategy A only, never B; row={row}"
        );
        assert_eq!(row["strategy_semantic_fingerprint"].as_str(), Some(fp_a.as_str()));
        assert_ne!(
            row["strategy_id"].as_str(),
            Some(sid_b.as_str()),
            "fill must never be attributed to strategy B"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// Test 3 — current-config drift negative control
// ---------------------------------------------------------------------------

/// After an order is placed under fingerprint OLD, the strategy's promotion
/// configuration changes (re-promoted) to fingerprint NEW. The historical
/// fill must still report OLD — current config must never overwrite
/// historical identity.
#[tokio::test]
async fn wl03_current_config_drift_never_rewrites_historical_fingerprint() {
    mqk_db::run_isolated("wl03_config_drift", |pool| async move {
        let sid = unique_id("strat_drift");
        let fp_old = fingerprint('c');
        let fp_new = fingerprint('d');
        seed_registry(&pool, &sid, true).await;
        seed_active_paper_promotion(&pool, &sid, "AAPL", 86400, &fp_old).await;
        arm(&pool).await;

        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let run_id = seed_active_run(&st).await;

        let dec_id = unique_id("dec_drift");
        let out =
            submit_internal_strategy_decision(&st, make_decision(&dec_id, &sid, &fp_old)).await;
        assert!(out.accepted, "decision under OLD fingerprint must be accepted: {out:?}");

        insert_fill_for_order(&pool, run_id, &dec_id, "AAPL").await;

        // Simulate a later re-promotion / config change to a new fingerprint
        // for the SAME (strategy_id, symbol, timeframe) identity.
        seed_promotion_config_drift(&pool, &sid, "AAPL", 86400, &fp_new).await;

        let journal = fetch_journal(&st).await;
        let row = find_fill_row(&journal, &dec_id);
        assert_eq!(
            row["strategy_semantic_fingerprint"].as_str(),
            Some(fp_old.as_str()),
            "historical fill must still report OLD fingerprint after config drift; row={row}"
        );
        assert_ne!(
            row["strategy_semantic_fingerprint"].as_str(),
            Some(fp_new.as_str()),
            "historical fill must never be rewritten to the new current fingerprint"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// Test 4 — legacy row (strategy_id present, no fingerprint)
// ---------------------------------------------------------------------------

/// An older-style outbox row persisted before fingerprint capture (only
/// `strategy_id`, no `strategy_semantic_fingerprint` key at all) must
/// surface strategy_id and a None/unknown fingerprint — never an invented
/// current fingerprint. Bypasses `submit_internal_strategy_decision`
/// entirely to construct a genuinely legacy-shaped `order_json`.
#[tokio::test]
async fn wl04_legacy_row_without_fingerprint_key_reports_unknown() {
    mqk_db::run_isolated("wl04_legacy_row", |pool| async move {
        let sid = unique_id("strat_legacy");
        let run_id = Uuid::new_v4();
        let now = Utc::now();
        mqk_db::insert_run(
            &pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "mqk-daemon".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: now,
                git_hash: "test".to_string(),
                config_hash: "test".to_string(),
                config_json: serde_json::json!({"source": "wl04_legacy_row"}),
                host_fingerprint: "test-host".to_string(),
            },
        )
        .await
        .expect("insert_run");
        mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
        mqk_db::begin_run(&pool, run_id).await.expect("begin_run");

        let dec_id = unique_id("dec_legacy");
        // Deliberately legacy-shaped: no strategy_semantic_fingerprint key
        // at all (predates this patch), unlike build_order_json's current
        // output.
        let legacy_order_json = serde_json::json!({
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
            "order_type": "market",
            "time_in_force": "day",
            "limit_price": serde_json::Value::Null,
            "strategy_id": sid,
            "signal_source": "internal_strategy_decision",
        });
        mqk_db::outbox_enqueue_for_running_run(&pool, run_id, &dec_id, legacy_order_json)
            .await
            .expect("enqueue legacy outbox row");

        insert_fill_for_order(&pool, run_id, &dec_id, "AAPL").await;

        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        st.inject_running_loop_for_test(run_id).await;

        let journal = fetch_journal(&st).await;
        let row = find_fill_row(&journal, &dec_id);
        assert_eq!(row["strategy_id"].as_str(), Some(sid.as_str()));
        assert!(
            row["strategy_semantic_fingerprint"].is_null(),
            "legacy row must report fingerprint as None/unknown, never invented; row={row}"
        );
        assert_eq!(row["strategy_attribution_state"], "attributed");
    })
    .await;
}

// ---------------------------------------------------------------------------
// Test 5 — manual / non-strategy order
// ---------------------------------------------------------------------------

/// A manual operator order (no `strategy_id` key in `order_json` at all,
/// matching the real shape produced by the manual-submit route) plus its
/// fill must report strategy_id=None, fingerprint=None, and be explicitly
/// distinguished from a legacy strategy row via
/// `strategy_attribution_state == "unattributed_manual"`.
#[tokio::test]
async fn wl05_manual_order_reports_unattributed_not_invented() {
    mqk_db::run_isolated("wl05_manual_order", |pool| async move {
        let run_id = Uuid::new_v4();
        let now = Utc::now();
        mqk_db::insert_run(
            &pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "mqk-daemon".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: now,
                git_hash: "test".to_string(),
                config_hash: "test".to_string(),
                config_json: serde_json::json!({"source": "wl05_manual_order"}),
                host_fingerprint: "test-host".to_string(),
            },
        )
        .await
        .expect("insert_run");
        mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
        mqk_db::begin_run(&pool, run_id).await.expect("begin_run");

        let client_request_id = unique_id("manual");
        // Real manual-submit order_json shape (see
        // routes/execution.rs::ValidatedManualOrderSubmit::order_json): no
        // strategy_id, no strategy_semantic_fingerprint, no signal_source.
        let manual_order_json = serde_json::json!({
            "symbol": "MSFT",
            "side": "sell",
            "qty": 5,
            "order_type": "market",
            "time_in_force": "day",
            "limit_price": serde_json::Value::Null,
        });
        mqk_db::outbox_enqueue_for_running_run(&pool, run_id, &client_request_id, manual_order_json)
            .await
            .expect("enqueue manual outbox row");

        insert_fill_for_order(&pool, run_id, &client_request_id, "MSFT").await;

        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        st.inject_running_loop_for_test(run_id).await;

        let journal = fetch_journal(&st).await;
        let row = find_fill_row(&journal, &client_request_id);
        assert!(row["strategy_id"].is_null(), "manual order must have no strategy_id; row={row}");
        assert!(
            row["strategy_semantic_fingerprint"].is_null(),
            "manual order must have no fingerprint; row={row}"
        );
        assert_eq!(row["strategy_attribution_state"], "unattributed_manual");
    })
    .await;
}

// ---------------------------------------------------------------------------
// Test 6 — missing originating outbox row
// ---------------------------------------------------------------------------

/// If fill telemetry claims an `internal_order_id` that resolves to no
/// `oms_outbox` row at all, the journal must surface an explicit
/// `"lineage_missing"` state rather than silently reporting
/// `"unattributed_manual"` (which would hide a genuine data contradiction).
///
/// Design choice (see api_types.rs::PaperJournalFillsLane /
/// routes/paper_journal.rs doc comments): this is a ROW-level truth, not a
/// lane-level `query_failed` — the fills query itself succeeded and the
/// per-row outbox lookup ran without error, it simply found nothing. A
/// lane-level `query_failed` is reserved for genuine query/lookup errors
/// (see the mutation/RED-adjacent path in the route), preserving the
/// existing closed truth_state set for the lane
/// (active/no_active_run/no_db/query_failed) exercised by
/// scenario_paper_journal_jour01_ops09.rs's J11.
#[tokio::test]
async fn wl06_missing_originating_outbox_reports_lineage_missing_not_unattributed() {
    mqk_db::run_isolated("wl06_missing_outbox", |pool| async move {
        let run_id = Uuid::new_v4();
        let now = Utc::now();
        mqk_db::insert_run(
            &pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "mqk-daemon".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: now,
                git_hash: "test".to_string(),
                config_hash: "test".to_string(),
                config_json: serde_json::json!({"source": "wl06_missing_outbox"}),
                host_fingerprint: "test-host".to_string(),
            },
        )
        .await
        .expect("insert_run");
        mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
        mqk_db::begin_run(&pool, run_id).await.expect("begin_run");

        // No oms_outbox row is ever inserted for this internal_order_id --
        // a genuine contradiction between fill telemetry and outbox truth
        // (e.g. outbox retention pruning, or a corrupted write path).
        let orphan_order_id = unique_id("orphan");
        insert_fill_for_order(&pool, run_id, &orphan_order_id, "AAPL").await;

        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        st.inject_running_loop_for_test(run_id).await;

        let journal = fetch_journal(&st).await;
        // The fills query itself succeeded (one telemetry row exists) --
        // the lane truth_state must remain "active", not degrade to
        // query_failed, because no lookup errored.
        assert_eq!(journal["fills_lane"]["truth_state"], "active");
        let row = find_fill_row(&journal, &orphan_order_id);
        assert!(
            row["strategy_id"].is_null(),
            "row with no originating outbox must not report a strategy_id; row={row}"
        );
        assert!(row["strategy_semantic_fingerprint"].is_null());
        assert_eq!(
            row["strategy_attribution_state"], "lineage_missing",
            "must be explicitly distinguished from unattributed_manual; row={row}"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// Shared fixture: a bare RUNNING run (no registry/promotion seeding needed
// for the lineage-integrity tests below).
// ---------------------------------------------------------------------------

///
/// `started_at_utc` is caller-supplied (not `Utc::now()` internally) because
/// `AppState::current_status_snapshot` resolves the daemon's "active run"
/// via `fetch_latest_run_for_engine`, which picks the run with the latest
/// `started_at_utc` for the engine/mode -- NOT whichever run a test injects
/// as its local loop. A test that seeds more than one run for the same
/// engine/mode must control this ordering explicitly and deterministically
/// rather than relying on wall-clock timing between two `Utc::now()` calls.
async fn seed_bare_running_run(
    pool: &sqlx::PgPool,
    source: &str,
    started_at_utc: DateTime<Utc>,
) -> Uuid {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({"source": source}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    run_id
}

// ---------------------------------------------------------------------------
// Test 7 — cross-run order/fill mismatch is lineage_invalid
// ---------------------------------------------------------------------------

/// A fill under Run A cites an `internal_order_id` whose globally-unique
/// `oms_outbox.idempotency_key` actually belongs to Run B. Unique
/// idempotency makes the join unambiguous, but it does NOT prove the
/// resolved outbox row belongs to Run A. The journal must never attribute
/// Run B's strategy identity to Run A's fill; it must report an explicit
/// lineage-integrity contradiction instead.
#[tokio::test]
async fn wl07_cross_run_order_fill_mismatch_is_lineage_invalid() {
    mqk_db::run_isolated("wl07_cross_run_mismatch", |pool| async move {
        // Run B is seeded with a strictly earlier started_at_utc than Run A
        // so `fetch_latest_run_for_engine` deterministically resolves Run A
        // as the daemon's current active run, regardless of wall-clock
        // timing or UUID tie-break ordering.
        let now = Utc::now();
        let run_b = seed_bare_running_run(&pool, "wl07_run_b", now - Duration::seconds(10)).await;
        let run_a = seed_bare_running_run(&pool, "wl07_run_a", now).await;

        // The idempotency_key is globally unique -- it lives under Run B's
        // outbox, carrying Run B's strategy identity.
        let shared_order_id = unique_id("dec_cross");
        let sid_b = unique_id("strat_b_cross");
        let run_b_order_json = serde_json::json!({
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
            "order_type": "market",
            "time_in_force": "day",
            "limit_price": serde_json::Value::Null,
            "strategy_id": sid_b,
            "strategy_semantic_fingerprint": fingerprint('x'),
            "signal_source": "internal_strategy_decision",
        });
        mqk_db::outbox_enqueue_for_running_run(&pool, run_b, &shared_order_id, run_b_order_json)
            .await
            .expect("enqueue Run B outbox row");

        // The fill telemetry row is recorded under Run A, citing the same
        // internal_order_id.
        insert_fill_for_order(&pool, run_a, &shared_order_id, "AAPL").await;

        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        st.inject_running_loop_for_test(run_a).await;

        let journal = fetch_journal(&st).await;
        assert_eq!(
            journal["fills_lane"]["truth_state"], "active",
            "a lineage-integrity contradiction is a row-level truth, not a query failure"
        );
        let row = find_fill_row(&journal, &shared_order_id);
        assert!(
            row["strategy_id"].is_null(),
            "Run B's strategy identity must never be attributed to Run A's fill; row={row}"
        );
        assert_ne!(
            row["strategy_id"].as_str(),
            Some(sid_b.as_str()),
            "must never surface Run B's strategy_id; row={row}"
        );
        assert!(row["strategy_semantic_fingerprint"].is_null());
        assert_eq!(
            row["strategy_attribution_state"], "lineage_invalid",
            "must never report attributed or unattributed_manual for a cross-run mismatch; row={row}"
        );
        assert_eq!(row["strategy_attribution_reason"], "run_mismatch");
    })
    .await;
}

// ---------------------------------------------------------------------------
// Test 8 — internal_strategy_decision with missing strategy_id
// ---------------------------------------------------------------------------

/// A durable outbox row whose `signal_source` says the order came from an
/// internal strategy decision, but which carries no `strategy_id` at all,
/// is a contradiction (a strategy-sourced order MUST carry its strategy
/// identity). This must surface as `lineage_invalid`, never as
/// `unattributed_manual` -- collapsing it into "manual" would hide the
/// corruption behind an innocuous label.
#[tokio::test]
async fn wl08_strategy_source_missing_strategy_id_is_lineage_invalid() {
    mqk_db::run_isolated("wl08_missing_strategy_id", |pool| async move {
        let run_id =
            seed_bare_running_run(&pool, "wl08_missing_strategy_id", Utc::now()).await;

        let dec_id = unique_id("dec_nosid");
        let malformed_order_json = serde_json::json!({
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
            "order_type": "market",
            "time_in_force": "day",
            "limit_price": serde_json::Value::Null,
            "signal_source": "internal_strategy_decision",
        });
        mqk_db::outbox_enqueue_for_running_run(&pool, run_id, &dec_id, malformed_order_json)
            .await
            .expect("enqueue malformed outbox row");

        insert_fill_for_order(&pool, run_id, &dec_id, "AAPL").await;

        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        st.inject_running_loop_for_test(run_id).await;

        let journal = fetch_journal(&st).await;
        assert_eq!(journal["fills_lane"]["truth_state"], "active");
        let row = find_fill_row(&journal, &dec_id);
        assert!(row["strategy_id"].is_null());
        assert!(row["strategy_semantic_fingerprint"].is_null());
        assert_eq!(
            row["strategy_attribution_state"], "lineage_invalid",
            "a strategy-sourced order missing strategy_id must never read as unattributed_manual; row={row}"
        );
        assert_eq!(
            row["strategy_attribution_reason"], "strategy_id_missing_for_strategy_source"
        );
    })
    .await;
}

/// The same contradiction via the other strategy-indicating `signal_source`
/// value (`external_signal_ingestion`) must be treated identically.
#[tokio::test]
async fn wl08b_external_signal_source_missing_strategy_id_is_lineage_invalid() {
    mqk_db::run_isolated("wl08b_external_missing_strategy_id", |pool| async move {
        let run_id =
            seed_bare_running_run(&pool, "wl08b_external_missing_strategy_id", Utc::now()).await;

        let dec_id = unique_id("dec_ext_nosid");
        let malformed_order_json = serde_json::json!({
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
            "order_type": "market",
            "time_in_force": "day",
            "limit_price": serde_json::Value::Null,
            "signal_source": "external_signal_ingestion",
        });
        mqk_db::outbox_enqueue_for_running_run(&pool, run_id, &dec_id, malformed_order_json)
            .await
            .expect("enqueue malformed outbox row");

        insert_fill_for_order(&pool, run_id, &dec_id, "AAPL").await;

        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        st.inject_running_loop_for_test(run_id).await;

        let journal = fetch_journal(&st).await;
        let row = find_fill_row(&journal, &dec_id);
        assert_eq!(row["strategy_attribution_state"], "lineage_invalid");
        assert_eq!(
            row["strategy_attribution_reason"], "strategy_id_missing_for_strategy_source"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// Test 9 — strategy_id wrong type / blank is lineage_invalid
// ---------------------------------------------------------------------------

/// A present-but-corrupt `strategy_id` (wrong JSON type, JSON null, or a
/// blank string) must never be treated as genuine absence. Each case must
/// surface `lineage_invalid` / `strategy_id_malformed`, never
/// `unattributed_manual`.
#[tokio::test]
async fn wl09_malformed_strategy_id_is_lineage_invalid() {
    mqk_db::run_isolated("wl09_malformed_strategy_id", |pool| async move {
        let cases: Vec<(&str, serde_json::Value)> = vec![
            ("wrong_type_number", serde_json::json!(12345)),
            ("json_null", serde_json::Value::Null),
            ("blank_string", serde_json::json!("   ")),
        ];

        for (label, bad_strategy_id) in cases {
            // A fresh run per case (matching wl04/wl05/wl06's single-run,
            // single-fetch convention) rather than one run fetched
            // repeatedly: this run is never heartbeated, and a repeated
            // fetch against the same stale-heartbeat run would trip the
            // daemon's deadman/staleness halt gate between cases -- a
            // concern orthogonal to what this test proves.
            let run_id =
                seed_bare_running_run(&pool, &format!("wl09_{label}"), Utc::now()).await;
            let dec_id = unique_id(&format!("dec_badsid_{label}"));
            let malformed_order_json = serde_json::json!({
                "symbol": "AAPL",
                "side": "buy",
                "qty": 10,
                "order_type": "market",
                "time_in_force": "day",
                "limit_price": serde_json::Value::Null,
                "strategy_id": bad_strategy_id,
                "signal_source": "internal_strategy_decision",
            });
            mqk_db::outbox_enqueue_for_running_run(&pool, run_id, &dec_id, malformed_order_json)
                .await
                .unwrap_or_else(|e| panic!("enqueue malformed outbox row ({label}): {e}"));

            insert_fill_for_order(&pool, run_id, &dec_id, "AAPL").await;

            let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
                pool.clone(),
                state::OperatorAuthMode::ExplicitDevNoToken,
            ));
            st.inject_running_loop_for_test(run_id).await;

            let journal = fetch_journal(&st).await;
            let row = find_fill_row(&journal, &dec_id);
            assert!(row["strategy_id"].is_null(), "case={label}; row={row}");
            assert_eq!(
                row["strategy_attribution_state"], "lineage_invalid",
                "case={label}; a corrupt strategy_id must never read as unattributed_manual; row={row}"
            );
            assert_eq!(
                row["strategy_attribution_reason"], "strategy_id_malformed",
                "case={label}; row={row}"
            );
        }
    })
    .await;
}

// ---------------------------------------------------------------------------
// Test 10 — fingerprint without strategy_id, or malformed fingerprint
// ---------------------------------------------------------------------------

/// A `strategy_semantic_fingerprint` present without a `strategy_id` is a
/// contradiction (a fingerprint only ever accompanies a strategy decision).
/// A `strategy_semantic_fingerprint` of the wrong JSON type is independently
/// malformed. Neither may surface as `attributed` or `unattributed_manual`.
#[tokio::test]
async fn wl10_fingerprint_without_strategy_id_or_malformed_is_lineage_invalid() {
    mqk_db::run_isolated("wl10_fingerprint_contradictions", |pool| async move {
        let run_id =
            seed_bare_running_run(&pool, "wl10_fingerprint_contradictions", Utc::now()).await;

        // Case A: well-formed fingerprint, but no strategy_id at all.
        let dec_id_a = unique_id("dec_fp_no_sid");
        let order_json_a = serde_json::json!({
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
            "order_type": "market",
            "time_in_force": "day",
            "limit_price": serde_json::Value::Null,
            "strategy_semantic_fingerprint": fingerprint('y'),
        });
        mqk_db::outbox_enqueue_for_running_run(&pool, run_id, &dec_id_a, order_json_a)
            .await
            .expect("enqueue fingerprint-without-strategy_id outbox row");
        insert_fill_for_order(&pool, run_id, &dec_id_a, "AAPL").await;

        // Case B: fingerprint present but wrong JSON type.
        let dec_id_b = unique_id("dec_fp_bad_type");
        let sid_b = unique_id("strat_fp_bad_type");
        let order_json_b = serde_json::json!({
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
            "order_type": "market",
            "time_in_force": "day",
            "limit_price": serde_json::Value::Null,
            "strategy_id": sid_b,
            "strategy_semantic_fingerprint": 999,
            "signal_source": "internal_strategy_decision",
        });
        mqk_db::outbox_enqueue_for_running_run(&pool, run_id, &dec_id_b, order_json_b)
            .await
            .expect("enqueue malformed-fingerprint outbox row");
        insert_fill_for_order(&pool, run_id, &dec_id_b, "AAPL").await;

        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        st.inject_running_loop_for_test(run_id).await;

        let journal = fetch_journal(&st).await;

        let row_a = find_fill_row(&journal, &dec_id_a);
        assert!(row_a["strategy_id"].is_null(), "row_a={row_a}");
        assert_eq!(row_a["strategy_attribution_state"], "lineage_invalid", "row_a={row_a}");
        assert_eq!(
            row_a["strategy_attribution_reason"], "fingerprint_without_strategy_id",
            "row_a={row_a}"
        );

        let row_b = find_fill_row(&journal, &dec_id_b);
        assert!(row_b["strategy_id"].is_null(), "row_b={row_b}");
        assert!(row_b["strategy_semantic_fingerprint"].is_null(), "row_b={row_b}");
        assert_eq!(row_b["strategy_attribution_state"], "lineage_invalid", "row_b={row_b}");
        assert_eq!(
            row_b["strategy_attribution_reason"], "fingerprint_malformed",
            "row_b={row_b}"
        );
    })
    .await;
}
