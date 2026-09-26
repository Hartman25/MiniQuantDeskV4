//! RC-M5-C: durable Paper portfolio snapshot positions carry an exact
//! `QtyMicros` quantity (migration 0078, `qty_micros_v1`).
//!
//! | ID  | Invariant                                                                 |
//! |-----|---------------------------------------------------------------------------|
//! | Q01 | a whole-unit position keeps the historical encoding (Equity unchanged)    |
//! | Q02 | a fractional position round-trips exactly under `qty_micros_v1`           |
//! | Q03 | exact replay is idempotent; a 1-micro change is a conflict                |
//! | Q04 | historical whole-unit rows read back as exact micros                      |
//! | Q05 | the CHECK constraint rejects mixed / mislabelled encodings                |
//! | Q06 | a corrupt row fails the read closed (never a fabricated quantity)         |
//!
//! DB-backed; require `MQK_DATABASE_URL` and are `#[ignore]`d:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-db --test scenario_paper_portfolio_snapshot_qty_micros_01 -- --include-ignored --test-threads=1

use chrono::{TimeZone, Utc};
use mqk_db::{
    fetch_paper_portfolio_snapshot_by_id, insert_or_confirm_paper_portfolio_snapshot, insert_run,
    InsertPaperPortfolioSnapshotOutcome, NewPaperPortfolioSnapshot, NewRun,
    PaperPortfolioSnapshotPosition, ENV_DB_URL, PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
};
use mqk_schemas::QtyMicros;
use sqlx::Row;
use uuid::Uuid;

const IGNORE: &str = "requires MQK_DATABASE_URL";

fn qty(s: &str) -> QtyMicros {
    s.parse().expect("test qty literal")
}

async fn pool() -> Option<sqlx::PgPool> {
    if std::env::var(ENV_DB_URL).is_err() {
        eprintln!("SKIP: requires MQK_DATABASE_URL");
        return None;
    }
    Some(mqk_db::testkit_db_pool().await.expect("test pool"))
}

fn id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.paper-snapshot-qty-micros.v1|{seed}").as_bytes(),
    )
}

async fn cleanup(pool: &sqlx::PgPool, run_id: Uuid) {
    let _ = sqlx::query(
        "delete from sys_paper_portfolio_snapshot_positions where snapshot_id in \
         (select snapshot_id from sys_paper_portfolio_snapshots where run_id = $1)",
    )
    .bind(run_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("delete from sys_paper_portfolio_snapshots where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

async fn fixture_run(pool: &sqlx::PgPool, run_id: Uuid) {
    insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: "test-paper-snapshot-qty-micros".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("fixture run insert");
}

fn snapshot(
    snapshot_id: Uuid,
    run_id: Uuid,
    positions: Vec<PaperPortfolioSnapshotPosition>,
) -> NewPaperPortfolioSnapshot {
    NewPaperPortfolioSnapshot {
        snapshot_id,
        captured_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 13, 0, 0).unwrap(),
        deployment_mode: "paper".to_string(),
        source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
        equity_micros: 100_000_000_000,
        cash_micros: 40_000_000_000,
        currency: "USD".to_string(),
        truth_state: "active".to_string(),
        run_id: Some(run_id),
        operation_id: None,
        positions,
    }
}

fn position(symbol: &str, q: &str) -> PaperPortfolioSnapshotPosition {
    PaperPortfolioSnapshotPosition {
        symbol: symbol.to_string(),
        qty_signed: qty(q),
        avg_entry_price_micros: 60_000_000_000,
        provenance: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
    }
}

async fn raw_row(
    pool: &sqlx::PgPool,
    snapshot_id: Uuid,
    symbol: &str,
) -> (Option<i64>, Option<i64>, Option<String>) {
    let row = sqlx::query(
        "select qty_signed, qty_signed_micros, quantity_schema_version \
         from sys_paper_portfolio_snapshot_positions where snapshot_id = $1 and symbol = $2",
    )
    .bind(snapshot_id)
    .bind(symbol)
    .fetch_one(pool)
    .await
    .expect("raw position row");
    (
        row.get("qty_signed"),
        row.get("qty_signed_micros"),
        row.get("quantity_schema_version"),
    )
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn q01_whole_unit_position_keeps_the_historical_encoding() {
    let _ = IGNORE;
    let Some(pool) = pool().await else { return };
    let run_id = id("q01-run");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;
    let sid = id("q01-snap");
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        snapshot(
            sid,
            run_id,
            vec![position("AAPL", "10"), position("TSLA", "-3")],
        ),
    )
    .await
    .expect("insert");

    assert_eq!(
        raw_row(&pool, sid, "AAPL").await,
        (Some(10), None, None),
        "Equity rows must stay byte-identical to the pre-0078 encoding"
    );
    assert_eq!(raw_row(&pool, sid, "TSLA").await, (Some(-3), None, None));

    let fetched = fetch_paper_portfolio_snapshot_by_id(&pool, sid)
        .await
        .expect("fetch")
        .expect("exists");
    let by_symbol: std::collections::BTreeMap<_, _> = fetched
        .positions
        .iter()
        .map(|p| (p.symbol.as_str(), p.qty_signed))
        .collect();
    assert_eq!(by_symbol["AAPL"], qty("10"));
    assert_eq!(by_symbol["TSLA"], qty("-3"));
    cleanup(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn q02_fractional_position_round_trips_exactly_under_qty_micros_v1() {
    let Some(pool) = pool().await else { return };
    let run_id = id("q02-run");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;
    let sid = id("q02-snap");
    let out = insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        snapshot(
            sid,
            run_id,
            vec![
                position("BTC/USD", "0.000101"),
                position("ETH/USD", "-1.5"),
                position("AAPL", "7"),
            ],
        ),
    )
    .await
    .expect("insert");
    assert!(matches!(
        out,
        InsertPaperPortfolioSnapshotOutcome::Inserted {
            positions_inserted: 3,
            ..
        }
    ));

    assert_eq!(
        raw_row(&pool, sid, "BTC/USD").await,
        (None, Some(101), Some("qty_micros_v1".to_string()))
    );
    assert_eq!(
        raw_row(&pool, sid, "ETH/USD").await,
        (None, Some(-1_500_000), Some("qty_micros_v1".to_string()))
    );
    assert_eq!(raw_row(&pool, sid, "AAPL").await, (Some(7), None, None));

    let fetched = fetch_paper_portfolio_snapshot_by_id(&pool, sid)
        .await
        .expect("fetch")
        .expect("exists");
    let by_symbol: std::collections::BTreeMap<_, _> = fetched
        .positions
        .iter()
        .map(|p| (p.symbol.as_str(), p.qty_signed))
        .collect();
    assert_eq!(by_symbol["BTC/USD"], QtyMicros::new(101));
    assert_eq!(by_symbol["ETH/USD"], QtyMicros::new(-1_500_000));
    assert_eq!(by_symbol["AAPL"], qty("7"));
    cleanup(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn q03_replay_is_idempotent_and_one_micro_is_a_conflict() {
    let Some(pool) = pool().await else { return };
    let run_id = id("q03-run");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;
    let sid = id("q03-snap");
    let base = || snapshot(sid, run_id, vec![position("BTC/USD", "0.5")]);
    insert_or_confirm_paper_portfolio_snapshot(&pool, base())
        .await
        .expect("insert");

    let replay = insert_or_confirm_paper_portfolio_snapshot(&pool, base())
        .await
        .expect("replay");
    assert!(matches!(
        replay,
        InsertPaperPortfolioSnapshotOutcome::AlreadyExists { .. }
    ));

    let changed = insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        snapshot(sid, run_id, vec![position("BTC/USD", "0.500001")]),
    )
    .await
    .expect("changed");
    assert!(matches!(
        changed,
        InsertPaperPortfolioSnapshotOutcome::Conflict { .. }
    ));
    cleanup(&pool, run_id).await;
}

async fn seed_snapshot_row(pool: &sqlx::PgPool, sid: Uuid, run_id: Uuid) {
    insert_or_confirm_paper_portfolio_snapshot(pool, snapshot(sid, run_id, vec![]))
        .await
        .expect("empty snapshot");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn q04_historical_whole_unit_rows_read_back_as_exact_micros() {
    let Some(pool) = pool().await else { return };
    let run_id = id("q04-run");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;
    let sid = id("q04-snap");
    seed_snapshot_row(&pool, sid, run_id).await;
    // A pre-0078 writer: legacy whole-unit column only.
    sqlx::query(
        "insert into sys_paper_portfolio_snapshot_positions \
         (snapshot_id, symbol, qty_signed, avg_entry_price_micros, provenance) \
         values ($1, 'AAPL', 25, 150000000, 'external_alpaca')",
    )
    .bind(sid)
    .execute(&pool)
    .await
    .expect("legacy insert");

    let fetched = fetch_paper_portfolio_snapshot_by_id(&pool, sid)
        .await
        .expect("fetch")
        .expect("exists");
    assert_eq!(fetched.positions.len(), 1);
    assert_eq!(fetched.positions[0].qty_signed, QtyMicros::new(25_000_000));
    cleanup(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn q05_check_constraint_rejects_mixed_or_mislabelled_encodings() {
    let Some(pool) = pool().await else { return };
    let run_id = id("q05-run");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;
    let sid = id("q05-snap");
    seed_snapshot_row(&pool, sid, run_id).await;

    // (qty_signed, qty_signed_micros, version)
    let bad: [(Option<i64>, Option<i64>, Option<&str>); 7] = [
        (Some(1), Some(1_000_000), None), // both quantities, no version
        (Some(1), Some(1_000_000), Some("qty_micros_v1")), // both quantities, versioned
        (Some(1), None, Some("qty_micros_v1")), // versioned but legacy value
        (None, None, Some("qty_micros_v1")), // versioned, no micros
        (None, Some(5), None),            // micros without a version
        (None, Some(5), Some("qty_micros_v2")), // unknown version
        (None, None, None),               // no quantity at all
    ];
    for (i, (legacy, micros, version)) in bad.iter().enumerate() {
        let res = sqlx::query(
            "insert into sys_paper_portfolio_snapshot_positions \
             (snapshot_id, symbol, qty_signed, qty_signed_micros, quantity_schema_version, \
              avg_entry_price_micros, provenance) \
             values ($1, $2, $3, $4, $5, 1, 'external_alpaca')",
        )
        .bind(sid)
        .bind(format!("BAD{i}"))
        .bind(legacy)
        .bind(micros)
        .bind(version)
        .execute(&pool)
        .await;
        assert!(res.is_err(), "case {i} {bad:?} must be rejected by CHECK");
    }
    cleanup(&pool, run_id).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn q06_a_corrupt_row_fails_the_read_closed() {
    let Some(pool) = pool().await else { return };
    let run_id = id("q06-run");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;
    let sid = id("q06-snap");
    seed_snapshot_row(&pool, sid, run_id).await;

    // Legacy whole quantity too large to express as micros: reading it must
    // fail rather than wrap.
    sqlx::query(
        "insert into sys_paper_portfolio_snapshot_positions \
         (snapshot_id, symbol, qty_signed, avg_entry_price_micros, provenance) \
         values ($1, 'HUGE', 9223372036854775807, 1, 'external_alpaca')",
    )
    .bind(sid)
    .execute(&pool)
    .await
    .expect("insert");

    let res = fetch_paper_portfolio_snapshot_by_id(&pool, sid).await;
    assert!(res.is_err(), "unrepresentable quantity must not decode");
    cleanup(&pool, run_id).await;
}
