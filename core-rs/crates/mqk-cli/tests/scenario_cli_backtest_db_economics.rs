//! BACKTEST-ECONOMICS-DB-CLI-ENTRY-01: extends the proven `mqk backtest csv`
//! economics flags (see `scenario_cli_backtest_economics.rs`) to
//! `mqk backtest db`. Same three opt-in flags
//! (--contract-multiplier/--initial-margin-micros/--maintenance-margin-micros),
//! same default-preserving/fail-closed semantics, now reachable from a
//! Postgres-backed `md_bars` run instead of only a CSV file.
//!
//! The fail-closed tests (zero/negative multiplier) require no DB at all --
//! `run_backtest_db` validates economics flags before connecting to Postgres,
//! so they run unconditionally. The default/multiplier/margin tests are
//! DB-backed and skip gracefully without `MQK_DATABASE_URL`, matching the
//! existing `scenario_cli_db_migrate_requires_yes_when_live_active.rs` pattern.

use std::path::PathBuf;
use std::process::Output;

use uuid::Uuid;

/// Unique per-test symbol so parallel test runs never collide on the same
/// `md_bars` rows, and cleanup never touches another test's data.
fn unique_symbol(tag: &str) -> String {
    format!("ZZBEDBCLI{}{}", tag, Uuid::new_v4().simple())
}

/// Fresh, never-created output directory path for a single test run.
fn fresh_out_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mqk_cli_bkt_db_econ_out_{}_{}", tag, Uuid::new_v4()))
}

/// Same three bars as `scenario_cli_backtest_economics.rs`'s CSV fixture
/// (300s-spaced, matching `intraday_scalper`'s required `timeframe_secs=300`),
/// seeded into `md_bars` instead of a CSV file.
async fn seed_three_bars(pool: &sqlx::PgPool, symbol: &str, timeframe: &str) -> anyhow::Result<()> {
    insert_test_bar(
        pool, symbol, timeframe, 1_700_000_300, 100_000_000, 101_000_000, 99_000_000,
        100_500_000, 1_000,
    )
    .await?;
    insert_test_bar(
        pool, symbol, timeframe, 1_700_000_600, 100_500_000, 101_500_000, 100_000_000,
        101_000_000, 1_200,
    )
    .await?;
    insert_test_bar(
        pool, symbol, timeframe, 1_700_000_900, 101_000_000, 102_000_000, 100_500_000,
        101_500_000, 1_300,
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_test_bar(
    pool: &sqlx::PgPool,
    symbol: &str,
    timeframe: &str,
    end_ts: i64,
    open_micros: i64,
    high_micros: i64,
    low_micros: i64,
    close_micros: i64,
    volume: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete
        ) values
          ($1,$2,$3,$4,$5,$6,$7,$8,true)
        on conflict (symbol, timeframe, end_ts) do update set
          open_micros = excluded.open_micros,
          high_micros = excluded.high_micros,
          low_micros = excluded.low_micros,
          close_micros = excluded.close_micros,
          volume = excluded.volume,
          is_complete = excluded.is_complete
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(end_ts)
    .bind(open_micros)
    .bind(high_micros)
    .bind(low_micros)
    .bind(close_micros)
    .bind(volume)
    .execute(pool)
    .await?;
    Ok(())
}

async fn delete_test_bars(pool: &sqlx::PgPool, symbol: &str) {
    let _ = sqlx::query("delete from md_bars where symbol = $1")
        .bind(symbol)
        .execute(pool)
        .await;
}

/// Skips gracefully (never fails the suite) without a working
/// `MQK_DATABASE_URL`, matching `scenario_cli_db_migrate_requires_yes_when_live_active.rs`.
async fn maybe_pool() -> Option<sqlx::PgPool> {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("SKIP: {} not set", mqk_db::ENV_DB_URL);
        return None;
    }
    match mqk_db::testkit_db_pool().await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("SKIP: cannot connect to DB: {e}");
            None
        }
    }
}

fn run_cli(args: &[&str], db_url: &str) -> anyhow::Result<Output> {
    Ok(assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .env(mqk_db::ENV_DB_URL, db_url)
        .output()?)
}

fn run_cli_ok(args: &[&str], db_url: &str) -> anyhow::Result<String> {
    let output = run_cli(args, db_url)?;
    assert!(
        output.status.success(),
        "mqk-cli failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

/// Parses the `artifacts_dir=<path>` line that `run_backtest_db` prints to stdout.
fn extract_artifacts_dir(stdout: &str) -> PathBuf {
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("artifacts_dir=") {
            return PathBuf::from(path);
        }
    }
    panic!("stdout did not contain artifacts_dir=...; stdout:\n{stdout}");
}

// ---------------------------------------------------------------------------
// DB-backed: no economics flags -> default equity economics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn backtest_db_economics_default_preserves_equity() -> anyhow::Result<()> {
    let Some(pool) = maybe_pool().await else {
        return Ok(());
    };
    let db_url = std::env::var(mqk_db::ENV_DB_URL).expect("set, just checked by maybe_pool");

    let symbol = unique_symbol("dflt");
    delete_test_bars(&pool, &symbol).await;
    seed_three_bars(&pool, &symbol, "5m").await?;

    let out_dir = fresh_out_dir("dbecon_dflt");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let result = run_cli_ok(
        &[
            "backtest",
            "db",
            "--timeframe",
            "5m",
            "--start-end-ts",
            "1700000000",
            "--end-end-ts",
            "1700001000",
            "--symbols",
            &symbol,
            "--strategy",
            "intraday_scalper",
            "--symbol",
            &symbol,
            "--timeframe-secs",
            "300",
            "--out-dir",
            &out_dir_s,
        ],
        &db_url,
    );
    delete_test_bars(&pool, &symbol).await;
    let stdout = result?;

    assert!(stdout.contains("bars_loaded=3"));
    assert!(stdout.contains("economics_contract_multiplier=1"));
    assert!(stdout.contains("economics_margin_enforced=false"));

    let run_dir = extract_artifacts_dir(&stdout);
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
    let econ = &metrics["economics"];
    assert_eq!(econ["contract_multiplier"], 1);
    assert!(econ["initial_margin_micros"].is_null());
    assert!(econ["maintenance_margin_micros"].is_null());
    assert_eq!(econ["margin_enforced"], false);

    let report_md = std::fs::read_to_string(run_dir.join("report.md"))?;
    assert!(report_md.contains("Contract Multiplier | 1"));

    Ok(())
}

// ---------------------------------------------------------------------------
// DB-backed: --contract-multiplier 50 reaches report/metrics/report.md
// ---------------------------------------------------------------------------

#[tokio::test]
async fn backtest_db_economics_multiplier_50_appears_in_artifacts() -> anyhow::Result<()> {
    let Some(pool) = maybe_pool().await else {
        return Ok(());
    };
    let db_url = std::env::var(mqk_db::ENV_DB_URL).expect("set, just checked by maybe_pool");

    let symbol = unique_symbol("mult50");
    delete_test_bars(&pool, &symbol).await;
    seed_three_bars(&pool, &symbol, "5m").await?;

    let out_dir = fresh_out_dir("dbecon_mult50");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let result = run_cli_ok(
        &[
            "backtest",
            "db",
            "--timeframe",
            "5m",
            "--start-end-ts",
            "1700000000",
            "--end-end-ts",
            "1700001000",
            "--symbols",
            &symbol,
            "--strategy",
            "intraday_scalper",
            "--symbol",
            &symbol,
            "--timeframe-secs",
            "300",
            "--contract-multiplier",
            "50",
            "--out-dir",
            &out_dir_s,
        ],
        &db_url,
    );
    delete_test_bars(&pool, &symbol).await;
    let stdout = result?;

    assert!(stdout.contains("economics_contract_multiplier=50"));

    let run_dir = extract_artifacts_dir(&stdout);
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
    assert_eq!(metrics["economics"]["contract_multiplier"], 50);

    let report_md = std::fs::read_to_string(run_dir.join("report.md"))?;
    assert!(report_md.contains("Contract Multiplier | 50"));

    Ok(())
}

// ---------------------------------------------------------------------------
// DB-backed: margin flags are metadata-only, never enforced
// ---------------------------------------------------------------------------

#[tokio::test]
async fn backtest_db_economics_margin_metadata_not_enforced() -> anyhow::Result<()> {
    let Some(pool) = maybe_pool().await else {
        return Ok(());
    };
    let db_url = std::env::var(mqk_db::ENV_DB_URL).expect("set, just checked by maybe_pool");

    let symbol = unique_symbol("margin");
    delete_test_bars(&pool, &symbol).await;
    seed_three_bars(&pool, &symbol, "5m").await?;

    let out_dir = fresh_out_dir("dbecon_margin");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let result = run_cli_ok(
        &[
            "backtest",
            "db",
            "--timeframe",
            "5m",
            "--start-end-ts",
            "1700000000",
            "--end-end-ts",
            "1700001000",
            "--symbols",
            &symbol,
            "--strategy",
            "intraday_scalper",
            "--symbol",
            &symbol,
            "--timeframe-secs",
            "300",
            "--contract-multiplier",
            "50",
            "--initial-margin-micros",
            "10000000000",
            "--maintenance-margin-micros",
            "5000000000",
            "--out-dir",
            &out_dir_s,
        ],
        &db_url,
    );
    delete_test_bars(&pool, &symbol).await;
    let stdout = result?;

    assert!(stdout.contains("economics_margin_enforced=false"));

    let run_dir = extract_artifacts_dir(&stdout);
    let metrics: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("metrics.json"))?)?;
    let econ = &metrics["economics"];
    assert_eq!(econ["initial_margin_micros"], 10_000_000_000i64);
    assert_eq!(econ["maintenance_margin_micros"], 5_000_000_000i64);
    assert_eq!(
        econ["margin_enforced"], false,
        "margin metadata must never claim enforcement"
    );

    let report_md = std::fs::read_to_string(run_dir.join("report.md"))?;
    assert!(report_md.contains("Margin Enforced | false"));

    Ok(())
}

// ---------------------------------------------------------------------------
// No DB required: invalid multiplier fails closed before any DB connect
// or artifact directory is created.
// ---------------------------------------------------------------------------

#[test]
fn backtest_db_economics_zero_multiplier_fails_closed() -> anyhow::Result<()> {
    let out_dir = fresh_out_dir("dbecon_zero");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "backtest",
            "db",
            "--timeframe",
            "5m",
            "--start-end-ts",
            "1700000000",
            "--end-end-ts",
            "1700001000",
            "--contract-multiplier",
            "0",
            "--out-dir",
            &out_dir_s,
        ])
        .output()?;

    assert!(
        !output.status.success(),
        "CLI must fail closed on --contract-multiplier 0"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("contract_multiplier must be positive"),
        "stderr must explain the rejection; stderr:\n{stderr}"
    );
    assert!(
        !out_dir.exists(),
        "no artifact directory must be created on fail-closed rejection"
    );

    Ok(())
}

#[test]
fn backtest_db_economics_negative_multiplier_fails_closed() -> anyhow::Result<()> {
    let out_dir = fresh_out_dir("dbecon_neg");
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let output = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args([
            "backtest",
            "db",
            "--timeframe",
            "5m",
            "--start-end-ts",
            "1700000000",
            "--end-end-ts",
            "1700001000",
            // clap parses a bare `-5` as a possible flag; `=` form is unambiguous.
            "--contract-multiplier=-5",
            "--out-dir",
            &out_dir_s,
        ])
        .output()?;

    assert!(
        !output.status.success(),
        "CLI must fail closed on a negative --contract-multiplier"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("contract_multiplier must be positive"),
        "stderr must explain the rejection; stderr:\n{stderr}"
    );
    assert!(
        !out_dir.exists(),
        "no artifact directory must be created on fail-closed rejection"
    );

    Ok(())
}
