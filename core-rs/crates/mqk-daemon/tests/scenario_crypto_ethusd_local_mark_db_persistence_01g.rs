//! CRYPTO-DATA-01G-ETHUSD-LOCAL-CSV-MARKS-01-COMBINED: second-symbol
//! DB-backed crypto mark persistence + readback proof.
//!
//! Continues `CRYPTO-DATA-01B-DB-BACKED-LOCAL-MARK-PERSISTENCE-01`'s DB proof
//! (`scenario_crypto_local_mark_db_persistence_01b.rs`) by proving the same
//! no-migration `md_bars` persistence/readback path, and the
//! `CRYPTO-DATA-01F` provider-metadata behavior, are not hardcoded to
//! `BTC/USD`:
//!
//! ```text
//! crypto_ethusd_1d_local.csv --(mqk_db::ingest_csv_to_md_bars, existing CLI-equivalent path)--> md_bars row (+ provider metadata)
//!     --(mqk_db::fetch_recent_completed_bars_for_strategy, existing read path)--> MdBarRow
//!     --(mqk_daemon::state::bridge_instrument_registry_v2_to_economics, 04B)--> InstrumentEconomics
//!     --(mqk_portfolio::value_position_economics, 04A)--> PositionEconomicsValue
//!     --(mqk_portfolio::aggregate_portfolio_economics, 04C)--> PortfolioEconomicsSnapshot
//! ```
//!
//! Unlike `scenario_crypto_local_mark_db_persistence_01b.rs` (which calls the
//! lower-level `mqk_db::ingest_provider_bars_to_md_bars` after a test-local
//! `RawBar` -> `ProviderBar` mapping), this test calls
//! `mqk_db::ingest_csv_to_md_bars` directly against the committed CSV file --
//! the same function `mqk-cli md ingest-csv` (and, transitively,
//! `Import-LocalCryptoMarks.ps1`) calls in production. This both simplifies
//! the test and proves `CRYPTO-DATA-01F`'s provider-metadata stamping
//! (`provider_id`/`provider_source` = the `--source` label,
//! `ingest_mode = "csv_import"`) applies unconditionally to any CSV file
//! ingested through that path, not specifically to `BTC/USD`.
//!
//! This is a DB-backed persistence/read-path proof only: no DB migration, no
//! provider/network call, no daemon runtime, no router, no order/risk path.
//! It does not touch `/api/v1/portfolio/economics/status` (ASSET-CORE-04D)
//! for the same structural reason `scenario_crypto_local_mark_db_persistence_01b.rs`
//! already documents.
//!
//! # Proof matrix
//!
//! | Test       | What it proves                                                                    |
//! |------------|------------------------------------------------------------------------------------|
//! | `db01`     | Existing `ingest_csv_to_md_bars` (the real CLI-equivalent path) inserts the ETH/USD CSV fixture's 3 rows into `md_bars`, zero rejects, fresh insert |
//! | `db02`     | `fetch_recent_completed_bars_for_strategy(pool, "ETH/USD", "1D", 1)` returns exactly the latest completed row |
//! | `db03`     | Latest completed close is exactly `3_200.00` in micros, matching the CSV fixture  |
//! | `db04`     | The `ETH/USD` slash symbol survives DB insert + readback exactly                  |
//! | `prov01`   | `provider_id`/`provider_source` == `"local_crypto_manual_ethusd_01g"`, `ingest_mode == "csv_import"` (CRYPTO-DATA-01F applies unmodified) |
//! | `chain_db01` | DB-read mark feeds the unmodified ASSET-CORE-04B bridge -> 04A valuation -> 04C aggregation -> `Active` |
//! | `chain_db02` | DB-read mark produces the expected `1 ETH` notional of `$3,200.00`               |
//! | `chain_db03` | Aggregation includes a `"crypto"` asset-class exposure bucket                    |
//! | `chain_db04` | Aggregation includes a `"USD"` currency exposure bucket                          |
//! | `safety01`   | The test makes zero writes to `oms_outbox`                                       |
//! | `cleanup01`  | Zero `ETH/USD` rows remain in `md_bars` after the test completes                 |
//!
//! DB-backed proof skips gracefully without `MQK_DATABASE_URL` pointing at the
//! local paper DB (port 5440 / `miniquantdesk_paper`), matching the exact
//! convention `scenario_crypto_local_mark_db_persistence_01b.rs` uses. No
//! broker adapter, provider, or network call is ever made.

use std::path::PathBuf;

use mqk_daemon::state::bridge_instrument_registry_v2_to_economics;
use mqk_md::instrument_registry_v2::{load_instrument_registry_v2, validate_registry_v2};
use mqk_portfolio::{
    aggregate_portfolio_economics, value_position_economics, InstrumentEconomicsTruthState,
    PortfolioEconomicsInput, PortfolioEconomicsTruthState, PositionEconomicsInput, MICROS_SCALE,
};
use uuid::Uuid;

const CSV_TIMEFRAME: &str = "1D";
const ETH_SYMBOL: &str = "ETH/USD";
const SOURCE_LABEL: &str = "local_crypto_manual_ethusd_01g";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn registry_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

fn eth_csv_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../mqk-md/tests/fixtures/crypto_ethusd_1d_local.csv")
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
        b"mqk:crypto-data-01g:ethusd:1d:db-persistence-test",
    )
}

async fn delete_eth_rows(pool: &sqlx::PgPool, timeframe: &str) {
    let _ = sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(ETH_SYMBOL)
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
/// `ETH/USD` row via the unmodified ASSET-CORE-04B bridge.
fn bridged_eth_economics() -> mqk_portfolio::InstrumentEconomics {
    let registry = load_instrument_registry_v2(&registry_fixture_path())
        .expect("crypto local-marks registry fixture must load");
    validate_registry_v2(&registry).expect("crypto local-marks registry fixture must validate");

    let summary = bridge_instrument_registry_v2_to_economics(&registry);
    summary
        .rows
        .iter()
        .find(|r| r.symbol == ETH_SYMBOL)
        .and_then(|r| r.economics.clone())
        .expect("ETH/USD must bridge to InstrumentEconomics")
}

// ---------------------------------------------------------------------------
// Combined sequential proof against the shared (ETH/USD, 1D) key. Mirrors
// scenario_crypto_local_mark_db_persistence_01b.rs's same-primary-key
// race-avoidance reasoning: independent #[tokio::test] functions that each
// delete-then-insert-then-assert against the same rows would race each other.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db_persistence_readback_provider_metadata_and_model_chain_proof() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("CRYPTO-DATA-01G: skipped (no MQK_DATABASE_URL pointing to paper DB)");
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
    delete_eth_rows(&pool, CSV_TIMEFRAME).await;
    delete_quality_report(&pool, ingest_id).await;

    let outbox_count_before: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await
        .expect("count oms_outbox before");

    // -----------------------------------------------------------------
    // db01: existing ingest_csv_to_md_bars (the real CLI-equivalent path,
    // including CRYPTO-DATA-01F provider-metadata stamping).
    // -----------------------------------------------------------------
    let ingest_result = mqk_db::ingest_csv_to_md_bars(
        &pool,
        mqk_db::IngestCsvArgs {
            path: eth_csv_fixture_path(),
            timeframe: CSV_TIMEFRAME.to_string(),
            source: SOURCE_LABEL.to_string(),
            ingest_id,
        },
    )
    .await
    .expect("db01: ingest_csv_to_md_bars must succeed");

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
        mqk_db::fetch_recent_completed_bars_for_strategy(&pool, ETH_SYMBOL, CSV_TIMEFRAME, 1)
            .await
            .expect("db02: fetch_recent_completed_bars_for_strategy must succeed");
    assert_eq!(
        recent.len(),
        1,
        "db02: limit=1 must return exactly one row: {recent:?}"
    );
    let latest = &recent[0];
    assert_eq!(
        latest.symbol, "ETH/USD",
        "db04: slash symbol must survive DB insert/readback exactly"
    );
    assert_eq!(latest.timeframe, CSV_TIMEFRAME);
    assert_eq!(
        latest.end_ts, 1_767_484_800,
        "db02: must be the fixture's latest row"
    );
    assert_eq!(
        latest.close_micros,
        3_200 * MICROS_SCALE,
        "db03: latest completed close must be exactly 3,200.00 in micros"
    );
    assert!(latest.is_complete);

    // -----------------------------------------------------------------
    // prov01: CRYPTO-DATA-01F provider metadata applies to this symbol too.
    // -----------------------------------------------------------------
    let provider_row: (String, Option<String>, Option<String>) = sqlx::query_as(
        r#"
        select provider_id, provider_source, ingest_mode
        from md_bars
        where symbol = $1 and timeframe = $2 and end_ts = $3
        "#,
    )
    .bind(ETH_SYMBOL)
    .bind(CSV_TIMEFRAME)
    .bind(1_767_484_800_i64)
    .fetch_one(&pool)
    .await
    .expect("prov01: fetch provider metadata row");
    assert_eq!(
        provider_row.0, SOURCE_LABEL,
        "prov01: provider_id must be the operator --source label, not 'unknown'"
    );
    assert_eq!(provider_row.1.as_deref(), Some(SOURCE_LABEL));
    assert_eq!(provider_row.2.as_deref(), Some("csv_import"));

    // -----------------------------------------------------------------
    // chain_db01-04: DB-read mark -> unmodified 04B/04A/04C chain.
    // -----------------------------------------------------------------
    let economics = bridged_eth_economics();
    let mark_price_micros = latest.close_micros;
    assert_eq!(mark_price_micros, 3_200 * MICROS_SCALE);

    let signed_qty_micros = MICROS_SCALE; // 1 ETH
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
        Some(3_200 * MICROS_SCALE as i128),
        "chain_db02: 1 ETH * $3,200.00 * 1.0x = $3,200.00"
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
        Some(cash_micros + 3_200 * MICROS_SCALE as i128)
    );
    assert_eq!(
        snapshot.gross_exposure_micros,
        Some(3_200 * MICROS_SCALE as i128)
    );

    let crypto_row = snapshot
        .asset_class_exposures
        .iter()
        .find(|r| r.key == "crypto")
        .expect("chain_db03: crypto asset-class exposure row must exist");
    assert_eq!(
        crypto_row.signed_notional_micros,
        3_200 * MICROS_SCALE as i128
    );
    assert_eq!(
        crypto_row.absolute_notional_micros,
        3_200 * MICROS_SCALE as i128
    );

    let usd_row = snapshot
        .currency_exposures
        .iter()
        .find(|r| r.key == "USD")
        .expect("chain_db04: USD currency exposure row must exist");
    assert_eq!(usd_row.signed_notional_micros, 3_200 * MICROS_SCALE as i128);
    assert_eq!(usd_row.absolute_notional_micros, 3_200 * MICROS_SCALE as i128);

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
    delete_eth_rows(&pool, CSV_TIMEFRAME).await;
    delete_quality_report(&pool, ingest_id).await;

    // -----------------------------------------------------------------
    // cleanup01: prove no leftover ETH/USD test rows remain.
    // -----------------------------------------------------------------
    let (remaining,): (i64,) =
        sqlx::query_as("select count(*)::bigint from md_bars where symbol = $1 and timeframe = $2")
            .bind(ETH_SYMBOL)
            .bind(CSV_TIMEFRAME)
            .fetch_one(&pool)
            .await
            .expect("cleanup01: count remaining ETH/USD rows");
    assert_eq!(
        remaining, 0,
        "cleanup01: zero ETH/USD rows must remain after this test"
    );
}
