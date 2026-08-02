//! CRYPTO-DATA-01W-KRAKEN-OHLCV-CLI-EVIDENCE-01 (part of
//! CRYPTO-DATA-01U-V-W-KRAKEN-OHLCV-ADAPTER-PARSER-CLI-BUNDLE-01-COMBINED).
//!
//! Proves `mqk-cli md kraken-ohlc-dry-run` is check-only/fixture-based by
//! default, requires no DB env var, makes no network call unless the
//! operator explicitly opts in, and excludes the forming candle from every
//! completed-bar claim.
//!
//! No `MQK_DATABASE_URL` is set anywhere in this file -- if the command
//! under test ever opened a DB connection, these tests would fail with a
//! connection error rather than the expected output.

use std::path::PathBuf;
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

fn eth_fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../mqk-md/tests/fixtures/kraken_ohlc_ethusd_1d.json")
}

fn run(args: &[&str]) -> std::process::Output {
    assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .env_remove("MQK_ALLOW_KRAKEN_NETWORK_SMOKE")
        .env_remove("MQK_DATABASE_URL")
        .output()
        .expect("failed to run mqk-cli")
}

// CLI-01: check-only fixture mode works end-to-end against the real
// registry fixture and the committed BTC/USD Kraken fixture.
#[test]
fn cli01_check_only_fixture_mode_succeeds_for_btc() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let input = btc_fixture_path();
    let input_s = input.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "kraken-ohlc-dry-run",
        "--registry",
        &registry_s,
        "--symbol",
        "BTC/USD",
        "--timeframe",
        "1D",
        "--input-file",
        &input_s,
    ]);

    assert!(
        output.status.success(),
        "kraken-ohlc-dry-run must succeed on the committed BTC fixture\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("network_call_made=false"));
    assert!(stdout.contains("db_write=false"));
    assert!(stdout.contains("md_bars_write=false"));
    assert!(stdout.contains("completed_bar_claim=true"));
    assert!(stdout.contains("forming_candle_excluded=true"));
    assert!(stdout.contains("bars_completed=2"));
    assert!(stdout.contains("bars_excluded_forming=1"));
    assert!(stdout.contains("latest_completed_end_ts=1783209600"));
    assert!(stdout.contains("volume_scaled=131715941434"));
}

// CLI-02: check-only fixture mode also succeeds for ETH/USD, proving the
// adapter is not hardcoded to a single symbol.
#[test]
fn cli02_check_only_fixture_mode_succeeds_for_eth() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let input = eth_fixture_path();
    let input_s = input.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "kraken-ohlc-dry-run",
        "--registry",
        &registry_s,
        "--symbol",
        "ETH/USD",
        "--timeframe",
        "1D",
        "--input-file",
        &input_s,
    ]);

    assert!(
        output.status.success(),
        "kraken-ohlc-dry-run must succeed on the committed ETH fixture\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("volume_scaled=1558801759742"));
    assert!(stdout.contains("bars_completed=2"));
}

// CLI-03: no network call is made by default (no --input-file, no env var
// opt-in) -- the command must refuse before attempting any GET.
#[test]
fn cli03_no_network_by_default_refuses_without_input_file_or_env_opt_in() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "kraken-ohlc-dry-run",
        "--registry",
        &registry_s,
        "--symbol",
        "BTC/USD",
        "--timeframe",
        "1D",
    ]);

    assert!(
        !output.status.success(),
        "command must refuse to run without --input-file or the network opt-in env var"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MQK_ALLOW_KRAKEN_NETWORK_SMOKE"),
        "error must name the required opt-in env var: {stderr}"
    );
}

// CLI-04: unsupported timeframe is refused before any file/network access.
#[test]
fn cli04_unsupported_timeframe_refused() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let input = btc_fixture_path();
    let input_s = input.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "kraken-ohlc-dry-run",
        "--registry",
        &registry_s,
        "--symbol",
        "BTC/USD",
        "--timeframe",
        "5m",
        "--input-file",
        &input_s,
    ]);

    assert!(
        !output.status.success(),
        "an unsupported timeframe must exit nonzero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("only --timeframe 1D is supported"),
        "unexpected stderr:\n{stderr}"
    );
}

// CLI-05: an unconfigured symbol with no kraken_pair alias exits nonzero.
#[test]
fn cli05_unconfigured_symbol_exits_nonzero() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let input = btc_fixture_path();
    let input_s = input.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "kraken-ohlc-dry-run",
        "--registry",
        &registry_s,
        "--symbol",
        "DOGE/USD",
        "--timeframe",
        "1D",
        "--input-file",
        &input_s,
    ]);

    assert!(
        !output.status.success(),
        "an unconfigured symbol must exit nonzero"
    );
}

// CLI-06: a malformed fixture (non-empty top-level error) exits nonzero
// rather than silently succeeding with partial/fabricated data.
#[test]
fn cli06_bad_fixture_exits_nonzero() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let bad = r#"{"error": ["EQuery:Unknown asset pair"], "result": {}}"#;
    let input = std::env::temp_dir().join(format!("mqk_cli_kraken_bad_{}.json", Uuid::new_v4()));
    std::fs::write(&input, bad).expect("write temp fixture");
    let input_s = input.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "kraken-ohlc-dry-run",
        "--registry",
        &registry_s,
        "--symbol",
        "BTC/USD",
        "--timeframe",
        "1D",
        "--input-file",
        &input_s,
    ]);

    let _ = std::fs::remove_file(&input);

    assert!(
        !output.status.success(),
        "a response carrying a non-empty top-level error must exit nonzero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("parse failed"),
        "unexpected stderr:\n{stderr}"
    );
}

// CLI-07: evidence JSON output is valid and carries the required contract
// fields, with the load-bearing safety flags exactly as documented.
#[test]
fn cli07_evidence_json_output_is_valid() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let input = btc_fixture_path();
    let input_s = input.to_string_lossy().to_string();
    let out_dir = std::env::temp_dir().join(format!("mqk_cli_kraken_evidence_{}", Uuid::new_v4()));
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "kraken-ohlc-dry-run",
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
    ]);

    assert!(
        output.status.success(),
        "command with --output-dir must succeed\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let evidence_line = stdout
        .lines()
        .find(|l| l.starts_with("evidence_path="))
        .expect("stdout must print evidence_path=...");
    let evidence_path = PathBuf::from(evidence_line.trim_start_matches("evidence_path="));

    let content = std::fs::read_to_string(&evidence_path).expect("evidence file must exist");
    let value: serde_json::Value =
        serde_json::from_str(&content).expect("evidence must be valid JSON");
    let _ = std::fs::remove_dir_all(&out_dir);

    let required_fields: &[&str] = &[
        "schema_version",
        "producer",
        "provider",
        "mode",
        "network_call_made",
        "db_write",
        "md_bars_write",
        "completed_bar_claim",
        "forming_candle_excluded",
        "volume_semantics",
        "volume_scale",
        "symbols_requested",
        "bars_completed",
        "bars_excluded_forming",
        "latest_completed_start_ts",
        "latest_completed_end_ts",
        "all_passed",
        "reason_code",
        "fail_reasons",
    ];
    let obj = value.as_object().expect("evidence must be a JSON object");
    for field in required_fields {
        assert!(
            obj.contains_key(*field),
            "evidence envelope missing required field '{field}': {value}"
        );
    }

    assert_eq!(value["schema_version"], "kraken-ohlc-dry-run-v1");
    assert_eq!(value["provider"], "kraken");
    assert_eq!(value["mode"], "input_file");
    assert_eq!(value["network_call_made"], false);
    assert_eq!(value["db_write"], false);
    assert_eq!(value["md_bars_write"], false);
    assert_eq!(value["completed_bar_claim"], true);
    assert_eq!(value["forming_candle_excluded"], true);
    assert_eq!(value["volume_scale"], 100_000_000);
    assert_eq!(value["bars_completed"], 2);
    assert_eq!(value["bars_excluded_forming"], 1);
    assert_eq!(value["latest_completed_start_ts"], 1_783_123_200);
    assert_eq!(value["latest_completed_end_ts"], 1_783_209_600);
    assert_eq!(value["all_passed"], true);
    assert!(value["fail_reasons"].as_array().unwrap().is_empty());
    let symbols_requested: Vec<String> = value["symbols_requested"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(symbols_requested, vec!["BTC/USD"]);
}
