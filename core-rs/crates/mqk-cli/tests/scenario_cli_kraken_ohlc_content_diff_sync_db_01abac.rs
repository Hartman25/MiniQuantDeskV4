//! CRYPTO-DATA-01AB-AC-KRAKEN-CONTENT-DIFF-SYNC-BUNDLE-01-COMBINED.
//!
//! Proves `mqk-cli md kraken-ohlc-sync`'s true content-diff sync semantics,
//! continuing from the presence-based proof in
//! `scenario_cli_kraken_ohlc_sync_db_01zaa.rs` (01Z-AA).
//!
//! 01Z-AA honestly documented that the write helper
//! (`ingest_provider_bars_to_md_bars_with_provider_metadata`) always
//! performs an unconditional `ON CONFLICT DO UPDATE`, so the sync command
//! could only classify by `end_ts` presence relative to a high-water mark --
//! it could not tell "existing and unchanged" from "existing and changed".
//! 01AB-AC closes that gap with a minimal read-only helper,
//! `mqk_db::md::fetch_md_bars_for_provider_sync_keys`, and a pure
//! comparison helper, `mqk_db::md::provider_bar_matches_existing`, that
//! compares OHLCV + `is_complete` + provider provenance
//! (`provider_id`/`provider_source`/`provider_symbol`/`ingest_mode`).
//!
//! This file proves, against a local test Postgres:
//! - fail-closed gate is unchanged (Test 1),
//! - a missing row is inserted, a changed row is updated, an unchanged row
//!   is truly skipped (never sent to the write helper) -- for both BTC/USD
//!   and ETH/USD,
//! - true idempotency: a second identical run performs zero inserts, zero
//!   updates, and does not call the write helper at all,
//! - `--no-update-existing` still detects a changed row but refuses to
//!   write it, distinctly counted from a truly-unchanged row,
//! - the forming candle is never written,
//! - scaled (x1e8) volume survives DB readback exactly,
//! - provider metadata is corrected to `kraken`/`kraken`/`XXBTZUSD`/
//!   `provider_sync` even when a stale seed used different values,
//! - `end_ts = row.time + 86400` (1D interval) for both fixtures,
//! - `oms_outbox` is never touched,
//! - cleanup proves zero leftover rows.
//!
//! DB-backed: every test that touches Postgres is `#[ignore]`-gated and
//! requires `MQK_DATABASE_URL`. Run with:
//! `MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_content_diff_sync_db_01abac -- --include-ignored`
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
const ROW_TIME_EARLIER: i64 = 1_783_036_800;
const ROW_TIME_LATEST: i64 = 1_783_123_200;
const INTERVAL_SECONDS_1D: i64 = 86_400;

const EARLIER_END_TS: i64 = 1_783_123_200;
const LATEST_END_TS: i64 = 1_783_209_600;
const FORMING_END_TS: i64 = 1_783_296_000;

const BTC_LATEST_VOLUME_SCALED: i64 = 131_715_941_434; // "1317.15941434" * 1e8
const BTC_EARLIER_VOLUME_SCALED: i64 = 98_012_345_678; // "980.12345678" * 1e8
const ETH_LATEST_VOLUME_SCALED: i64 = 1_558_801_759_742; // "15588.01759742" * 1e8

const BTC_LATEST_CLOSE_MICROS: i64 = 63_085_800_000; // "63085.8"
const ETH_LATEST_CLOSE_MICROS: i64 = 1_778_720_000; // "1778.72"

// Deliberately different from the fixture's earlier-row values, so the
// content-diff test proves the "existing row content differs -> update"
// half of the semantics (not just "already matches").
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
                "DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_content_diff_sync_db_01abac -- --include-ignored"
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

/// Seeds a single stale "earlier" completed Kraken row directly (bypassing
/// the CLI) with content that provably differs from the fixture (close,
/// volume, and provider_symbol), and metadata that otherwise matches what
/// `kraken-ohlc-sync` would write (provider_id/provider_source='kraken',
/// ingest_mode='provider_sync') so cleanup_kraken_rows can remove it like
/// any other row.
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
// Test 1 — fail closed before DB remains true. Runs unconditionally -- no
// MQK_DATABASE_URL needed to prove this, and none is set.
// ---------------------------------------------------------------------------

#[test]
fn kcd01_no_input_file_and_no_network_opt_in_refuses_without_db_env() {
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
// Test 2 — content-diff insert/update/true-skip proof for BTC/USD,
// --no-update-existing proof, ETH/USD proof, end_ts arithmetic, oms_outbox
// isolation, and cleanup.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_content_diff_sync_db_01abac -- --include-ignored"]
async fn kcd02_kraken_content_diff_insert_update_true_skip_and_cleans_up() -> anyhow::Result<()> {
    // end_ts is always row.time + interval (1D => 86400 seconds).
    assert_eq!(EARLIER_END_TS, ROW_TIME_EARLIER + INTERVAL_SECONDS_1D);
    assert_eq!(LATEST_END_TS, ROW_TIME_LATEST + INTERVAL_SECONDS_1D);

    let pool = db_pool().await?;
    let db_url = std::env::var(mqk_db::ENV_DB_URL).expect("set, just checked by db_pool");

    // Setup: clear any leftover kraken-tagged rows from a prior failed run.
    cleanup_kraken_rows(&pool, "BTC/USD").await?;
    cleanup_kraken_rows(&pool, "ETH/USD").await?;

    let outbox_before = oms_outbox_count(&pool).await?;

    // --- Seed: only the older completed BTC row exists before sync, with
    // stale content that provably differs from the fixture (close, volume,
    // provider_symbol). The newer completed row is left missing. ---
    seed_stale_earlier_row(&pool, "BTC/USD").await?;
    assert_eq!(
        count_kraken_rows(&pool, "BTC/USD").await?,
        1,
        "seed must write exactly one row"
    );
    let seeded = read_bar_row(&pool, "BTC/USD", EARLIER_END_TS).await?;
    assert_eq!(
        seeded.0, SEED_EARLIER_CLOSE_MICROS,
        "seed close_micros sanity check"
    );
    assert_eq!(
        seeded.1, SEED_EARLIER_VOLUME_SCALED,
        "seed volume sanity check"
    );

    // --- Sync (default content-diff policy) ---
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
    assert!(
        stdout.contains("sync_policy=content_diff_skip_unchanged_update_changed_insert_missing")
    );
    assert!(stdout.contains("provider_id=kraken"));
    assert!(stdout.contains("provider_source=kraken"));
    assert!(stdout.contains("provider_symbol=XXBTZUSD"));
    assert!(stdout.contains("ingest_mode=provider_sync"));
    assert!(stdout.contains("forming_candle_excluded=true"));
    assert!(stdout.contains("bars_completed=2"));
    assert!(stdout.contains("bars_excluded_forming=1"));
    assert!(stdout.contains(&format!("latest_existing_end_ts_before={EARLIER_END_TS}")));
    assert!(
        stdout.contains("bars_missing_new=1"),
        "only the newer row is missing/new: {stdout}"
    );
    assert!(
        stdout.contains("bars_existing_candidate=1"),
        "the seeded older row is an existing candidate: {stdout}"
    );
    assert!(
        stdout.contains("rows_changed=1"),
        "the seeded row's stale content must be detected as changed: {stdout}"
    );
    assert!(
        stdout.contains("rows_skipped_unchanged=0"),
        "nothing yet matches: {stdout}"
    );
    assert!(stdout.contains("rows_changed_skipped_due_to_no_update_existing=0"));
    assert!(
        stdout.contains("inserted=1"),
        "only the genuinely new (later) row must be inserted: {stdout}"
    );
    assert!(
        stdout.contains("updated=1"),
        "the changed (earlier) row must be updated: {stdout}"
    );

    // Forming candle must never be written.
    assert!(
        !row_exists(&pool, "BTC/USD", FORMING_END_TS).await?,
        "forming candle end_ts must never appear in md_bars"
    );

    // The stale seed row must now be overwritten with the fixture's real
    // values and corrected metadata.
    let earlier = read_bar_row(&pool, "BTC/USD", EARLIER_END_TS).await?;
    assert_eq!(
        earlier.1, BTC_EARLIER_VOLUME_SCALED,
        "stale seed volume must be corrected"
    );
    assert_ne!(
        earlier.0, SEED_EARLIER_CLOSE_MICROS,
        "stale seed close must not survive sync"
    );
    assert_eq!(earlier.3, "kraken", "provider_id corrected");
    assert_eq!(
        earlier.4.as_deref(),
        Some("kraken"),
        "provider_source corrected"
    );
    assert_eq!(
        earlier.5.as_deref(),
        Some("XXBTZUSD"),
        "provider_symbol corrected from seed_stale"
    );
    assert_eq!(
        earlier.6.as_deref(),
        Some("provider_sync"),
        "ingest_mode corrected"
    );

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

    // --- True idempotency: re-running the identical sync must detect both
    // rows as unchanged and must not call the write helper at all. ---
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
        "second run finds nothing new: {stdout2}"
    );
    assert!(
        stdout2.contains("rows_skipped_unchanged=2"),
        "second run's content now matches the fixture exactly for both rows: {stdout2}"
    );
    assert!(stdout2.contains("rows_changed=0"));
    assert!(
        stdout2.contains("inserted=0"),
        "re-run must not insert new rows: {stdout2}"
    );
    assert!(
        stdout2.contains("updated=0"),
        "re-run must not update unchanged rows under true content-diff: {stdout2}"
    );
    assert!(
        stdout2.contains("md_bars_write=false"),
        "nothing should be sent to the write helper once content matches: {stdout2}"
    );
    assert_eq!(
        count_kraken_rows(&pool, "BTC/USD").await?,
        2,
        "idempotent re-run must not duplicate md_bars rows"
    );

    // --- --no-update-existing: dirty the earlier row again, then prove the
    // conservative flag detects it as changed but refuses to write it. ---
    seed_stale_earlier_row(&pool, "BTC/USD").await?;
    let dirtied = read_bar_row(&pool, "BTC/USD", EARLIER_END_TS).await?;
    assert_eq!(
        dirtied.0, SEED_EARLIER_CLOSE_MICROS,
        "re-dirty sanity check"
    );

    let output3 = run_sync("BTC/USD", &btc_input, &db_url, true);
    assert!(
        output3.status.success(),
        "no-update-existing sync must also succeed\nstderr:\n{}",
        String::from_utf8_lossy(&output3.stderr)
    );
    let stdout3 = String::from_utf8_lossy(&output3.stdout).to_string();
    assert!(stdout3.contains("no_update_existing=true"));
    assert!(
        stdout3.contains("rows_changed=1"),
        "the re-dirtied earlier row must be classified as changed: {stdout3}"
    );
    assert!(
        stdout3.contains("rows_changed_skipped_due_to_no_update_existing=1"),
        "no-update-existing must count the changed row as skipped, not written: {stdout3}"
    );
    assert!(
        stdout3.contains("rows_skipped_unchanged=1"),
        "the latest row still matches and is skipped independently: {stdout3}"
    );
    assert!(stdout3.contains("bars_missing_new=0"));
    assert!(stdout3.contains("inserted=0"));
    assert!(stdout3.contains("updated=0"));
    assert!(
        stdout3.contains("md_bars_write=false"),
        "nothing was sent to the write helper: {stdout3}"
    );
    assert_eq!(
        count_kraken_rows(&pool, "BTC/USD").await?,
        2,
        "no-update-existing must not change row count"
    );
    let still_stale = read_bar_row(&pool, "BTC/USD", EARLIER_END_TS).await?;
    assert_eq!(
        still_stale.0, SEED_EARLIER_CLOSE_MICROS,
        "no-update-existing must leave the changed row's stale value in place"
    );
    assert_eq!(
        still_stale.1, SEED_EARLIER_VOLUME_SCALED,
        "no-update-existing must leave the changed row's stale volume in place"
    );

    // A default-policy run afterward must correct the still-stale row (proves
    // the flag is opt-in, not a persistent change to default behavior).
    let output3b = run_sync("BTC/USD", &btc_input, &db_url, false);
    assert!(output3b.status.success());
    let stdout3b = String::from_utf8_lossy(&output3b.stdout).to_string();
    assert!(stdout3b.contains("rows_changed=1"));
    assert!(stdout3b.contains("updated=1"));
    let corrected = read_bar_row(&pool, "BTC/USD", EARLIER_END_TS).await?;
    assert_ne!(
        corrected.0, SEED_EARLIER_CLOSE_MICROS,
        "default policy must correct the stale row"
    );
    assert_eq!(
        corrected.1, BTC_EARLIER_VOLUME_SCALED,
        "default policy must correct stale volume"
    );

    // --- ETH/USD path: both rows missing on first sync, then rerun proves
    // unchanged-skip; readback proves scaled volume exact. ---
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
    assert!(stdout_eth.contains("rows_changed=0"));
    assert!(stdout_eth.contains("rows_skipped_unchanged=0"));
    assert!(stdout_eth.contains("inserted=2"));
    assert!(stdout_eth.contains("updated=0"));

    let eth_latest = read_bar_row(&pool, "ETH/USD", LATEST_END_TS).await?;
    assert_eq!(eth_latest.0, ETH_LATEST_CLOSE_MICROS);
    assert_eq!(
        eth_latest.1, ETH_LATEST_VOLUME_SCALED,
        "ETH scaled volume readback"
    );
    assert_eq!(eth_latest.5.as_deref(), Some("XETHZUSD"));

    assert_eq!(count_kraken_rows(&pool, "ETH/USD").await?, 2);
    assert!(!row_exists(&pool, "ETH/USD", FORMING_END_TS).await?);

    // Rerun proves unchanged-skip for ETH/USD too.
    let output_eth2 = run_sync("ETH/USD", &eth_input, &db_url, false);
    assert!(output_eth2.status.success());
    let stdout_eth2 = String::from_utf8_lossy(&output_eth2.stdout).to_string();
    assert!(stdout_eth2.contains("bars_missing_new=0"));
    assert!(stdout_eth2.contains("rows_skipped_unchanged=2"));
    assert!(stdout_eth2.contains("inserted=0"));
    assert!(stdout_eth2.contains("updated=0"));
    assert!(stdout_eth2.contains("md_bars_write=false"));
    assert_eq!(count_kraken_rows(&pool, "ETH/USD").await?, 2);

    // --- oms_outbox must be completely untouched by this sync path ---
    let outbox_after = oms_outbox_count(&pool).await?;
    assert_eq!(
        outbox_before, outbox_after,
        "market-data sync must never write to oms_outbox"
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
