//! CUTOVER-1D-A3: durable strategy signal-evaluation quantity authority
//! (migration 0077, `qty_micros_v1`).
//!
//! Proves a successfully evaluated fractional signal is journaled as exact
//! raw `QtyMicros` (never as an absent quantity), that historical whole-unit
//! rows are preserved unscaled, and that mixed / unknown quantity authority is
//! refused by the schema.
//!
//! DB-backed; requires `MQK_DATABASE_URL` (disposable test DB) and is `#[ignore]`.
//! Run: `cargo test -p mqk-db --test scenario_signal_evaluation_qty_micros_01 -- --include-ignored --test-threads=1`

use chrono::Utc;
use mqk_db::{
    fetch_strategy_signal_evaluation, insert_strategy_signal_evaluation,
    InsertStrategySignalEvaluationArgs, SignalQtyEvidence, ENV_DB_URL,
};
use mqk_schemas::{QtyMicros, QTY_MICROS_SCALE};
use sqlx::Row;
use uuid::Uuid;

async fn test_pool() -> Option<sqlx::PgPool> {
    if std::env::var(ENV_DB_URL).is_err() {
        eprintln!("SKIP: requires MQK_DATABASE_URL");
        return None;
    }
    Some(mqk_db::testkit_db_pool().await.expect("testkit db pool"))
}

fn args(
    evaluation_id: Uuid,
    stage: &str,
    generated: bool,
    signal_qty: SignalQtyEvidence,
    side: Option<&str>,
) -> InsertStrategySignalEvaluationArgs {
    let now = Utc::now();
    InsertStrategySignalEvaluationArgs {
        evaluation_id,
        ts_utc: now,
        run_id: Some(Uuid::new_v4()),
        strategy_id: "qty_epoch_test".to_string(),
        symbol: "TEST".to_string(),
        timeframe: "5m".to_string(),
        bar_context_source: "db_loaded".to_string(),
        bars_loaded: 10,
        latest_bar_ts_utc: Some(now),
        signal_generated: generated,
        signal_qty,
        signal_side: side.map(str::to_string),
        reason_code: "test".to_string(),
        reason: "test".to_string(),
        decision_stage: stage.to_string(),
        source: "test".to_string(),
    }
}

async fn raw_columns(
    pool: &sqlx::PgPool,
    id: Uuid,
) -> (Option<i64>, Option<String>, Option<i64>, Option<String>) {
    let row = sqlx::query(
        "select signal_qty, quantity_schema_version, signal_qty_micros, signal_side \
         from strategy_signal_evaluations where evaluation_id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("read raw columns");
    (
        row.try_get("signal_qty").unwrap(),
        row.try_get("quantity_schema_version").unwrap(),
        row.try_get("signal_qty_micros").unwrap(),
        row.try_get("signal_side").unwrap(),
    )
}

/// Rows left by an earlier failed run of this suite (fractional rows would make
/// the daemon's global V1 journal listing refuse for unrelated tests).
async fn purge_leftovers(pool: &sqlx::PgPool) {
    let _ =
        sqlx::query("delete from strategy_signal_evaluations where strategy_id = 'qty_epoch_test'")
            .execute(pool)
            .await;
}

async fn cleanup(pool: &sqlx::PgPool, ids: &[Uuid]) {
    for id in ids {
        let _ = sqlx::query("delete from strategy_signal_evaluations where evaluation_id = $1")
            .bind(id)
            .execute(pool)
            .await;
    }
}

/// D: a successfully evaluated 0.0001 signal persists raw 100, generated=true,
/// side=buy -- never NULL/0 -- and round-trips exactly.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn sq01_fractional_signal_persists_exact_raw_100_and_side_buy() {
    let Some(pool) = test_pool().await else {
        return;
    };
    purge_leftovers(&pool).await;
    let id = Uuid::new_v4();
    let fractional = QtyMicros::new(100);
    assert_eq!(fractional.raw(), 100, "fixture is 0.0001 == 100 raw micros");
    insert_strategy_signal_evaluation(
        &pool,
        &args(
            id,
            "strategy_evaluated",
            true,
            SignalQtyEvidence::Exact(fractional),
            Some("buy"),
        ),
    )
    .await
    .expect("insert");

    let (legacy, schema, micros, side) = raw_columns(&pool, id).await;
    assert_eq!(
        legacy, None,
        "historical whole-unit column is never written"
    );
    assert_eq!(schema.as_deref(), Some("qty_micros_v1"));
    assert_eq!(
        micros,
        Some(100),
        "exact raw micros, not absent/zero/rounded"
    );
    assert_eq!(side.as_deref(), Some("buy"));

    let rec = fetch_strategy_signal_evaluation(&pool, id)
        .await
        .expect("fetch")
        .expect("row");
    assert!(rec.signal_generated);
    assert_eq!(
        rec.signal_qty,
        SignalQtyEvidence::Exact(QtyMicros::new(100))
    );
    cleanup(&pool, &[id]).await;
}

/// Whole Equity, flat, pre-dispatch (absent) and overflow rows keep distinct
/// durable encodings (C, E).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn sq02_whole_flat_absent_and_overflow_are_distinct_durable_states() {
    let Some(pool) = test_pool().await else {
        return;
    };
    purge_leftovers(&pool).await;
    let (whole, flat, absent, overflow) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let five = QtyMicros::from_whole_units(5).unwrap();
    for (id, stage, generated, ev, side) in [
        (
            whole,
            "strategy_evaluated",
            true,
            SignalQtyEvidence::Exact(five),
            Some("buy"),
        ),
        (
            flat,
            "strategy_evaluated",
            false,
            SignalQtyEvidence::Exact(QtyMicros::ZERO),
            None,
        ),
        (
            absent,
            "pre_dispatch_gate",
            false,
            SignalQtyEvidence::NotEvaluated,
            None,
        ),
        (
            overflow,
            "strategy_evaluated",
            true,
            SignalQtyEvidence::TotalOverflowed,
            None,
        ),
    ] {
        insert_strategy_signal_evaluation(&pool, &args(id, stage, generated, ev, side))
            .await
            .expect("insert");
    }

    assert_eq!(
        raw_columns(&pool, whole).await,
        (
            None,
            Some("qty_micros_v1".into()),
            Some(5 * QTY_MICROS_SCALE),
            Some("buy".into())
        )
    );
    assert_eq!(
        raw_columns(&pool, flat).await,
        (None, Some("qty_micros_v1".into()), Some(0), None),
        "flat is exact zero, not absent"
    );
    assert_eq!(
        raw_columns(&pool, absent).await,
        (None, None, None, None),
        "pre-dispatch refusal is genuinely quantity-absent"
    );
    assert_eq!(
        raw_columns(&pool, overflow).await,
        (None, Some("qty_micros_v1".into()), None, None),
        "overflow is evaluated-without-exact-quantity, not pre-dispatch absence"
    );

    for (id, expected) in [
        (whole, SignalQtyEvidence::Exact(five)),
        (flat, SignalQtyEvidence::Exact(QtyMicros::ZERO)),
        (absent, SignalQtyEvidence::NotEvaluated),
        (overflow, SignalQtyEvidence::TotalOverflowed),
    ] {
        let rec = fetch_strategy_signal_evaluation(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rec.signal_qty, expected);
    }
    cleanup(&pool, &[whole, flat, absent, overflow]).await;
}

/// A historical whole-unit row (NULL schema, `signal_qty`) is preserved
/// unscaled at rest and decodes through the checked whole-unit conversion.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn sq03_historical_whole_unit_row_is_not_reinterpreted() {
    let Some(pool) = test_pool().await else {
        return;
    };
    purge_leftovers(&pool).await;
    let id = Uuid::new_v4();
    sqlx::query(
        "insert into strategy_signal_evaluations (evaluation_id, ts_utc, strategy_id, symbol, \
         timeframe, bar_context_source, bars_loaded, signal_generated, signal_qty, signal_side, \
         reason_code, reason, decision_stage, source) \
         values ($1, now(), 's', 'TEST', '5m', 'db_loaded', 1, true, 7, 'buy', 'r', 'r', \
         'strategy_evaluated', 'legacy')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("insert historical-shape row");

    let (legacy, schema, micros, _) = raw_columns(&pool, id).await;
    assert_eq!((legacy, schema, micros), (Some(7), None, None));
    let rec = fetch_strategy_signal_evaluation(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rec.signal_qty,
        SignalQtyEvidence::Exact(QtyMicros::new(7 * QTY_MICROS_SCALE))
    );
    cleanup(&pool, &[id]).await;
}

/// I: mixed / unknown quantity authority is refused by the schema.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn sq04_mixed_or_unknown_quantity_authority_is_refused() {
    let Some(pool) = test_pool().await else {
        return;
    };
    purge_leftovers(&pool).await;
    let cases: [(&str, Option<&str>, Option<i64>, Option<i64>); 3] = [
        ("historical row carrying micros", None, None, Some(5)),
        (
            "qty_micros_v1 row carrying legacy qty",
            Some("qty_micros_v1"),
            Some(1),
            Some(1),
        ),
        (
            "unknown schema version",
            Some("qty_micros_v2"),
            None,
            Some(1),
        ),
    ];
    for (label, schema, legacy, micros) in cases {
        let id = Uuid::new_v4();
        let res = sqlx::query(
            "insert into strategy_signal_evaluations (evaluation_id, ts_utc, strategy_id, symbol, \
             timeframe, bar_context_source, bars_loaded, signal_generated, signal_qty, \
             quantity_schema_version, signal_qty_micros, reason_code, reason, decision_stage, \
             source) values ($1, now(), 's', 'TEST', '5m', 'db_loaded', 1, true, $2, $3, $4, \
             'r', 'r', 'strategy_evaluated', 't')",
        )
        .bind(id)
        .bind(legacy)
        .bind(schema)
        .bind(micros)
        .execute(&pool)
        .await;
        assert!(
            res.is_err(),
            "{label} must be refused by the CHECK constraint"
        );
        cleanup(&pool, &[id]).await;
    }
}
