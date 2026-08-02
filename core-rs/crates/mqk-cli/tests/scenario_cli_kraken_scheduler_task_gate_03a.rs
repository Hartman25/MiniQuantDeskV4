//! CRYPTO-DATA-03A-KRAKEN-SCHEDULED-NETWORK-GATE-01.
//!
//! Proves `mqk-cli md kraken-ohlc-sync` recognizes a second, explicit
//! network opt-in -- `MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1` -- distinct from
//! the existing manual-smoke gate `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`, while
//! leaving the manual-smoke gate itself unchanged and leaving
//! `kraken-ohlc-dry-run` untouched (it reads only the manual-smoke gate,
//! proven separately in `scenario_cli_kraken_ohlc_dry_run_01w.rs`).
//!
//! IMPORTANT -- why this file does NOT subprocess-test the "opt-in env var
//! recognized, network branch reached" case directly:
//!
//! `mqk-cli`'s `main.rs` bootstraps `.env.local` via
//! `dotenvy::from_filename(".env.local")` at startup. dotenvy fills in any
//! env var not already present in the process -- so a test harness that
//! does `.env_remove("MQK_DATABASE_URL")` before spawning the CLI does NOT
//! make `MQK_DATABASE_URL` absent inside the child process if a real
//! `.env.local` exists in this repo (it does, for local dev): the child
//! re-populates it from the file after the harness's removal took effect.
//! In that environment, any subprocess run that gets past the opt-in check
//! without `--input-file` would reach the real `mqk_md::fetch_kraken_ohlc_body`
//! call and make a genuine live network request to Kraken -- exactly what
//! this patch's validation must never do. (This was discovered the hard
//! way during development of this patch: an earlier version of these tests
//! reached a live Kraken fetch and a real paper-DB write; both were cleaned
//! up and the tests were redesigned to the shape below.)
//!
//! So: the *decision logic* for "which network-authorization mode governs
//! this run" is implemented as a pure, I/O-free function
//! (`kraken_sync_network_gate` in `mqk-cli/src/commands/md.rs`) and is
//! exhaustively unit-tested there (`kraken_sync_network_gate_tests`),
//! including the "opt-in env var recognized" and "DB url required" cases
//! this file's mission text describes. This integration file only
//! subprocess-tests the two paths that are safe regardless of
//! `.env.local`'s contents: the true negative (neither env var set, which
//! fails before the DB-url check ever runs) and `--input-file` mode (which
//! never consults either env var, proven with a deliberately-unreachable
//! `MQK_DATABASE_URL` override so the process fails fast on a connection
//! error rather than touching any real database). A third, `#[ignore]`-gated
//! DB-backed test proves the evidence JSON's `network_authorization_mode`
//! field for `--input-file` mode, using the same explicit-`.env(...)`-
//! override convention as `scenario_cli_kraken_ohlc_sync_db_01zaa.rs`.
//!
//! No `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` or `MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1`
//! run without `--input-file` appears anywhere in this file, by design.

use std::path::PathBuf;

use sqlx::PgPool;
use uuid::Uuid;

fn registry_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

fn btc_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../mqk-md/tests/fixtures/kraken_ohlc_xbtusd_1d.json")
}

fn run(args: &[&str], envs: &[(&str, &str)], remove_envs: &[&str]) -> std::process::Output {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("mqk-cli");
    cmd.args(args);
    for r in remove_envs {
        cmd.env_remove(r);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run mqk-cli")
}

// ---------------------------------------------------------------------------
// Test 1 -- neither env var set, no --input-file: fail closed before DB;
// the error must name both accepted gates. Safe regardless of .env.local:
// this path never reaches the DB-url check.
// ---------------------------------------------------------------------------

#[test]
fn gate01_no_input_file_and_no_env_vars_refuses_and_names_both_gates() {
    let registry_s = registry_fixture_path().to_string_lossy().to_string();
    let output = run(
        &[
            "md",
            "kraken-ohlc-sync",
            "--registry",
            &registry_s,
            "--symbol",
            "BTC/USD",
            "--timeframe",
            "1D",
        ],
        &[],
        &["MQK_ALLOW_KRAKEN_NETWORK_SMOKE", "MQK_ALLOW_KRAKEN_SCHEDULED_SYNC", "MQK_DATABASE_URL"],
    );

    assert!(
        !output.status.success(),
        "command must refuse without --input-file or either opt-in env var"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("kraken_sync_requires_input_file_or_network_opt_in"),
        "error must name the sync-specific fail-closed reason: {stderr}"
    );
    assert!(
        stderr.contains("MQK_ALLOW_KRAKEN_NETWORK_SMOKE"),
        "error must name the manual-smoke gate: {stderr}"
    );
    assert!(
        stderr.contains("MQK_ALLOW_KRAKEN_SCHEDULED_SYNC"),
        "error must name the scheduled-sync gate: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Test 2 -- --input-file mode is unaffected by either env var: it never
// consults them, so it never trips the opt-in-refusal error regardless of
// whether they are set. Uses a deliberately-unreachable MQK_DATABASE_URL
// (an explicit override, which dotenvy will NOT replace since it is already
// present) so the process fails fast on a connection error without ever
// touching a real database.
// ---------------------------------------------------------------------------

#[test]
fn gate02_input_file_mode_unaffected_by_either_env_var() {
    let registry_s = registry_fixture_path().to_string_lossy().to_string();
    let input_s = btc_fixture_path().to_string_lossy().to_string();
    let output = run(
        &[
            "md",
            "kraken-ohlc-sync",
            "--registry",
            &registry_s,
            "--symbol",
            "BTC/USD",
            "--timeframe",
            "1D",
            "--input-file",
            &input_s,
        ],
        &[(
            "MQK_DATABASE_URL",
            "postgres://mqk_gate_test_invalid:invalid@127.0.0.1:1/mqk_unreachable_placeholder?sslmode=disable",
        )],
        &["MQK_ALLOW_KRAKEN_NETWORK_SMOKE", "MQK_ALLOW_KRAKEN_SCHEDULED_SYNC"],
    );

    // --input-file mode still needs a DB connection to write md_bars, so it
    // must fail there (a connection error against the deliberately-bogus
    // port) -- never at the opt-in gate or the network-mode DB-url gate,
    // proving --input-file never consults either env var.
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("kraken_sync_requires_input_file_or_network_opt_in"),
        "input-file mode must never trip the network opt-in gate: {stderr}"
    );
    assert!(
        !stderr.contains("kraken_sync_requires_database_url_for_network_mode"),
        "input-file mode must not be routed through the network-mode DB-url gate: {stderr}"
    );
    assert!(
        !stderr.contains("network fetch failed"),
        "input-file mode must never attempt a Kraken network fetch: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 (DB-backed) -- evidence JSON's network_authorization_mode is
// "input_file" for a fixture-driven run. `#[ignore]`-gated and requires the
// operator to explicitly set MQK_DATABASE_URL to a real (test) database
// before running with --include-ignored, mirroring the safe, established
// convention in scenario_cli_kraken_ohlc_sync_db_01zaa.rs -- an explicit
// `.env(...)` override here, never a `.env_remove` relying on absence.
// Cleans up the exact rows it writes; touches no other symbol/provider.
// ---------------------------------------------------------------------------

const EARLIER_END_TS: i64 = 1_783_123_200;
const LATEST_END_TS: i64 = 1_783_209_600;

async fn cleanup_kraken_rows(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        delete from md_bars
        where symbol = 'BTC/USD'
          and timeframe = '1D'
          and provider_id = 'kraken'
          and end_ts = any($1)
        "#,
    )
    .bind(&[EARLIER_END_TS, LATEST_END_TS][..])
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_scheduler_task_gate_03a -- --include-ignored"]
async fn gate03_evidence_records_input_file_network_authorization_mode() -> anyhow::Result<()> {
    let db_url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => panic!(
            "DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-cli --test scenario_cli_kraken_scheduler_task_gate_03a -- --include-ignored"
        ),
    };
    let pool = mqk_db::testkit_db_pool().await?;

    // Clear any leftover kraken-tagged rows from a prior failed run.
    cleanup_kraken_rows(&pool).await?;

    let registry_s = registry_fixture_path().to_string_lossy().to_string();
    let input_s = btc_fixture_path().to_string_lossy().to_string();
    let out_dir =
        std::env::temp_dir().join(format!("mqk_cli_kraken_gate_evidence_{}", Uuid::new_v4()));
    let out_dir_s = out_dir.to_string_lossy().to_string();

    // Removes `out_dir` on drop (any exit path, including a failed assert!
    // panic) -- but only once this guard goes out of scope at the end of the
    // function, i.e. after the evidence file has already been read and
    // validated below. Deleting the directory any earlier would delete the
    // evidence file this test still needs to read back.
    struct OutDirCleanup(PathBuf);
    impl Drop for OutDirCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _out_dir_cleanup = OutDirCleanup(out_dir.clone());

    // Explicit .env(...) override -- present before dotenvy runs inside the
    // child, so dotenvy will not replace it with .env.local's value.
    let output = run(
        &[
            "md",
            "kraken-ohlc-sync",
            "--registry",
            &registry_s,
            "--symbol",
            "BTC/USD",
            "--timeframe",
            "1D",
            "--input-file",
            &input_s,
            "--output-dir",
            &out_dir_s,
        ],
        &[("MQK_DATABASE_URL", &db_url)],
        &["MQK_ALLOW_KRAKEN_NETWORK_SMOKE", "MQK_ALLOW_KRAKEN_SCHEDULED_SYNC"],
    );

    let cleanup_result = cleanup_kraken_rows(&pool).await;

    assert!(
        output.status.success(),
        "fixture-driven sync must succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    cleanup_result?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        stdout.contains("network_authorization_mode=input_file"),
        "stdout must print the new field: {stdout}"
    );

    let evidence_line = stdout
        .lines()
        .find(|l| l.starts_with("evidence_path="))
        .expect("stdout must print evidence_path=...");
    let evidence_path = PathBuf::from(evidence_line.trim_start_matches("evidence_path="));
    let content = std::fs::read_to_string(&evidence_path)?;
    let value: serde_json::Value = serde_json::from_str(&content)?;

    assert_eq!(value["network_authorization_mode"], "input_file");
    assert_eq!(value["mode"], "input_file");
    assert_eq!(value["network_call_made"], false);

    Ok(())
}
