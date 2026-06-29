//! CRYPTO-DATA-01B-DB-BACKED-LOCAL-MARK-PERSISTENCE-01: DB-backed crypto mark
//! persistence + readback proof, continuing
//! `CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01`'s pure in-memory chain proof
//! (`scenario_crypto_portfolio_economics_local_marks_01a.rs`).
//!
//! Proves the same committed `BTC/USD` local CSV fixture this lane already
//! uses can be persisted into the existing `md_bars` schema and read back
//! through the existing symbol-keyed completed-bar lookup, then reach the
//! unmodified ASSET-CORE-04A/04B/04C model chain from a real DB-read mark
//! instead of an in-memory CSV-parsed one:
//!
//! ```text
//! crypto_btcusd_1d_local.csv --(mqk_md::ingest_csv::parse_csv_file)--> RawBar
//!     --(mqk_db::ingest_provider_bars_to_md_bars, existing upsert helper)--> md_bars row
//!     --(mqk_db::fetch_recent_completed_bars_for_strategy, existing read path)--> MdBarRow
//!     --(mqk_daemon::state::bridge_instrument_registry_v2_to_economics, 04B)--> InstrumentEconomics
//!     --(mqk_portfolio::value_position_economics, 04A)--> PositionEconomicsValue
//!     --(mqk_portfolio::aggregate_portfolio_economics, 04C)--> PortfolioEconomicsSnapshot
//! ```
//!
//! This is a DB-backed persistence/read-path proof only: no DB migration, no
//! provider/network call, no daemon runtime, no router, no order/risk path.
//! It does not touch `/api/v1/portfolio/economics/status` (ASSET-CORE-04D)
//! for the same structural reason `scenario_crypto_portfolio_economics_local_marks_01a.rs`
//! already documents: that route loads the v1 equity-only registry
//! (`config/instruments/equities.json`) and converts it to v2 via
//! `convert_v1_registry_to_v2`, which always stamps a plain `Equity`/`Etf`
//! contract regardless of the source row's `asset_class` string, so a
//! v1-sourced row can never carry a `CryptoPair` contract. A real crypto
//! position can only reach that specific route once a registry-v2-shaped
//! route-input seam exists; building that seam remains out of this patch's
//! scope.
//!
//! # Why two tests, not twelve
//!
//! `db01`-`db06`, `chain_db01`-`chain_db04`, and `safety01` all key off the
//! same `(symbol="BTC/USD", timeframe="1D")` row family, and `db05`/`db06`
//! must insert *extra* rows into that same family to prove the read path
//! filters them correctly. `cargo test` runs `#[tokio::test]` functions
//! concurrently by default within one binary; independent test functions
//! that each delete-then-insert-then-assert against the *same* primary-key
//! rows would race each other (mirroring the documented reason
//! `scenario_md_fetch_returns_ordered_rows.rs` already groups its own
//! multi-assertion DB proof into one test rather than several). All
//! shared-key proof points are therefore composed sequentially inside
//! [`db_persistence_readback_and_model_chain_proof`]. `db07` instead proves
//! the empty-result case, which intentionally needs the *opposite*
//! precondition (zero rows) and is given a private `timeframe` tag
//! (`1D_CRYPTO_DATA_01B_DB07_EMPTY`) that no other test in this file ever
//! touches, so it never races the combined test regardless of execution
//! order.
//!
//! # Proof matrix
//!
//! | Test         | What it proves                                                                    |
//! |--------------|------------------------------------------------------------------------------------|
//! | `db01`       | Existing `ingest_csv` parser + existing `ingest_provider_bars_to_md_bars` upsert helper insert the CSV fixture's 3 rows into `md_bars`, zero rejects, fresh insert |
//! | `db02`       | `fetch_recent_completed_bars_for_strategy(pool, "BTC/USD", "1D", 1)` returns exactly the latest completed row |
//! | `db03`       | Latest completed close is exactly `44_100.00` in micros, matching the CSV fixture |
//! | `db04`       | The `BTC/USD` slash symbol survives DB insert + readback exactly                  |
//! | `db05`       | An incomplete newer `BTC/USD` row is ignored by the completed-bar lookup          |
//! | `db06`       | An unrelated symbol's rows do not affect the `BTC/USD` lookup                     |
//! | `db07`       | Zero rows for a (symbol, timeframe) key returns an empty result, not a fabricated mark (isolated test, private key) |
//! | `chain_db01` | DB-read mark feeds the unmodified ASSET-CORE-04B bridge -> 04A valuation -> 04C aggregation -> `Active` |
//! | `chain_db02` | DB-read mark produces the expected `0.01 BTC` notional of `$441.00`              |
//! | `chain_db03` | Aggregation includes a `"crypto"` asset-class exposure bucket                    |
//! | `chain_db04` | Aggregation includes a `"USD"` currency exposure bucket                          |
//! | `safety01`   | The test makes zero writes to `oms_outbox`                                       |
//!
//! DB-backed tests skip gracefully without `MQK_DATABASE_URL` pointing at the
//! local paper DB (port 5440 / `miniquantdesk_paper`), matching the
//! convention in `scenario_portfolio_live_weights_01.rs` /
//! `scenario_portfolio_economics_status_asset_core_04d.rs`. No broker
//! adapter, provider, or network call is ever made.

use std::path::PathBuf;

use mqk_daemon::state::bridge_instrument_registry_v2_to_economics;
use mqk_md::ingest_csv::parse_csv_file;
use mqk_md::instrument_registry_v2::{load_instrument_registry_v2, validate_registry_v2};
use mqk_portfolio::{
    aggregate_portfolio_economics, value_position_economics, InstrumentEconomicsTruthState,
    PortfolioEconomicsInput, PortfolioEconomicsTruthState, PositionEconomicsInput, MICROS_SCALE,
};
use uuid::Uuid;

const CSV_TIMEFRAME: &str = "1D";
const BTC_SYMBOL: &str = "BTC/USD";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn registry_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

fn csv_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../mqk-md/tests/fixtures/crypto_btcusd_1d_local.csv")
}

fn get_paper_db_url() -> Option<String> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    if url.contains(":5440") || url.contains("miniquantdesk_paper") {
        Some(url)
    } else {
        None
    }
}

/// Deterministic (UUIDv5) ingest id so repeated test runs upsert the same
/// `md_quality_reports` row instead of accumulating orphans across runs.
fn deterministic_ingest_id() -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        b"mqk:crypto-data-01b:btcusd:1d:db-persistence-test",
    )
}

/// Field-for-field conversion from `mqk_md::provider::RawBar` (the existing
/// `ingest_csv` parser's output type) to `mqk_db::ProviderBar` (the existing
/// DB upsert helper's input type). Both crates define structurally identical
/// but nominally distinct bar types; no `From` impl exists between them, so
/// this test-local mapping is the minimal seam. Not a production helper.
fn raw_bar_to_db_provider_bar(b: mqk_md::provider::RawBar) -> mqk_db::ProviderBar {
    mqk_db::ProviderBar {
        symbol: b.symbol,
        timeframe: b.timeframe,
        end_ts: b.end_ts,
        open: b.open,
        high: b.high,
        low: b.low,
        close: b.close,
        volume: b.volume,
        is_complete: b.is_complete,
    }
}

async fn delete_btc_rows(pool: &sqlx::PgPool, timeframe: &str) {
    let _ = sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(BTC_SYMBOL)
        .bind(timeframe)
        .execute(pool)
        .await;
}

async fn delete_quality_report(pool: &sqlx::PgPool, ingest_id: Uuid) {
    let _ = sqlx::query("delete from md_quality_reports where ingest_id = $1")
        .bind(ingest_id)
        .execute(pool)
        .await;
}

/// Load + validate the real (disabled) registry-v2 fixture and bridge its
/// `BTC/USD` row via the unmodified ASSET-CORE-04B bridge. Shared setup,
/// mirroring `scenario_crypto_portfolio_economics_local_marks_01a.rs`.
fn bridged_btc_economics() -> mqk_portfolio::InstrumentEconomics {
    let registry = load_instrument_registry_v2(&registry_fixture_path())
        .expect("crypto local-marks registry fixture must load");
    validate_registry_v2(&registry).expect("crypto local-marks registry fixture must validate");

    let summary = bridge_instrument_registry_v2_to_economics(&registry);
    summary
        .rows
        .iter()
        .find(|r| r.symbol == BTC_SYMBOL)
        .and_then(|r| r.economics.clone())
        .expect("BTC/USD must bridge to InstrumentEconomics")
}

// ---------------------------------------------------------------------------
// db01-db06 / chain_db01-04 / safety01: combined sequential proof against
// the shared (BTC/USD, 1D) key. See module docs for why this is one test.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db_persistence_readback_and_model_chain_proof() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("CRYPTO-DATA-01B: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let ingest_id = deterministic_ingest_id();

    // Clean slate (covers a prior interrupted run as well as this run's own end).
    delete_btc_rows(&pool, CSV_TIMEFRAME).await;
    delete_quality_report(&pool, ingest_id).await;

    let outbox_count_before: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("count oms_outbox before");

    // -----------------------------------------------------------------
    // db01: existing ingest_csv parser -> existing DB upsert helper.
    // -----------------------------------------------------------------
    let raw_bars = parse_csv_file(&csv_fixture_path(), CSV_TIMEFRAME)
        .expect("crypto CSV fixture must parse via the existing ingest_csv parser");
    assert_eq!(
        raw_bars.len(),
        3,
        "fixture must contain exactly 3 daily bars"
    );

    let provider_bars: Vec<mqk_db::ProviderBar> = raw_bars
        .into_iter()
        .map(raw_bar_to_db_provider_bar)
        .collect();

    let ingest_result = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "crypto_local_csv_test".to_string(),
            timeframe: CSV_TIMEFRAME.to_string(),
            ingest_id,
            bars: provider_bars,
        },
    )
    .await
    .expect("db01: ingest_provider_bars_to_md_bars must succeed");

    let coverage = &ingest_result.report.coverage;
    assert_eq!(coverage.rows_read, 3, "db01: coverage={coverage:?}");
    assert_eq!(coverage.rows_ok, 3, "db01: coverage={coverage:?}");
    assert_eq!(coverage.rows_rejected, 0, "db01: coverage={coverage:?}");
    assert_eq!(
        coverage.rows_inserted, 3,
        "db01: clean slate means all 3 rows must be fresh inserts: coverage={coverage:?}"
    );
    assert_eq!(coverage.rows_updated, 0, "db01: coverage={coverage:?}");

    // -----------------------------------------------------------------
    // db02/db03/db04: existing completed-bar read path.
    // -----------------------------------------------------------------
    let recent =
        mqk_db::fetch_recent_completed_bars_for_strategy(&pool, BTC_SYMBOL, CSV_TIMEFRAME, 1)
            .await
            .expect("db02: fetch_recent_completed_bars_for_strategy must succeed");
    assert_eq!(
        recent.len(),
        1,
        "db02: limit=1 must return exactly one row: {recent:?}"
    );
    let latest = &recent[0];
    assert_eq!(
        latest.symbol, "BTC/USD",
        "db04: slash symbol must survive DB insert/readback exactly"
    );
    assert_eq!(latest.timeframe, CSV_TIMEFRAME);
    assert_eq!(
        latest.end_ts, 1_767_484_800,
        "db02: must be the fixture's latest row"
    );
    assert_eq!(
        latest.close_micros,
        44_100 * MICROS_SCALE,
        "db03: latest completed close must be exactly 44,100.00 in micros"
    );
    assert!(latest.is_complete);

    // -----------------------------------------------------------------
    // db05: an incomplete newer row must be ignored by the lookup.
    // -----------------------------------------------------------------
    let newer_incomplete_end_ts = 1_767_571_200_i64; // fixture's latest end_ts + 1 day
    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros,
          volume, is_complete
        ) values ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        on conflict (symbol, timeframe, end_ts) do update set
          open_micros = excluded.open_micros,
          high_micros = excluded.high_micros,
          low_micros = excluded.low_micros,
          close_micros = excluded.close_micros,
          volume = excluded.volume,
          is_complete = excluded.is_complete
        "#,
    )
    .bind(BTC_SYMBOL)
    .bind(CSV_TIMEFRAME)
    .bind(newer_incomplete_end_ts)
    .bind(50_000 * MICROS_SCALE)
    .bind(51_000 * MICROS_SCALE)
    .bind(49_000 * MICROS_SCALE)
    .bind(50_000 * MICROS_SCALE)
    .bind(1_i64)
    .bind(false)
    .execute(&pool)
    .await
    .expect("db05: insert incomplete newer probe row");

    let recent_after_incomplete =
        mqk_db::fetch_recent_completed_bars_for_strategy(&pool, BTC_SYMBOL, CSV_TIMEFRAME, 1)
            .await
            .expect("db05: fetch must succeed");
    assert_eq!(
        recent_after_incomplete.len(),
        1,
        "db05: incomplete newer row must not appear: {recent_after_incomplete:?}"
    );
    assert_eq!(
        recent_after_incomplete[0].end_ts, 1_767_484_800,
        "db05: latest COMPLETED row must remain the fixture's last row, not the newer incomplete one"
    );
    assert_eq!(
        recent_after_incomplete[0].close_micros,
        44_100 * MICROS_SCALE
    );

    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2 and end_ts = $3")
        .bind(BTC_SYMBOL)
        .bind(CSV_TIMEFRAME)
        .bind(newer_incomplete_end_ts)
        .execute(&pool)
        .await
        .expect("db05 cleanup: delete incomplete probe row");

    // -----------------------------------------------------------------
    // db06: an unrelated symbol's rows must not affect the BTC/USD lookup.
    // -----------------------------------------------------------------
    let unrelated_symbol = "ZZCRYPTODB06UNRELATED";
    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros,
          volume, is_complete
        ) values ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        on conflict (symbol, timeframe, end_ts) do update set
          close_micros = excluded.close_micros,
          is_complete = excluded.is_complete
        "#,
    )
    .bind(unrelated_symbol)
    .bind(CSV_TIMEFRAME)
    .bind(1_767_484_800_i64)
    .bind(MICROS_SCALE)
    .bind(MICROS_SCALE)
    .bind(MICROS_SCALE)
    .bind(MICROS_SCALE)
    .bind(1_i64)
    .bind(true)
    .execute(&pool)
    .await
    .expect("db06: insert unrelated-symbol probe row");

    let recent_after_unrelated =
        mqk_db::fetch_recent_completed_bars_for_strategy(&pool, BTC_SYMBOL, CSV_TIMEFRAME, 1)
            .await
            .expect("db06: fetch must succeed");
    assert_eq!(recent_after_unrelated.len(), 1);
    assert_eq!(
        recent_after_unrelated[0].symbol, "BTC/USD",
        "db06: unrelated symbol must not leak into BTC/USD lookup"
    );
    assert_eq!(
        recent_after_unrelated[0].close_micros,
        44_100 * MICROS_SCALE
    );

    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(unrelated_symbol)
        .bind(CSV_TIMEFRAME)
        .execute(&pool)
        .await
        .expect("db06 cleanup: delete unrelated-symbol probe row");

    // -----------------------------------------------------------------
    // chain_db01-04: DB-read mark -> unmodified 04B/04A/04C chain.
    // -----------------------------------------------------------------
    let economics = bridged_btc_economics();
    let mark_price_micros = latest.close_micros;
    assert_eq!(mark_price_micros, 44_100 * MICROS_SCALE);

    let signed_qty_micros = MICROS_SCALE / 100; // 0.01 BTC
    let position = value_position_economics(PositionEconomicsInput {
        instrument: economics,
        signed_qty_micros,
        mark_price_micros: Some(mark_price_micros),
        account_currency: "USD".to_string(),
    });
    assert_eq!(
        position.truth_state,
        InstrumentEconomicsTruthState::Active,
        "chain_db01: position={position:?}"
    );
    assert_eq!(
        position.notional_micros,
        Some(441 * MICROS_SCALE as i128),
        "chain_db02: 0.01 BTC * $44,100.00 * 1.0x = $441.00"
    );

    let cash_micros: i128 = 100_000 * MICROS_SCALE as i128;
    let snapshot = aggregate_portfolio_economics(PortfolioEconomicsInput {
        cash_micros,
        account_currency: "USD".to_string(),
        positions: vec![position],
    });
    assert_eq!(
        snapshot.truth_state,
        PortfolioEconomicsTruthState::Active,
        "chain_db01: snapshot={snapshot:?}"
    );
    assert_eq!(
        snapshot.nav_micros,
        Some(cash_micros + 441 * MICROS_SCALE as i128)
    );
    assert_eq!(
        snapshot.gross_exposure_micros,
        Some(441 * MICROS_SCALE as i128)
    );

    let crypto_row = snapshot
        .asset_class_exposures
        .iter()
        .find(|r| r.key == "crypto")
        .expect("chain_db03: crypto asset-class exposure row must exist");
    assert_eq!(
        crypto_row.signed_notional_micros,
        441 * MICROS_SCALE as i128
    );
    assert_eq!(
        crypto_row.absolute_notional_micros,
        441 * MICROS_SCALE as i128
    );

    let usd_row = snapshot
        .currency_exposures
        .iter()
        .find(|r| r.key == "USD")
        .expect("chain_db04: USD currency exposure row must exist");
    assert_eq!(usd_row.signed_notional_micros, 441 * MICROS_SCALE as i128);
    assert_eq!(usd_row.absolute_notional_micros, 441 * MICROS_SCALE as i128);

    // -----------------------------------------------------------------
    // safety01: this entire test must never write to oms_outbox.
    // -----------------------------------------------------------------
    let outbox_count_after: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("count oms_outbox after");
    assert_eq!(
        outbox_count_before, outbox_count_after,
        "safety01: this test must never write to oms_outbox"
    );

    // Final cleanup — leave the paper DB exactly as found.
    delete_btc_rows(&pool, CSV_TIMEFRAME).await;
    delete_quality_report(&pool, ingest_id).await;
}

// ---------------------------------------------------------------------------
// db07: absent key returns an empty result, not a fabricated mark. Isolated
// on a private timeframe tag so it cannot race the combined test above.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db07_absent_key_returns_empty_completed_bars_result() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("CRYPTO-DATA-01B db07: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let private_timeframe = "1D_CRYPTO_DATA_01B_DB07_EMPTY";
    delete_btc_rows(&pool, private_timeframe).await; // ensure zero rows exist for this private key

    let recent =
        mqk_db::fetch_recent_completed_bars_for_strategy(&pool, BTC_SYMBOL, private_timeframe, 1)
            .await
            .expect("db07: fetch must succeed even with zero rows");
    assert!(
        recent.is_empty(),
        "db07: no rows must produce an empty result, not a fabricated mark: {recent:?}"
    );

    delete_btc_rows(&pool, private_timeframe).await; // hygiene
}
