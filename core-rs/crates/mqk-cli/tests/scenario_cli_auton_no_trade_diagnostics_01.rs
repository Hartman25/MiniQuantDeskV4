//! AUTON-NO-TRADE-OFFHOURS-01D — read-only CLI report for the durable
//! autonomous no-trade diagnostic journal.
//!
//! `mqk autonomous no-trade-diagnostics` mirrors
//! `GET /api/v1/autonomous/no-trade-diagnostics` (01C) at the CLI: read-only,
//! no DB write, no runtime start, no broker/provider call. DB-backed, skipped
//! if `MQK_DATABASE_URL` is not set (matches the existing
//! `scenario_cli_db_migrate_requires_yes_when_live_active.rs` graceful-skip
//! pattern).

use predicates::prelude::*;
use uuid::Uuid;

fn core_rs_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repo root must resolve")
        .join("core-rs")
}

/// CLI-01: with at least one seeded row for a unique reason_code, the command
/// succeeds and prints that row's `reason_code` and honest `paper_order_attempted=false`
/// / `live_order_attempted=false` flags to stdout.
#[allow(deprecated)]
#[tokio::test]
async fn cli01_no_trade_diagnostics_prints_seeded_row() -> anyhow::Result<()> {
    let Ok(url) = std::env::var(mqk_db::ENV_DB_URL) else {
        eprintln!("SKIP: MQK_DATABASE_URL not set");
        return Ok(());
    };
    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SKIP: cannot connect to DB: {e}");
            return Ok(());
        }
    };
    mqk_db::migrate(&pool).await?;

    let reason_code = format!("CLI01_{}", Uuid::new_v4().simple());
    let diagnostic_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.no-trade-diagnostic.v1|{reason_code}|pre_session_window|cli01").as_bytes(),
    );
    mqk_db::insert_autonomous_no_trade_diagnostic(
        &pool,
        &mqk_db::InsertAutonomousNoTradeDiagnosticArgs {
            diagnostic_id,
            observed_at_utc: chrono::Utc::now(),
            run_id: None,
            mode: "paper".to_string(),
            session_window_state: "outside_window".to_string(),
            runtime_start_allowed: true,
            arm_state: "armed".to_string(),
            overall_ready: false,
            reason_code: reason_code.clone(),
            reason: "CLI-01 test blocker text".to_string(),
            stage: "pre_session_window".to_string(),
            paper_order_attempted: false,
            live_order_attempted: false,
            source: "test".to_string(),
        },
    )
    .await?;

    let mut cmd = assert_cmd::Command::cargo_bin("mqk-cli")?;
    cmd.current_dir(core_rs_dir())
        .env(mqk_db::ENV_DB_URL, &url)
        .args(["autonomous", "no-trade-diagnostics", "--limit", "50"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("truth_state=active"))
        .stdout(predicate::str::contains(reason_code.as_str()))
        .stdout(predicate::str::contains("paper_order_attempted=false"))
        .stdout(predicate::str::contains("live_order_attempted=false"));

    Ok(())
}

/// CLI-02: the command never writes to `oms_outbox` — a purely read-only report.
#[allow(deprecated)]
#[tokio::test]
async fn cli02_no_trade_diagnostics_creates_zero_outbox_rows() -> anyhow::Result<()> {
    let Ok(url) = std::env::var(mqk_db::ENV_DB_URL) else {
        eprintln!("SKIP: MQK_DATABASE_URL not set");
        return Ok(());
    };
    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SKIP: cannot connect to DB: {e}");
            return Ok(());
        }
    };
    mqk_db::migrate(&pool).await?;

    let outbox_before: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await?;

    let mut cmd = assert_cmd::Command::cargo_bin("mqk-cli")?;
    cmd.current_dir(core_rs_dir())
        .env(mqk_db::ENV_DB_URL, &url)
        .args(["autonomous", "no-trade-diagnostics", "--limit", "5"]);
    cmd.assert().success();

    let outbox_after: i64 = sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        outbox_before, outbox_after,
        "CLI-02: the read-only report must never write to oms_outbox"
    );

    Ok(())
}
