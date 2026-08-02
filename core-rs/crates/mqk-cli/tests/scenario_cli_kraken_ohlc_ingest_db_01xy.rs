//! CRYPTO-DATA-01X-Y-KRAKEN-INGEST-PROVIDER-DB-PROOF-BUNDLE-01-COMBINED.
//!
//! Proves `mqk-cli md kraken-ohlc-ingest` (the fixture-first Kraken
//! provider-ingest path, distinct from the read-only
//! `kraken-ohlc-dry-run` proven by 01W) can safely reach the canonical
//! `md_bars` DB write path:
//!
//! - zero live network (fixture/`--input-file` only),
//! - completed Kraken bars land in `md_bars` with truthful provider
//!   metadata (`provider_id="kraken"`, `provider_source="kraken"`,
//!   `provider_symbol=<kraken result key>`, `ingest_mode="provider_ingest"`),
//! - the forming (not-yet-committed) candle is never written,
//! - Kraken's scaled (x1e8) fractional base-asset volume survives DB
//!   readback exactly,
//! - re-running the same ingest is idempotent (no duplicate rows),
//! - cleanup leaves zero leftover rows and touches zero `oms_outbox` rows.
//!
//! DB-backed: every test that touches Postgres is `#[ignore]`-gated and
//! requires `MQK_DATABASE_URL`, matching
//! `mqk-db/tests/scenario_md_ingest_provider.rs`'s convention. Run with:
//! `MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_ingest_db_01xy -- --include-ignored`
//!
//! Cleanup only ever deletes rows matching `provider_id = 'kraken'` at the
//! exact fixture `end_ts` values, so it can never touch pre-existing
//! non-Kraken history at the same canonical symbol/timeframe.

use std::path::PathBuf;
use std::process::Output;

use sqlx::PgPool;

// ---------------------------------------------------------------------------
// Fixture paths
// ---------------------------------------------------------------------------

fn registry_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

fn btc_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../mqk-md/tests/fixtures/kraken_ohlc_xbtusd_1d.json")
}

fn eth_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../mqk-md/tests/fixtures/kraken_ohlc_ethusd_1d.json")
}

// Both committed fixtures (kraken_ohlc_xbtusd_1d.json / kraken_ohlc_ethusd_1d.json)
// share the same time grid: 2 completed rows + 1 forming row.
const COMPLETED_END_TS: [i64; 2] = [1_783_123_200, 1_783_209_600];
const FORMING_END_TS: i64 = 1_783_296_000;

const BTC_LATEST_VOLUME_SCALED: i64 = 131_715_941_434; // "1317.15941434" * 1e8
const BTC_EARLIER_VOLUME_SCALED: i64 = 98_012_345_678; // "980.12345678" * 1e8
const ETH_LATEST_VOLUME_SCALED: i64 = 1_558_801_759_742; // "15588.01759742" * 1e8

const BTC_LATEST_CLOSE_MICROS: i64 = 63_085_800_000; // "63085.8"
const ETH_LATEST_CLOSE_MICROS: i64 = 1_778_720_000; // "1778.72"

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

async fn db_pool() -> anyhow::Result<PgPool> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(_) => mqk_db::testkit_db_pool().await,
        Err(_) => {
            panic!(
                "DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_ingest_db_01xy -- --include-ignored"
            );
        }
    }
}

/// Deletes only rows this test itself could have written: exact
/// (symbol, timeframe='1D', end_ts) keys AND provider_id='kraken'. Never
/// touches non-Kraken data at the same canonical symbol/timeframe.
async fn cleanup_kraken_rows(pool: &PgPool, symbol: &str) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        delete from md_bars
        where symbol = $1
          and timeframe = '1D'
          and provider_id = 'kraken'
          and end_ts = any($2)
        "#,
    )
    .bind(symbol)
    .bind(&[COMPLETED_END_TS[0], COMPLETED_END_TS[1], FORMING_END_TS][..])
    .execute(pool)
    .await?;
    Ok(())
}

async fn count_kraken_rows(pool: &PgPool, symbol: &str) -> anyhow::Result<i64> {
    let (cnt,): (i64,) = sqlx::query_as(
        r#"
        select count(*)::bigint
        from md_bars
        where symbol = $1
          and timeframe = '1D'
          and provider_id = 'kraken'
        "#,
    )
    .bind(symbol)
    .fetch_one(pool)
    .await?;
    Ok(cnt)
}

async fn row_exists(pool: &PgPool, symbol: &str, end_ts: i64) -> anyhow::Result<bool> {
    let (exists,): (bool,) = sqlx::query_as(
        r#"
        select exists(
          select 1 from md_bars
          where symbol = $1 and timeframe = '1D' and end_ts = $2 and provider_id = 'kraken'
        )
        "#,
    )
    .bind(symbol)
    .bind(end_ts)
    .fetch_one(pool)
    .await?;
    Ok(exists)
}

async fn oms_outbox_count(pool: &PgPool) -> anyhow::Result<i64> {
    let (cnt,): (i64,) = sqlx::query_as("select count(*)::bigint from oms_outbox")
        .fetch_one(pool)
        .await?;
    Ok(cnt)
}

#[allow(clippy::type_complexity)]
async fn read_bar_row(
    pool: &PgPool,
    symbol: &str,
    end_ts: i64,
) -> anyhow::Result<(
    i64,
    i64,
    bool,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
    let row: (
        i64,
        i64,
        bool,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        r#"
        select close_micros, volume, is_complete,
               provider_id, provider_source, provider_symbol, ingest_mode
        from md_bars
        where symbol = $1 and timeframe = '1D' and end_ts = $2
        "#,
    )
    .bind(symbol)
    .bind(end_ts)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// CLI helper
// ---------------------------------------------------------------------------

fn run_ingest(symbol: &str, input_file: &str, db_url: &str) -> Output {
    assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "md",
            "kraken-ohlc-ingest",
            "--registry",
            &registry_fixture_path().to_string_lossy(),
            "--symbol",
            symbol,
            "--timeframe",
            "1D",
            "--input-file",
            input_file,
        ])
        .env(mqk_db::ENV_DB_URL, db_url)
        .env_remove("MQK_ALLOW_KRAKEN_NETWORK_SMOKE")
        .output()
        .expect("failed to run mqk-cli")
}

// ---------------------------------------------------------------------------
// Fail-closed: no --input-file and no network opt-in refuses before any
// DB connection is attempted. Runs unconditionally -- no MQK_DATABASE_URL
// needed to prove this, and none is set.
// ---------------------------------------------------------------------------

#[test]
fn ki01_no_input_file_and_no_network_opt_in_refuses_without_db_env() {
    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "md",
            "kraken-ohlc-ingest",
            "--registry",
            &registry_fixture_path().to_string_lossy(),
            "--symbol",
            "BTC/USD",
            "--timeframe",
            "1D",
        ])
        .env_remove("MQK_ALLOW_KRAKEN_NETWORK_SMOKE")
        .env_remove("MQK_DATABASE_URL")
        .output()
        .expect("failed to run mqk-cli");

    assert!(
        !output.status.success(),
        "command must refuse without --input-file or the network opt-in env var"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("kraken_requires_input_file_or_network_opt_in"),
        "error must name the fail-closed reason: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// DB-backed: full ingest -> readback -> idempotency -> cleanup proof.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_ingest_db_01xy -- --include-ignored"]
async fn ki02_kraken_fixture_ingest_writes_completed_bars_with_truthful_metadata_and_cleans_up(
) -> anyhow::Result<()> {
    let pool = db_pool().await?;
    let db_url = std::env::var(mqk_db::ENV_DB_URL).expect("set, just checked by db_pool");

    // Setup: clear any leftover kraken-tagged rows from a prior failed run.
    cleanup_kraken_rows(&pool, "BTC/USD").await?;
    cleanup_kraken_rows(&pool, "ETH/USD").await?;

    let outbox_before = oms_outbox_count(&pool).await?;

    // --- BTC/USD ingest ---
    let btc_input = btc_fixture_path().to_string_lossy().to_string();
    let output = run_ingest("BTC/USD", &btc_input, &db_url);
    assert!(
        output.status.success(),
        "kraken-ohlc-ingest must succeed for BTC/USD\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(stdout.contains("network_call_made=false"));
    assert!(stdout.contains("db_write=true"));
    assert!(stdout.contains("md_bars_write=true"));
    assert!(stdout.contains("provider_id=kraken"));
    assert!(stdout.contains("provider_source=kraken"));
    assert!(stdout.contains("provider_symbol=XXBTZUSD"));
    assert!(stdout.contains("ingest_mode=provider_ingest"));
    assert!(stdout.contains("forming_candle_excluded=true"));
    assert!(stdout.contains("bars_completed=2"));
    assert!(stdout.contains("bars_excluded_forming=1"));
    assert!(stdout.contains("latest_completed_end_ts=1783209600"));
    assert!(
        stdout.contains("inserted=2"),
        "first ingest must insert both completed rows: {stdout}"
    );

    // Forming candle must never be written.
    assert!(
        !row_exists(&pool, "BTC/USD", FORMING_END_TS).await?,
        "forming candle end_ts must never appear in md_bars"
    );

    // Readback: latest completed row.
    let latest = read_bar_row(&pool, "BTC/USD", COMPLETED_END_TS[1]).await?;
    assert_eq!(latest.0, BTC_LATEST_CLOSE_MICROS, "close_micros readback");
    assert_eq!(latest.1, BTC_LATEST_VOLUME_SCALED, "scaled volume readback");
    assert!(latest.2, "is_complete must be true");
    assert_eq!(latest.3, "kraken");
    assert_eq!(latest.4.as_deref(), Some("kraken"));
    assert_eq!(latest.5.as_deref(), Some("XXBTZUSD"));
    assert_eq!(latest.6.as_deref(), Some("provider_ingest"));

    // Readback: earlier completed row.
    let earlier = read_bar_row(&pool, "BTC/USD", COMPLETED_END_TS[0]).await?;
    assert_eq!(
        earlier.1, BTC_EARLIER_VOLUME_SCALED,
        "earlier scaled volume readback"
    );
    assert!(earlier.2);

    assert_eq!(
        count_kraken_rows(&pool, "BTC/USD").await?,
        2,
        "exactly the 2 completed rows must be written, not the forming row"
    );

    // --- Idempotency: re-running the identical ingest must not duplicate rows ---
    let output2 = run_ingest("BTC/USD", &btc_input, &db_url);
    assert!(
        output2.status.success(),
        "second identical ingest must also succeed\nstderr:\n{}",
        String::from_utf8_lossy(&output2.stderr)
    );
    let stdout2 = String::from_utf8_lossy(&output2.stdout).to_string();
    assert!(
        stdout2.contains("updated=2"),
        "re-run must register as updates against existing rows, not new inserts: {stdout2}"
    );
    assert!(
        stdout2.contains("inserted=0"),
        "re-run must not insert new rows: {stdout2}"
    );
    assert_eq!(
        count_kraken_rows(&pool, "BTC/USD").await?,
        2,
        "idempotent re-run must not duplicate md_bars rows"
    );

    // --- ETH/USD ingest (proves the path is not hardcoded to one symbol) ---
    let eth_input = eth_fixture_path().to_string_lossy().to_string();
    let output_eth = run_ingest("ETH/USD", &eth_input, &db_url);
    assert!(
        output_eth.status.success(),
        "kraken-ohlc-ingest must succeed for ETH/USD\nstderr:\n{}",
        String::from_utf8_lossy(&output_eth.stderr)
    );
    let stdout_eth = String::from_utf8_lossy(&output_eth.stdout).to_string();
    assert!(stdout_eth.contains("provider_symbol=XETHZUSD"));
    assert!(stdout_eth.contains("bars_completed=2"));

    let eth_latest = read_bar_row(&pool, "ETH/USD", COMPLETED_END_TS[1]).await?;
    assert_eq!(eth_latest.0, ETH_LATEST_CLOSE_MICROS);
    assert_eq!(
        eth_latest.1, ETH_LATEST_VOLUME_SCALED,
        "ETH scaled volume readback"
    );
    assert_eq!(eth_latest.5.as_deref(), Some("XETHZUSD"));

    assert_eq!(count_kraken_rows(&pool, "ETH/USD").await?, 2);
    assert!(!row_exists(&pool, "ETH/USD", FORMING_END_TS).await?);

    // --- oms_outbox must be completely untouched by this ingest path ---
    let outbox_after = oms_outbox_count(&pool).await?;
    assert_eq!(
        outbox_before, outbox_after,
        "market-data ingest must never write to oms_outbox"
    );

    // --- Cleanup: prove zero leftover rows ---
    cleanup_kraken_rows(&pool, "BTC/USD").await?;
    cleanup_kraken_rows(&pool, "ETH/USD").await?;
    assert_eq!(
        count_kraken_rows(&pool, "BTC/USD").await?,
        0,
        "zero leftover BTC/USD rows"
    );
    assert_eq!(
        count_kraken_rows(&pool, "ETH/USD").await?,
        0,
        "zero leftover ETH/USD rows"
    );

    Ok(())
}
