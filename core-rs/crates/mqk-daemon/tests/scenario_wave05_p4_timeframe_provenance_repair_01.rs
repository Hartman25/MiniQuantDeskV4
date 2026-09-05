//! WAVE05-P4-DURABLE-TIMEFRAME-PROVENANCE-REPAIR-01
//!
//! Closes a false-positive production-seam gap: P4's regime context reads
//! `order_json.timeframe_secs` from the durable `oms_outbox` row
//! (`mqk_db::fetch_order_symbol_timeframe_context`), but the real production
//! internal-decision writer (`decision::build_order_json`) did not persist
//! that field -- every prior P4 test proved the reader against a
//! test-hand-seeded payload, never against what
//! `submit_internal_strategy_decision` actually writes.
//!
//! These tests drive the REAL production path:
//! `InternalStrategyDecision -> submit_internal_strategy_decision ->
//! build_order_json -> oms_outbox`, then verify the persisted row and (for
//! TF-R2/TF-R3) chain through to a real fill and `GET
//! /api/v1/strategy/performance`'s P4 regime context.
//!
//! All tests run against a real disposable Postgres database created and
//! torn down per-test via `mqk_db::run_isolated` (migrations applied
//! automatically). No `MQK_DATABASE_URL` / `--include-ignored` required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{DateTime, Duration, TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalDecisionOutcome, InternalStrategyDecision},
    routes, state,
};
use mqk_execution::{BrokerEvent, Side};
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

fn get(path: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn fetch_performance(st: &Arc<state::AppState>, run_id: Uuid) -> serde_json::Value {
    let router = routes::build_router(Arc::clone(st));
    let (status, body) = call(
        router,
        get(&format!("/api/v1/strategy/performance?run_id={run_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "strategy/performance must return 200");
    serde_json::from_slice(&body).expect("body is not valid JSON")
}

fn find_row<'a>(perf: &'a serde_json::Value, strategy_id: &str, fingerprint: &str) -> Option<&'a serde_json::Value> {
    perf["rows"]
        .as_array()
        .expect("rows must be an array")
        .iter()
        .find(|r| {
            r["strategy_id"].as_str() == Some(strategy_id)
                && r["strategy_semantic_fingerprint"].as_str() == Some(fingerprint)
        })
}

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap()
}

/// A fixed, syntactically-valid (64 lowercase hex) semantic fingerprint used
/// consistently for both `seed_active_paper_promotion`'s row and every
/// decision built here -- these tests prove timeframe provenance / gate
/// sequencing, not config-identity binding, so the exact value is irrelevant
/// as long as both sides agree (mirrors `scenario_internal_strategy_decision.rs`).
fn test_fingerprint() -> String {
    "e".repeat(64)
}

// ---------------------------------------------------------------------------
// DB fixtures
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

/// STRATEGY-PROMOTION-REGISTRY-01D: seed a durable `active_paper` promotion
/// for the exact `(strategy_id, symbol, timeframe_secs)` identity, walking
/// the full legal transition graph. Distinct `timeframe_secs` values for the
/// same `(strategy_id, symbol)` are independent identities -- used by
/// TF-R2 to model a "current config" that has drifted from what an
/// already-durable order actually recorded.
async fn seed_active_paper_promotion(pool: &sqlx::PgPool, strategy_id: &str, symbol: &str, timeframe_secs: i64) {
    let now = Utc::now();
    let seed = |suffix: &str| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("test-promo-seed:{strategy_id}:{symbol}:{timeframe_secs}:{suffix}").as_bytes(),
        )
    };
    let step = |transition_id: Uuid,
                previous_state: Option<&str>,
                new_state: &str,
                effective_at: DateTime<Utc>| {
        mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            config_fingerprint: Some(test_fingerprint()),
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
    mqk_db::insert_strategy_promotion_transition(pool, &step(seed("1"), None, "shadow_approved", now))
        .await
        .expect("seed shadow_approved");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(seed("2"), Some("shadow_approved"), "paper_approved", now + Duration::milliseconds(1)),
    )
    .await
    .expect("seed paper_approved");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(seed("3"), Some("paper_approved"), "active_paper", now + Duration::milliseconds(2)),
    )
    .await
    .expect("seed active_paper");
}

/// Seed a RUNNING run in the DB and wire up the local loop handle so
/// `submit_internal_strategy_decision`'s Gate 6 (active run) passes.
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
            config_json: serde_json::json!({"source": "scenario_wave05_p4_timeframe_provenance_repair_01"}),
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

/// Submit one internal decision through the REAL production gate sequence
/// and, when accepted, apply a matching fill through the real inbox path --
/// mirrors the manual `SENT` + inbox-apply steps every other Wave05
/// closed-trade fixture performs, but sourced from the actual outbox row
/// `submit_internal_strategy_decision` wrote rather than a hand-built one.
#[allow(clippy::too_many_arguments)]
async fn submit_and_fill(
    st: &Arc<state::AppState>,
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    side: &str,
    qty: i64,
    price_micros: i64,
    decision_id: &str,
    at: DateTime<Utc>,
) -> InternalDecisionOutcome {
    let d = InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: symbol.to_string(),
        timeframe_secs,
        strategy_semantic_fingerprint: test_fingerprint(),
        side: side.to_string(),
        qty,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    };
    let out = submit_internal_strategy_decision(st, d).await;
    if out.accepted {
        sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
            .bind(decision_id)
            .execute(pool)
            .await
            .expect("mark outbox SENT should succeed");

        let broker_side = if side.eq_ignore_ascii_case("buy") { Side::Buy } else { Side::Sell };
        let bm = format!("bm:{decision_id}");
        let bo = format!("bo:{decision_id}");
        let ev = BrokerEvent::Fill {
            broker_message_id: bm.clone(),
            broker_fill_id: None,
            internal_order_id: decision_id.to_string(),
            broker_order_id: Some(bo.clone()),
            symbol: symbol.to_string(),
            side: broker_side,
            delta_qty: qty,
            price_micros,
            fee_micros: 0,
        };
        let json = serde_json::to_value(&ev).expect("serialize BrokerEvent");
        let run_id = out.active_run_id.expect("accepted decision must echo active_run_id");
        mqk_db::inbox_insert_deduped_with_identity(
            pool, run_id, &bm, None, decision_id, &bo, "fill", &json, 0, at,
        )
        .await
        .expect("inbox insert should succeed");
        mqk_db::inbox_mark_applied(pool, run_id, &bm, at)
            .await
            .expect("inbox mark applied should succeed");
    }
    out
}

async fn max_applied_inbox_id(pool: &sqlx::PgPool, run_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, Option<i64>>(
        "select max(inbox_id) from oms_inbox where run_id = $1 and applied_at_utc is not null",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .expect("max inbox_id query failed")
    .unwrap_or(0)
}

async fn seed_accounting_state(pool: &sqlx::PgPool, run_id: Uuid, realized_pnl_micros: i64) {
    let snapshot_id = Uuid::new_v4();
    let now = Utc::now();
    let insert_outcome = mqk_db::insert_or_confirm_paper_portfolio_snapshot(
        pool,
        mqk_db::NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: now,
            deployment_mode: "paper".to_string(),
            source: mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 100_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert_or_confirm_paper_portfolio_snapshot failed");
    assert!(matches!(insert_outcome, mqk_db::InsertPaperPortfolioSnapshotOutcome::Inserted { .. }));

    let last_applied_inbox_id = max_applied_inbox_id(pool, run_id).await;
    let upsert_outcome = mqk_db::upsert_paper_portfolio_accounting_state(
        pool,
        mqk_db::UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 100_000_000_000,
            realized_pnl_micros,
            fees_micros: 0,
            last_applied_inbox_id,
            accounting_epoch: "complete".to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: now,
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("upsert_paper_portfolio_accounting_state failed");
    assert!(matches!(upsert_outcome, mqk_db::UpsertPaperPortfolioAccountingStateOutcome::Inserted { .. }));
}

async fn seed_md_bar(pool: &sqlx::PgPool, symbol: &str, timeframe: &str, end_ts: i64, close_micros: i64) {
    sqlx::query(
        r#"
        insert into md_bars (symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete)
        values ($1, $2, $3, $4, $4, $4, $4, 1000, true)
        on conflict (symbol, timeframe, end_ts) do nothing
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(end_ts)
    .bind(close_micros)
    .execute(pool)
    .await
    .expect("seed_md_bar insert failed");
}

/// Minimal run row for TF-R5's direct DB-level assertions (no decision/HTTP
/// path involved -- just a valid `run_id` for the outbox FK).
async fn seed_bare_run(pool: &sqlx::PgPool) -> Uuid {
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
            config_json: serde_json::json!({"source": "tf_r5"}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert_run");
    run_id
}

// ---------------------------------------------------------------------------
// TF-R1 -- the real production writer persists the exact timeframe_secs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tf_r1_internal_decision_persists_exact_timeframe_secs() {
    mqk_db::run_isolated("tf_r1_persist", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let sid = unique_id("strat");
        seed_registry(&pool, &sid, true).await;
        seed_active_paper_promotion(&pool, &sid, "AAPL", 300).await;
        mqk_db::persist_arm_state(&pool, "ARMED", None)
            .await
            .expect("persist arm state");
        let _run_id = seed_active_run(&st).await;

        let dec_id = unique_id("dec");
        let d = InternalStrategyDecision {
            decision_id: dec_id.clone(),
            strategy_id: sid.clone(),
            symbol: "AAPL".to_string(),
            timeframe_secs: 300,
            strategy_semantic_fingerprint: test_fingerprint(),
            side: "buy".to_string(),
            qty: 10,
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
        };
        let out = submit_internal_strategy_decision(&st, d).await;
        assert_eq!(out.disposition, "accepted", "blockers={:?}", out.blockers);

        let row: (serde_json::Value,) =
            sqlx::query_as("SELECT order_json FROM oms_outbox WHERE idempotency_key = $1")
                .bind(&dec_id)
                .fetch_one(&pool)
                .await
                .expect("outbox row must exist");
        assert_eq!(row.0["timeframe_secs"], 300, "order_json={}", row.0);
        assert_eq!(row.0["strategy_id"], sid);
        assert_eq!(row.0["strategy_semantic_fingerprint"], test_fingerprint());
        assert_eq!(row.0["signal_source"], "internal_strategy_decision");
    })
    .await;
}

// ---------------------------------------------------------------------------
// TF-R2 -- a conflicting CURRENT config/promotion timeframe must never
// override the already-durable order's exact recorded timeframe.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tf_r2_current_config_drift_never_overrides_durable_timeframe() {
    mqk_db::run_isolated("tf_r2_config_drift", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let sid = unique_id("strat");
        seed_registry(&pool, &sid, true).await;
        seed_active_paper_promotion(&pool, &sid, "AAPL", 300).await;
        mqk_db::persist_arm_state(&pool, "ARMED", None)
            .await
            .expect("persist arm state");
        let run_id = seed_active_run(&st).await;

        let buy = submit_and_fill(&st, &pool, &sid, "AAPL", 300, "buy", 10, 100_000_000, &unique_id("tf2buy"), at()).await;
        assert_eq!(buy.disposition, "accepted", "blockers={:?}", buy.blockers);
        let sell = submit_and_fill(&st, &pool, &sid, "AAPL", 300, "sell", 10, 110_000_000, &unique_id("tf2sell"), at()).await;
        assert_eq!(sell.disposition, "accepted", "blockers={:?}", sell.blockers);

        seed_accounting_state(&pool, run_id, 100_000_000).await;
        for i in 0..20 {
            seed_md_bar(&pool, "AAPL", "5m", 1_700_000_000 + i * 300, 1_000_000 + i * 100).await;
        }

        // CURRENT config drift: the strategy is NOW also promoted at a
        // completely different timeframe for the same (strategy_id, symbol).
        // The already-durable order above still recorded 300 -- that must be
        // what P4 reports, never the newer 86400 identity.
        seed_active_paper_promotion(&pool, &sid, "AAPL", 86400).await;

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        let row = find_row(&perf, &sid, &test_fingerprint()).expect("row must exist");
        assert_eq!(
            row["regime_context"]["timeframe_secs"], 300,
            "current config drift must never override the durable order's exact timeframe; row={row}"
        );
        assert_ne!(row["regime_context"]["timeframe_secs"], 86400, "row={row}");
    })
    .await;
}

// ---------------------------------------------------------------------------
// TF-R3 -- the P4 route resolves regime context from provenance created
// through the real production internal-decision writer end to end.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tf_r3_p4_route_resolves_regime_from_real_production_provenance() {
    mqk_db::run_isolated("tf_r3_route_proof", |pool| async move {
        let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        let sid = unique_id("strat");
        seed_registry(&pool, &sid, true).await;
        seed_active_paper_promotion(&pool, &sid, "AAPL", 300).await;
        mqk_db::persist_arm_state(&pool, "ARMED", None)
            .await
            .expect("persist arm state");
        let run_id = seed_active_run(&st).await;

        let buy = submit_and_fill(&st, &pool, &sid, "AAPL", 300, "buy", 10, 100_000_000, &unique_id("tf3buy"), at()).await;
        assert_eq!(buy.disposition, "accepted", "blockers={:?}", buy.blockers);
        let sell = submit_and_fill(&st, &pool, &sid, "AAPL", 300, "sell", 10, 110_000_000, &unique_id("tf3sell"), at()).await;
        assert_eq!(sell.disposition, "accepted", "blockers={:?}", sell.blockers);

        seed_accounting_state(&pool, run_id, 100_000_000).await;
        // 20 completed 5m bars -- comfortably above the regime detector's min_bars.
        for i in 0..20 {
            seed_md_bar(&pool, "AAPL", "5m", 1_700_000_000 + i * 300, 1_000_000 + i * 100).await;
        }

        let perf = fetch_performance(&st, run_id).await;
        assert_eq!(perf["truth_state"], "active", "perf={perf}");
        let row = find_row(&perf, &sid, &test_fingerprint()).expect("row must exist");
        assert_eq!(row["attributed_close_event_count"], 1, "row={row}");
        assert_eq!(row["regime_context"]["symbol"], "AAPL", "row={row}");
        assert_eq!(row["regime_context"]["timeframe_secs"], 300, "row={row}");
        assert_eq!(row["regime_context"]["regime_authority"], "research_only_observational");
        assert_ne!(row["regime_context"]["regime_truth_state"], "context_unavailable", "row={row}");
        assert_ne!(row["regime_context"]["regime_truth_state"], "query_failed", "row={row}");
    })
    .await;
}

// ---------------------------------------------------------------------------
// TF-R5 -- null / wrong-type / zero / negative timeframe_secs all resolve to
// `None` (context_unavailable upstream), never a fabricated default.
//
// Hand-seeded malformed legacy rows are an explicitly permitted exception to
// "drive the real writer" (see module doc / mission REPAIR B) -- this proves
// the READER's fail-closed handling of every malformed shape, independent of
// which writer produced it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tf_r5_malformed_timeframe_secs_variants_resolve_to_none() {
    mqk_db::run_isolated("tf_r5_malformed", |pool| async move {
        let run_id = seed_bare_run(&pool).await;

        let cases: [(&str, serde_json::Value); 5] = [
            ("missing", serde_json::Value::Null), // key omitted entirely below
            ("null", serde_json::Value::Null),
            ("wrong_type", serde_json::json!("not-a-number")),
            ("zero", serde_json::json!(0)),
            ("negative", serde_json::json!(-60)),
        ];

        for (label, value) in cases {
            let order_id = unique_id(&format!("tfr5_{label}"));
            let mut json = serde_json::json!({"symbol": "AAPL", "qty": 1, "side": "buy"});
            if label != "missing" {
                json["timeframe_secs"] = value;
            }
            mqk_db::outbox_enqueue(&pool, run_id, &order_id, json)
                .await
                .expect("outbox_enqueue should succeed");

            let resolved = mqk_db::fetch_order_symbol_timeframe_context(&pool, run_id, &order_id)
                .await
                .expect("query must not error");
            assert_eq!(resolved, None, "label={label} must resolve to None");
        }
    })
    .await;
}
