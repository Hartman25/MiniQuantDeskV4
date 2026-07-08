//! REGISTRY-V2-TRANSLATION-01C-VALIDATOR-CLI-AND-REPORT-01.
//!
//! Proves `mqk-cli md registry-v2-translation-check` is a read-only proof of
//! `mqk_md::instrument_registry_v2::RegistryV2SymbolTranslationIndex`
//! (REGISTRY-V2-TRANSLATION-01B) against the current registry universe: it
//! never opens a DB connection, never makes a provider/network call, never
//! mutates `--registry-v1`/`--registry-v2`, classifies a clean run as
//! `active`/`all_passed=true`, and fails closed (nonzero exit,
//! `all_passed=false`) on a duplicate converted-v1 symbol, a duplicate v2
//! `instrument_id`, or an enabled non-equity trading flag in a v2 fixture.
//!
//! No `MQK_DATABASE_URL` is set anywhere in this file -- if the command
//! under test ever opened a DB connection, these tests would fail with a
//! connection error rather than the expected output.

use std::path::PathBuf;
use uuid::Uuid;

fn real_v1_registry_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../../../config/instruments/equities.json")
}

fn real_crypto_v2_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

fn write_temp_json(label: &str, content: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_cli_registry_v2_translation_check_{label}_{}.json",
        Uuid::new_v4()
    ));
    std::fs::write(&path, content).expect("write temp fixture");
    path
}

fn cleanup(paths: &[&PathBuf]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}

fn run(args: &[&str]) -> std::process::Output {
    assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .env_remove("MQK_DATABASE_URL")
        .output()
        .expect("failed to run mqk-cli")
}

/// A v1 registry fixture with a configurable duplicate symbol (two rows
/// sharing `symbol="AAPL"` when `duplicate=true`, otherwise two distinct
/// single-row equities).
fn v1_registry_json(duplicate: bool) -> String {
    let second_symbol = if duplicate { "AAPL" } else { "MSFT" };
    let second_id = if duplicate {
        "equity:US:AAPL2"
    } else {
        "equity:US:MSFT"
    };
    format!(
        r#"[
  {{
    "instrument_id": "equity:US:AAPL",
    "symbol": "AAPL",
    "asset_class": "equity",
    "provider": "twelvedata",
    "provider_symbol": "AAPL",
    "venue": "NASDAQ",
    "currency": "USD",
    "enabled": true,
    "timeframes": ["1D"],
    "notes": "fixture"
  }},
  {{
    "instrument_id": "{second_id}",
    "symbol": "{second_symbol}",
    "asset_class": "equity",
    "provider": "twelvedata",
    "provider_symbol": "{second_symbol}",
    "venue": "NASDAQ",
    "currency": "USD",
    "enabled": true,
    "timeframes": ["1D"],
    "notes": "fixture"
  }}
]"#
    )
}

/// A v2 registry fixture with a configurable duplicate `instrument_id`
/// across two rows, and configurable crypto trading-flag enablement.
fn v2_registry_json(duplicate_instrument_id: bool, crypto_live_trading_enabled: bool) -> String {
    let second_id = if duplicate_instrument_id {
        "crypto:GLOBAL:BTCUSD"
    } else {
        "crypto:GLOBAL:ETHUSD"
    };
    format!(
        r#"{{
  "schema_version": 1,
  "instruments": [
    {{
      "instrument_id": "crypto:GLOBAL:BTCUSD",
      "symbol": "BTC/USD",
      "asset_class": "crypto",
      "currency": "USD",
      "quote_currency": "USD",
      "provider_symbols": {{"local_csv": "BTC/USD"}},
      "enabled": false,
      "paper_trading_enabled": false,
      "live_trading_enabled": {crypto_live_trading_enabled},
      "timeframes": ["1D"],
      "contract": {{"kind": "crypto_pair", "base": "BTC", "quote": "USD"}}
    }},
    {{
      "instrument_id": "{second_id}",
      "symbol": "ETH/USD",
      "asset_class": "crypto",
      "currency": "USD",
      "quote_currency": "USD",
      "provider_symbols": {{"local_csv": "ETH/USD"}},
      "enabled": false,
      "paper_trading_enabled": false,
      "live_trading_enabled": false,
      "timeframes": ["1D"],
      "contract": {{"kind": "crypto_pair", "base": "ETH", "quote": "USD"}}
    }}
  ]
}}"#
    )
}

// RVTC-01: happy path against the real, committed v1 registry (88-row equity
// universe) -> active / all_passed=true, full round-trip count.
#[test]
fn rvtc_01_happy_path_real_v1_registry_is_active() {
    let output = run(&[
        "md",
        "registry-v2-translation-check",
        "--registry-v1",
        &real_v1_registry_path().to_string_lossy(),
    ]);

    assert!(
        output.status.success(),
        "translation check must succeed against the real v1 registry\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("truth_state=active"));
    assert!(stdout.contains("converted_v1_instrument_count=88"));
    assert!(stdout.contains("round_trip_checked_count=88"));
    assert!(stdout.contains("db_write=false"));
    assert!(stdout.contains("network_call_made=false"));
    assert!(stdout.contains("production_cutover_enabled=false"));
    assert!(stdout.contains("trading_uses_v2=false"));
    assert!(stdout.contains("all_passed=true"));
}

// RVTC-02: happy path against the real v1 registry *and* the committed
// crypto local-marks v2 fixture (BTC/USD + ETH/USD, both disabled) -> both
// lanes report active/all_passed=true independently.
#[test]
fn rvtc_02_happy_path_v1_and_crypto_v2_fixture_is_active() {
    let output = run(&[
        "md",
        "registry-v2-translation-check",
        "--registry-v1",
        &real_v1_registry_path().to_string_lossy(),
        "--registry-v2",
        &real_crypto_v2_fixture_path().to_string_lossy(),
    ]);

    assert!(
        output.status.success(),
        "translation check must succeed against v1 + crypto v2 fixture\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("truth_state=active"));
    assert!(stdout.contains("converted_v1_instrument_count=88"));
    assert!(stdout.contains("v2_instrument_count=2"));
    assert!(stdout.contains("non_tradable_count=2"));
    assert!(stdout.contains("enabled_count=0"));
    assert!(stdout.contains("all_passed=true"));
}

// RVTC-03: a duplicate converted-v1 symbol fails closed with nonzero exit
// and truth_state=translation_collision.
#[test]
fn rvtc_03_duplicate_converted_v1_symbol_fails_closed() {
    let path = write_temp_json("rvtc03", &v1_registry_json(true));

    let output = run(&[
        "md",
        "registry-v2-translation-check",
        "--registry-v1",
        &path.to_string_lossy(),
    ]);
    cleanup(&[&path]);

    assert!(
        !output.status.success(),
        "duplicate v1 symbol must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=translation_collision"));
    assert!(stdout.contains("all_passed=false"));
    assert!(stdout.contains("v1_translation_index_build_failed"));
}

// RVTC-04: a duplicate v2 instrument_id fails closed with nonzero exit and
// truth_state=translation_collision.
#[test]
fn rvtc_04_duplicate_v2_instrument_id_fails_closed() {
    let path = write_temp_json("rvtc04", &v2_registry_json(true, false));

    let output = run(&[
        "md",
        "registry-v2-translation-check",
        "--registry-v2",
        &path.to_string_lossy(),
    ]);
    cleanup(&[&path]);

    assert!(
        !output.status.success(),
        "duplicate v2 instrument_id must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=translation_collision"));
    assert!(stdout.contains("all_passed=false"));
    assert!(stdout.contains("v2_translation_index_build_failed"));
}

// RVTC-05: an enabled non-equity (crypto) live-trading flag fails closed
// with truth_state=unsafe_trading_enabled -- distinct from a plain
// collision, since this is a safety violation, not a data-shape violation.
#[test]
fn rvtc_05_crypto_live_trading_flag_true_fails_closed() {
    let path = write_temp_json("rvtc05", &v2_registry_json(false, true));

    let output = run(&[
        "md",
        "registry-v2-translation-check",
        "--registry-v2",
        &path.to_string_lossy(),
    ]);
    cleanup(&[&path]);

    assert!(
        !output.status.success(),
        "enabled crypto live_trading_enabled must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=unsafe_trading_enabled"));
    assert!(stdout.contains("all_passed=false"));
    assert!(stdout.contains("non_equity_trading_flag_enabled"));
}

// RVTC-06: written evidence carries no DB/network/trading claim.
#[test]
fn rvtc_06_evidence_has_no_db_network_or_trading_claim() {
    let out_dir = std::env::temp_dir().join(format!(
        "mqk_cli_registry_v2_translation_check_evidence_{}",
        Uuid::new_v4()
    ));

    let output = run(&[
        "md",
        "registry-v2-translation-check",
        "--registry-v1",
        &real_v1_registry_path().to_string_lossy(),
        "--output-dir",
        &out_dir.to_string_lossy(),
    ]);

    assert!(
        output.status.success(),
        "evidence-writing run must succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let evidence_line = stdout
        .lines()
        .find(|l| l.starts_with("evidence_path="))
        .expect("evidence_path line must be printed");
    let evidence_path = evidence_line.trim_start_matches("evidence_path=");
    let evidence_content =
        std::fs::read_to_string(evidence_path).expect("evidence file must exist and be readable");
    let evidence: serde_json::Value =
        serde_json::from_str(&evidence_content).expect("evidence must be valid JSON");

    assert_eq!(
        evidence["schema_version"],
        "registry-v2-translation-check-v1"
    );
    assert_eq!(evidence["safety"]["no_db_connection"], true);
    assert_eq!(evidence["safety"]["no_network_call_made"], true);
    assert_eq!(evidence["safety"]["no_production_cutover"], true);
    assert_eq!(evidence["safety"]["no_trading_enabled"], true);
    assert_eq!(evidence["safety"]["no_config_file_mutated"], true);

    let _ = std::fs::remove_dir_all(&out_dir);
}

// RVTC-07: omitting --output-dir means no evidence file is created (only
// stdout is printed).
#[test]
fn rvtc_07_no_output_dir_means_no_evidence_file() {
    let output = run(&[
        "md",
        "registry-v2-translation-check",
        "--registry-v1",
        &real_v1_registry_path().to_string_lossy(),
    ]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("evidence_path="),
        "no --output-dir must mean no evidence_path is printed"
    );
}

// RVTC-08: an unknown/missing --registry-v1 file path returns a clear,
// non-panicking error (nonzero exit, descriptive stderr) rather than a panic.
#[test]
fn rvtc_08_missing_registry_v1_file_returns_clear_error_not_panic() {
    let missing = std::env::temp_dir().join(format!(
        "mqk_cli_registry_v2_translation_check_missing_{}.json",
        Uuid::new_v4()
    ));

    let output = run(&[
        "md",
        "registry-v2-translation-check",
        "--registry-v1",
        &missing.to_string_lossy(),
    ]);

    assert!(
        !output.status.success(),
        "missing registry file must return a nonzero exit, not panic"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "missing file must not panic\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("v1 load failed") || stderr.contains("registry-v2-translation-check"),
        "error must be descriptive\nstderr:\n{stderr}"
    );
}
