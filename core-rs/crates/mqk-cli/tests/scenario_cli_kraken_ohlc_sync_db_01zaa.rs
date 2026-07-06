//! CRYPTO-DATA-01Z-AA-KRAKEN-INCREMENTAL-SYNC-DB-PROOF-BUNDLE-01-COMBINED.
//!
//! Proves `mqk-cli md kraken-ohlc-sync` (the incremental-sync-flavored
//! sibling of `kraken-ohlc-ingest`, closed by 01X-Y) can safely:
//!
//! - fail closed before any DB connection when neither `--input-file` nor
//!   the network opt-in env var is present,
//! - read the pre-sync latest stored `end_ts` for a symbol/timeframe before
//!   writing anything,
//! - insert a genuinely missing/new completed row while leaving a
//!   previously-stored row's presence intact (updated in place under the
//!   default policy -- see the honest-limitation note below),
//! - never write the forming (not-yet-committed) candle,
//! - preserve Kraken's scaled (x1e8) fractional base-asset volume across
//!   readback exactly,
//! - re-run idempotently (no duplicate rows),
//! - leave zero `oms_outbox` side effects,
//! - clean up to zero leftover rows.
//!
//! Honest limitation proven, not hidden: the underlying
//! `ingest_provider_bars_to_md_bars_with_provider_metadata` helper (reused
//! unchanged from `kraken-ohlc-ingest`) has no row-content-diff "skip if
//! unchanged" capability. Under the default policy this command upserts
//! every completed bar exactly like `kraken-ohlc-ingest` -- a previously
//! stored row is always updated, never skipped by value-equality. The
//! `--no-update-existing` flag proves a real, provable-by-presence skip
//! instead (any completed bar whose `end_ts` was already <= the pre-sync
//! high-water mark is never sent to the write helper).
//!
//! DB-backed: every test that touches Postgres is `#[ignore]`-gated and
//! requires `MQK_DATABASE_URL`, matching
//! `scenario_cli_kraken_ohlc_ingest_db_01xy.rs`'s convention. Run with:
//! `MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_sync_db_01zaa -- --include-ignored`
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
const EARLIER_END_TS: i64 = 1_783_123_200;
const LATEST_END_TS: i64 = 1_783_209_600;
const FORMING_END_TS: i64 = 1_783_296_000;

const BTC_LATEST_VOLUME_SCALED: i64 = 131_715_941_434; // "1317.15941434" * 1e8
const BTC_EARLIER_VOLUME_SCALED: i64 = 98_012_345_678; // "980.12345678" * 1e8 (fixture's earlier row)
const ETH_LATEST_VOLUME_SCALED: i64 = 1_558_801_759_742; // "15588.01759742" * 1e8

const BTC_LATEST_CLOSE_MICROS: i64 = 63_085_800_000; // "63085.8"
const ETH_LATEST_CLOSE_MICROS: i64 = 1_778_720_000; // "1778.72"

// Deliberately different from the fixture's earlier-row values, so the
// default-policy test proves the "existing row gets updated" half of the
// documented sync semantics (not just "already matches").
const SEED_EARLIER_CLOSE_MICROS: i64 = 60_000_000_000; // "60000.0" -- stale seed value
const SEED_EARLIER_VOLUME_SCALED: i64 = 1_000_000_000; // "10.0" * 1e8 -- stale seed value

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

async fn db_pool() -> anyhow::Result<PgPool> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(_) => mqk_db::testkit_db_pool().await,
        Err(_) => {
            panic!(
                "DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_sync_db_01zaa -- --include-ignored"
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
    .bind(&[EARLIER_END_TS, LATEST_END_TS, FORMING_END_TS][..])
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
) -> anyhow::Result<(i64, i64, bool, String, Option<String>, Option<String>, Option<String>)> {
    let row: (i64, i64, bool, String, Option<String>, Option<String>, Option<String>) =
        sqlx::query_as(
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

/// Seeds a single stale "earlier" completed Kraken row directly (bypassing
/// the CLI) so the sync test can prove the default policy's "existing row
/// gets updated" behavior against a value that provably differs from the
/// fixture, not just re-writing an identical value. Mirrors the exact
/// column set `ingest_provider_bars_to_md_bars_with_provider_metadata`
/// writes, so cleanup_kraken_rows can remove it like any other row.
async fn seed_stale_earlier_row(pool: &PgPool, symbol: &str) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts,
          open_micros, high_micros, low_micros, close_micros,
          volume, is_complete,
          provider_id, provider_source, provider_symbol, ingest_mode,
          provider_bar_id, provider_updated_at_utc
        ) values ($1,'1D',$2,$3,$3,$3,$3,$4,true,'kraken','kraken','seed_stale','provider_sync',null,null)
        on conflict (symbol, timeframe, end_ts) do update set
          close_micros = excluded.close_micros,
          volume = excluded.volume,
          provider_symbol = excluded.provider_symbol
        "#,
    )
    .bind(symbol)
    .bind(EARLIER_END_TS)
    .bind(SEED_EARLIER_CLOSE_MICROS)
    .bind(SEED_EARLIER_VOLUME_SCALED)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// CLI helper
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_sync(symbol: &str, input_file: &str, db_url: &str, no_update_existing: bool) -> Output {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli");
    cmd.args([
        "md",
        "kraken-ohlc-sync",
        "--registry",
        &registry_fixture_path().to_string_lossy(),
        "--symbol",
        symbol,
        "--timeframe",
        "1D",
        "--input-file",
        input_file,
    ]);
    if no_update_existing {
        cmd.arg("--no-update-existing");
    }
    cmd.env(mqk_db::ENV_DB_URL, db_url)
        .env_remove("MQK_ALLOW_KRAKEN_NETWORK_SMOKE")
        .output()
        .expect("failed to run mqk-cli")
}

// ---------------------------------------------------------------------------
// Test 1 — fail closed before DB. Runs unconditionally -- no
// MQK_DATABASE_URL needed to prove this, and none is set.
// ---------------------------------------------------------------------------

#[test]
fn ksz01_no_input_file_and_no_network_opt_in_refuses_without_db_env() {
    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "md",
            "kraken-ohlc-sync",
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
        stderr.contains("kraken_sync_requires_input_file_or_network_opt_in"),
        "error must name the sync-specific fail-closed reason: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — incremental insert/update/skip proof, idempotency, cleanup.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_sync_db_01zaa -- --include-ignored"]
async fn ksz02_kraken_fixture_sync_inserts_missing_updates_existing_and_cleans_up(
) -> anyhow::Result<()> {
    let pool = db_pool().await?;
    let db_url = std::env::var(mqk_db::ENV_DB_URL).expect("set, just checked by db_pool");

    // Setup: clear any leftover kraken-tagged rows from a prior failed run.
    cleanup_kraken_rows(&pool, "BTC/USD").await?;
    cleanup_kraken_rows(&pool, "ETH/USD").await?;

    let outbox_before = oms_outbox_count(&pool).await?;

    // --- Seed: only the older completed BTC row exists before sync, with
    // stale values that provably differ from the fixture. ---
    seed_stale_earlier_row(&pool, "BTC/USD").await?;
    assert_eq!(count_kraken_rows(&pool, "BTC/USD").await?, 1, "seed must write exactly one row");
    let seeded = read_bar_row(&pool, "BTC/USD", EARLIER_END_TS).await?;
    assert_eq!(seeded.0, SEED_EARLIER_CLOSE_MICROS, "seed close_micros sanity check");
    assert_eq!(seeded.1, SEED_EARLIER_VOLUME_SCALED, "seed volume sanity check");

    // --- Sync (default policy: upsert every completed bar) ---
    let btc_input = btc_fixture_path().to_string_lossy().to_string();
    let output = run_sync("BTC/USD", &btc_input, &db_url, false);
    assert!(
        output.status.success(),
        "kraken-ohlc-sync must succeed for BTC/USD\nstdout:\n{}\nstderr:\n{}",
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
    assert!(stdout.contains("ingest_mode=provider_sync"));
    assert!(stdout.contains("forming_candle_excluded=true"));
    assert!(stdout.contains("bars_completed=2"));
    assert!(stdout.contains("bars_excluded_forming=1"));
    assert!(stdout.contains(&format!("latest_existing_end_ts_before={EARLIER_END_TS}")));
    assert!(stdout.contains("bars_missing_new=1"), "only the newer row is missing/new: {stdout}");
    assert!(
        stdout.contains("bars_existing_candidate=1"),
        "the seeded older row is an existing candidate: {stdout}"
    );
    assert!(stdout.contains("rows_skipped_if_known=0"), "default policy never skips: {stdout}");
    assert!(
        stdout.contains("inserted=1"),
        "only the genuinely new (later) row must be inserted: {stdout}"
    );
    assert!(
        stdout.contains("updated=1"),
        "the pre-existing (earlier) row must be updated, not re-inserted: {stdout}"
    );

    // Forming candle must never be written.
    assert!(
        !row_exists(&pool, "BTC/USD", FORMING_END_TS).await?,
        "forming candle end_ts must never appear in md_bars"
    );

    // The stale seed row must now be overwritten with the fixture's real
    // values -- proving "existing -> updated" under the default policy.
    let earlier = read_bar_row(&pool, "BTC/USD", EARLIER_END_TS).await?;
    assert_eq!(earlier.1, BTC_EARLIER_VOLUME_SCALED, "stale seed volume must be corrected");
    assert_ne!(earlier.0, SEED_EARLIER_CLOSE_MICROS, "stale seed close must not survive sync");
    assert_eq!(earlier.6.as_deref(), Some("provider_sync"), "sync-written rows carry provider_sync");

    // Readback: latest completed row (the newly inserted one).
    let latest = read_bar_row(&pool, "BTC/USD", LATEST_END_TS).await?;
    assert_eq!(latest.0, BTC_LATEST_CLOSE_MICROS, "close_micros readback");
    assert_eq!(latest.1, BTC_LATEST_VOLUME_SCALED, "scaled volume readback");
    assert!(latest.2, "is_complete must be true");
    assert_eq!(latest.3, "kraken");
    assert_eq!(latest.4.as_deref(), Some("kraken"));
    assert_eq!(latest.5.as_deref(), Some("XXBTZUSD"));
    assert_eq!(latest.6.as_deref(), Some("provider_sync"));

    assert_eq!(
        count_kraken_rows(&pool, "BTC/USD").await?,
        2,
        "exactly the 2 completed rows must be present, not the forming row"
    );

    // --- Idempotency: re-running the identical sync must not duplicate rows ---
    let output2 = run_sync("BTC/USD", &btc_input, &db_url, false);
    assert!(
        output2.status.success(),
        "second identical sync must also succeed\nstderr:\n{}",
        String::from_utf8_lossy(&output2.stderr)
    );
    let stdout2 = String::from_utf8_lossy(&output2.stdout).to_string();
    assert!(
        stdout2.contains(&format!("latest_existing_end_ts_before={LATEST_END_TS}")),
        "second run must observe the first run's high-water mark: {stdout2}"
    );
    assert!(
        stdout2.contains("bars_missing_new=0"),
        "second run finds nothing new (both rows already stored): {stdout2}"
    );
    assert!(
        stdout2.contains("inserted=0"),
        "re-run must not insert new rows: {stdout2}"
    );
    assert!(
        stdout2.contains("updated=2"),
        "re-run must register as updates against existing rows under the default policy: {stdout2}"
    );
    assert_eq!(
        count_kraken_rows(&pool, "BTC/USD").await?,
        2,
        "idempotent re-run must not duplicate md_bars rows"
    );

    // --- --no-update-existing proves a real, provable-by-presence skip:
    // a third run with the conservative flag must skip both already-stored
    // rows and write nothing. ---
    let output3 = run_sync("BTC/USD", &btc_input, &db_url, true);
    assert!(
        output3.status.success(),
        "no-update-existing sync must also succeed\nstderr:\n{}",
        String::from_utf8_lossy(&output3.stderr)
    );
    let stdout3 = String::from_utf8_lossy(&output3.stdout).to_string();
    assert!(stdout3.contains("no_update_existing=true"));
    assert!(
        stdout3.contains("rows_skipped_if_known=2"),
        "no-update-existing must skip both already-stored rows: {stdout3}"
    );
    assert!(stdout3.contains("inserted=0"));
    assert!(stdout3.contains("updated=0"));
    assert!(stdout3.contains("md_bars_write=false"), "nothing was sent to the write helper: {stdout3}");
    assert_eq!(
        count_kraken_rows(&pool, "BTC/USD").await?,
        2,
        "no-update-existing must not change row count"
    );

    // --- ETH/USD sync (proves the path is not hardcoded to one symbol) ---
    let eth_input = eth_fixture_path().to_string_lossy().to_string();
    let output_eth = run_sync("ETH/USD", &eth_input, &db_url, false);
    assert!(
        output_eth.status.success(),
        "kraken-ohlc-sync must succeed for ETH/USD\nstderr:\n{}",
        String::from_utf8_lossy(&output_eth.stderr)
    );
    let stdout_eth = String::from_utf8_lossy(&output_eth.stdout).to_string();
    assert!(stdout_eth.contains("provider_symbol=XETHZUSD"));
    assert!(stdout_eth.contains("bars_completed=2"));
    assert!(
        stdout_eth.contains("bars_missing_new=2"),
        "ETH/USD had no prior rows, both are missing/new: {stdout_eth}"
    );
    assert!(stdout_eth.contains("inserted=2"));
    assert!(stdout_eth.contains("updated=0"));

    let eth_latest = read_bar_row(&pool, "ETH/USD", LATEST_END_TS).await?;
    assert_eq!(eth_latest.0, ETH_LATEST_CLOSE_MICROS);
    assert_eq!(eth_latest.1, ETH_LATEST_VOLUME_SCALED, "ETH scaled volume readback");
    assert_eq!(eth_latest.5.as_deref(), Some("XETHZUSD"));

    assert_eq!(count_kraken_rows(&pool, "ETH/USD").await?, 2);
    assert!(!row_exists(&pool, "ETH/USD", FORMING_END_TS).await?);

    // --- oms_outbox must be completely untouched by this sync path ---
    let outbox_after = oms_outbox_count(&pool).await?;
    assert_eq!(
        outbox_before, outbox_after,
        "market-data sync must never write to oms_outbox"
    );

    // --- Cleanup: prove zero leftover rows ---
    cleanup_kraken_rows(&pool, "BTC/USD").await?;
    cleanup_kraken_rows(&pool, "ETH/USD").await?;
    assert_eq!(count_kraken_rows(&pool, "BTC/USD").await?, 0, "zero leftover BTC/USD rows");
    assert_eq!(count_kraken_rows(&pool, "ETH/USD").await?, 0, "zero leftover ETH/USD rows");

    Ok(())
}
