//! CUTOVER-1D-A3: a valid fractional strategy signal must never collapse to
//! absence at the operator/status or durable signal-evaluation seams.
//!
//! FS-01..FS-05 are in-process (no DB). FS-06..FS-07 are DB-backed
//! (`MQK_DATABASE_URL`, `#[ignore]`; run with `--include-ignored --test-threads=1`).
//!
//! Frozen contract under test: V1 whole-unit fields keep their historical
//! meaning and FAIL CLOSED (409 `quantity_not_representable_in_v1`) instead of
//! truncating/nulling a fractional quantity; the exact quantity is exposed via
//! additive V2 (`quantity_schema_version = "qty_micros_v1"`, raw `QtyMicros`).
//! `null` on V1 means only "no bar dispatched" / "on_bar never ran".

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::state::{AlpacaWsContinuityState, BrokerKind};
use mqk_daemon::{routes, state};
use mqk_db::{InsertStrategySignalEvaluationArgs, SignalQtyEvidence};
use mqk_schemas::QtyMicros;
use tower::ServiceExt;
use uuid::Uuid;

async fn call(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = router
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot failed");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json body"))
}

/// Paper+Alpaca state on the canonical path (WS live).
async fn paper_alpaca() -> Arc<state::AppState> {
    let st = Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ));
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "fs-msg".to_string(),
        last_event_at: "2026-03-30T15:00:00Z".to_string(),
    })
    .await;
    st
}

const FRACTIONAL: QtyMicros = QtyMicros::new(100);
const V1_REFUSAL: &str = "quantity_not_representable_in_v1";

// ---------------------------------------------------------------------------
// FS-01 (A/B/C): exact in-memory state; fractional != no-dispatch; flat != no-dispatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fs_01_last_bar_signal_v2_keeps_no_dispatch_flat_fractional_and_min_raw_distinct() {
    // no dispatch yet
    let st = paper_alpaca().await;
    let (s, none) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v2/strategy/last-bar-signal",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(none["quantity_schema_version"], "qty_micros_v1");
    assert_eq!(none["signal_state"], "no_dispatch");
    assert!(none["signal_qty_micros"].is_null());

    // flat (exact zero) is an evaluated outcome, NOT no-dispatch
    st.set_bar_tick_state_exact_for_test(1, QtyMicros::ZERO, 5);
    let (_, flat) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v2/strategy/last-bar-signal",
    )
    .await;
    assert_eq!(flat["signal_state"], "evaluated");
    assert_eq!(flat["signal_qty_micros"], 0);
    assert_ne!(flat, none);

    // 0.0001 => exact raw 100
    st.set_bar_tick_state_exact_for_test(2, FRACTIONAL, 5);
    let (_, frac) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v2/strategy/last-bar-signal",
    )
    .await;
    assert_eq!(frac["signal_state"], "evaluated");
    assert_eq!(frac["signal_qty_micros"], 100);
    assert_eq!(frac["bar_tick_dispatch_count"], 2);
    assert_ne!(
        frac, none,
        "a fractional dispatch must not look like no-dispatch"
    );

    // The historical i64::MIN sentinel must not collide with a real raw value.
    st.set_bar_tick_state_exact_for_test(3, QtyMicros::new(i64::MIN), 5);
    let (_, min) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v2/strategy/last-bar-signal",
    )
    .await;
    assert_eq!(min["signal_state"], "evaluated");
    assert_eq!(min["signal_qty_micros"], i64::MIN);
}

// ---------------------------------------------------------------------------
// FS-02 (F/J): readiness V1 — whole Equity unchanged; fractional refused
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fs_02_readiness_v1_whole_unchanged_fractional_refused_never_null() {
    let st = paper_alpaca().await;

    let (s, v) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v1/autonomous/readiness",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["last_bar_signal_qty"].is_null(), "no dispatch => null");

    st.set_bar_tick_state_for_test(1, 5, 5);
    let (s, v) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v1/autonomous/readiness",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v["last_bar_signal_qty"], 5,
        "whole Equity V1 serialization unchanged"
    );

    st.set_bar_tick_state_exact_for_test(2, FRACTIONAL, 5);
    let (s, v) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v1/autonomous/readiness",
    )
    .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "fractional must fail closed on V1: {v}"
    );
    assert_eq!(v["error"], V1_REFUSAL);
    assert!(
        v.get("last_bar_signal_qty").is_none(),
        "no null/0 placeholder"
    );
    assert!(
        v["detail"]
            .as_str()
            .unwrap()
            .contains("/api/v2/strategy/last-bar-signal"),
        "refusal must point at the exact V2 surface: {v}"
    );
}

// ---------------------------------------------------------------------------
// FS-03 (F/J): autonomous paper-status V1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fs_03_paper_status_v1_whole_unchanged_fractional_refused() {
    let st = paper_alpaca().await;

    st.set_bar_tick_state_for_test(1, 5, 5);
    let (s, v) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v1/autonomous/paper-status",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["target_qty"], 5);

    st.set_bar_tick_state_exact_for_test(2, FRACTIONAL, 5);
    let (s, v) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v1/autonomous/paper-status",
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"], V1_REFUSAL);
    assert!(v.get("target_qty").is_none());
}

// ---------------------------------------------------------------------------
// FS-04: overflowed total is an evaluated outcome without an exact quantity
// (never no-dispatch); V1 refuses it, V2 says so explicitly.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fs_04_overflow_is_distinct_from_no_dispatch_and_refused_on_v1() {
    let st = paper_alpaca().await;
    st.record_bar_tick_outcome_for_test(None);

    let (_, v2) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v2/strategy/last-bar-signal",
    )
    .await;
    assert_eq!(v2["signal_state"], "total_overflowed");
    assert!(v2["signal_qty_micros"].is_null());
    assert_eq!(v2["bar_tick_dispatch_count"], 1);

    let (s, v1) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v1/autonomous/readiness",
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v1}");
    assert_eq!(v1["error"], V1_REFUSAL);
}

// ---------------------------------------------------------------------------
// FS-05: V2 signal-evaluation route is mounted and honest without a DB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fs_05_signal_evaluations_v2_no_db_is_unavailable_not_empty() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let (s, v) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v2/execution/signal-evaluations",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["quantity_schema_version"], "qty_micros_v1");
    assert_eq!(v["truth_state"], "db_unavailable");
    let (s, v1) = call(
        routes::build_router(st),
        "/api/v1/execution/signal-evaluations",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v1["truth_state"], "db_unavailable");
    assert!(
        v1.get("quantity_schema_version").is_none(),
        "V1 shape unchanged"
    );
}

// ---------------------------------------------------------------------------
// DB-backed: durable journal V1/V2 (D/E/F/G/J)
// ---------------------------------------------------------------------------

async fn db_state() -> (sqlx::PgPool, Arc<state::AppState>) {
    let pool = mqk_db::testkit_db_pool()
        .await
        .expect("MQK_DATABASE_URL test pool");
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    (pool, st)
}

fn eval_args(
    id: Uuid,
    stage: &str,
    generated: bool,
    qty: SignalQtyEvidence,
    side: Option<&str>,
) -> InsertStrategySignalEvaluationArgs {
    let now = chrono::Utc::now();
    InsertStrategySignalEvaluationArgs {
        evaluation_id: id,
        ts_utc: now,
        run_id: None,
        strategy_id: "fs_a3_strategy".to_string(),
        symbol: "FSA3".to_string(),
        timeframe: "5m".to_string(),
        bar_context_source: "db_loaded".to_string(),
        bars_loaded: 10,
        latest_bar_ts_utc: Some(now),
        signal_generated: generated,
        signal_qty: qty,
        signal_side: side.map(str::to_string),
        reason_code: "fs".to_string(),
        reason: "fs".to_string(),
        decision_stage: stage.to_string(),
        source: "fs_a3_test".to_string(),
    }
}

/// A failed earlier run must not leave a fractional row that makes the global
/// V1 journal listing refuse for unrelated tests.
async fn purge_leftovers(pool: &sqlx::PgPool) {
    let _ = sqlx::query(
        "delete from strategy_signal_evaluations          where strategy_id in ('qty_epoch_test', 'fs_a3_strategy', 'fs_a3_writer')",
    )
    .execute(pool)
    .await;
}

async fn purge(pool: &sqlx::PgPool, ids: &[Uuid]) {
    for id in ids {
        let _ = sqlx::query("delete from strategy_signal_evaluations where evaluation_id = $1")
            .bind(id)
            .execute(pool)
            .await;
    }
}

fn row(v: &serde_json::Value, id: Uuid) -> &serde_json::Value {
    v["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .find(|r| r["evaluation_id"] == id.to_string())
        .unwrap_or_else(|| panic!("row {id} missing from {v}"))
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn fs_06_signal_evaluation_journal_v1_refuses_fractional_v2_exact() {
    let (pool, st) = db_state().await;
    purge_leftovers(&pool).await;
    let (whole, flat, absent, frac) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let five = QtyMicros::from_whole_units(5).unwrap();
    for a in [
        eval_args(
            whole,
            "strategy_evaluated",
            true,
            SignalQtyEvidence::Exact(five),
            Some("buy"),
        ),
        eval_args(
            flat,
            "strategy_evaluated",
            false,
            SignalQtyEvidence::Exact(QtyMicros::ZERO),
            None,
        ),
        eval_args(
            absent,
            "pre_dispatch_gate",
            false,
            SignalQtyEvidence::NotEvaluated,
            None,
        ),
        eval_args(
            frac,
            "strategy_evaluated",
            true,
            SignalQtyEvidence::Exact(FRACTIONAL),
            Some("buy"),
        ),
    ] {
        mqk_db::insert_strategy_signal_evaluation(&pool, &a)
            .await
            .expect("insert");
    }
    let all = [whole, flat, absent, frac];

    // F: V1 with a fractional evaluated row present refuses (never null/0/truncated).
    let (s, v1) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v1/execution/signal-evaluations",
    )
    .await;
    // G: V2 exposes the exact value.
    let (s2, v2) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v2/execution/signal-evaluations",
    )
    .await;

    // J + E: with the fractional row removed, V1 serialization is the
    // historical shape and the pre-dispatch row is genuinely absent.
    purge(&pool, &[frac]).await;
    let (s3, v1_ok) = call(
        routes::build_router(Arc::clone(&st)),
        "/api/v1/execution/signal-evaluations",
    )
    .await;
    purge(&pool, &all).await;

    assert_eq!(s, StatusCode::CONFLICT, "{v1}");
    assert_eq!(v1["error"], V1_REFUSAL);
    assert!(v1.get("rows").is_none());

    assert_eq!(s2, StatusCode::OK);
    assert_eq!(v2["quantity_schema_version"], "qty_micros_v1");
    let f = row(&v2, frac);
    assert_eq!(f["signal_qty_micros"], 100, "exact raw 100");
    assert_eq!(f["signal_qty_state"], "exact");
    assert_eq!(f["signal_generated"], true);
    assert_eq!(f["signal_side"], "buy");
    assert!(f.get("signal_qty").is_none(), "V2 has no whole-unit field");
    assert_eq!(row(&v2, whole)["signal_qty_micros"], 5_000_000);
    assert_eq!(row(&v2, flat)["signal_qty_micros"], 0);
    assert_eq!(row(&v2, flat)["signal_qty_state"], "exact");
    assert!(row(&v2, absent)["signal_qty_micros"].is_null());
    assert_eq!(row(&v2, absent)["signal_qty_state"], "not_evaluated");

    assert_eq!(s3, StatusCode::OK, "{v1_ok}");
    assert!(v1_ok.get("quantity_schema_version").is_none());
    assert_eq!(row(&v1_ok, whole)["signal_qty"], 5);
    assert_eq!(row(&v1_ok, whole)["signal_side"], "buy");
    assert_eq!(row(&v1_ok, flat)["signal_qty"], 0);
    assert!(row(&v1_ok, absent)["signal_qty"].is_null());
    assert!(row(&v1_ok, whole).get("signal_qty_micros").is_none());
}

/// D: the real production writer persists a successfully evaluated 0.0001
/// signal as raw 100 / generated=true / side=buy, and a negative fractional as
/// side=sell -- never as an absent quantity.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn fs_07_production_writer_persists_fractional_signal_exactly() {
    let (pool, st) = db_state().await;
    purge_leftovers(&pool).await;
    let run_id = Uuid::new_v4();
    st.record_signal_evaluation_for_test(
        run_id,
        "fs_a3_writer",
        "FSW1",
        1,
        Some(QtyMicros::new(100)),
    )
    .await;
    st.record_signal_evaluation_for_test(
        run_id,
        "fs_a3_writer",
        "FSW2",
        2,
        Some(QtyMicros::new(-250)),
    )
    .await;
    let rows = mqk_db::fetch_strategy_signal_evaluations_for_run(&pool, run_id, 10)
        .await
        .expect("fetch");
    let _ = sqlx::query("delete from strategy_signal_evaluations where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await;

    let b = rows.iter().find(|r| r.symbol == "FSW1").expect("buy row");
    assert!(
        b.signal_generated,
        "successful evaluation is generated=true"
    );
    assert_eq!(b.signal_qty, SignalQtyEvidence::Exact(QtyMicros::new(100)));
    assert_eq!(b.signal_side.as_deref(), Some("buy"));
    assert_eq!(b.decision_stage, "strategy_evaluated");
    let s = rows.iter().find(|r| r.symbol == "FSW2").expect("sell row");
    assert_eq!(s.signal_qty, SignalQtyEvidence::Exact(QtyMicros::new(-250)));
    assert_eq!(s.signal_side.as_deref(), Some("sell"));
}
