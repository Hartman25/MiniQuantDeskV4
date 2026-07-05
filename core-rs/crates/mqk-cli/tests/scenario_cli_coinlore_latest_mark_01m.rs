//! CRYPTO-DATA-01M-COINLORE-READONLY-CLI-SURFACE-01 (part of
//! CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED).
//!
//! Proves `mqk-cli md coinlore-latest-mark` is check-only/fixture-based by
//! default, requires no DB env var, makes no network call unless the
//! operator explicitly opts in, and exits nonzero on a malformed fixture.
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

fn write_temp_file(label: &str, content: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_cli_coinlore_latest_mark_{label}_{}.json",
        Uuid::new_v4()
    ));
    std::fs::write(&path, content).expect("write temp fixture");
    path
}

fn run(args: &[&str]) -> std::process::Output {
    assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .env_remove("MQK_ALLOW_COINLORE_NETWORK_SMOKE")
        .env_remove("MQK_DATABASE_URL")
        .output()
        .expect("failed to run mqk-cli")
}

const VALID_TICKER_BODY: &str = r#"[
    {"id":"90","symbol":"BTC","price_usd":"62906.61","volume24":16261421089.448309},
    {"id":"80","symbol":"ETH","price_usd":"1777.74","volume24":9252669028.469555}
]"#;

// CLI-01: check-only fixture mode works end-to-end against the real
// registry fixture and a valid inline response file.
#[test]
fn cli01_check_only_fixture_mode_succeeds() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let input = write_temp_file("valid", VALID_TICKER_BODY);
    let input_s = input.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "coinlore-latest-mark",
        "--registry",
        &registry_s,
        "--symbols",
        "BTC/USD,ETH/USD",
        "--input-file",
        &input_s,
    ]);

    let _ = std::fs::remove_file(&input);

    assert!(
        output.status.success(),
        "coinlore-latest-mark must succeed on a valid fixture\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("network_call_made=false"));
    assert!(stdout.contains("db_write=false"));
    assert!(stdout.contains("md_bars_write=false"));
    assert!(stdout.contains("completed_bar_claim=false"));
    assert!(stdout.contains("symbol=BTC/USD"));
    assert!(stdout.contains("symbol=ETH/USD"));
    assert!(stdout.contains("price_usd=62906.61"));
}

// CLI-02: no MQK_DATABASE_URL is required -- the command above already runs
// with it unset (env_remove) and succeeds, proving no DB connection is
// attempted. This test re-asserts explicitly for a single-symbol request.
#[test]
fn cli02_no_database_url_env_required() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let input = write_temp_file("no_db_env", VALID_TICKER_BODY);
    let input_s = input.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "coinlore-latest-mark",
        "--registry",
        &registry_s,
        "--symbols",
        "BTC/USD",
        "--input-file",
        &input_s,
    ]);

    let _ = std::fs::remove_file(&input);

    assert!(
        output.status.success(),
        "command must succeed without MQK_DATABASE_URL set\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// CLI-03: no network call is made by default (no --input-file, no env var
// opt-in) -- the command must refuse before attempting any GET.
#[test]
fn cli03_no_network_by_default_refuses_without_input_file_or_env_opt_in() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "coinlore-latest-mark",
        "--registry",
        &registry_s,
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    assert!(
        !output.status.success(),
        "command must refuse to run without --input-file or the network opt-in env var"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MQK_ALLOW_COINLORE_NETWORK_SMOKE"),
        "error must name the required opt-in env var: {stderr}"
    );
}

// CLI-04: evidence JSON output is valid when --output-dir is supplied.
#[test]
fn cli04_evidence_json_output_is_valid() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let input = write_temp_file("evidence", VALID_TICKER_BODY);
    let input_s = input.to_string_lossy().to_string();
    let out_dir = std::env::temp_dir().join(format!("mqk_cli_coinlore_evidence_{}", Uuid::new_v4()));
    let out_dir_s = out_dir.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "coinlore-latest-mark",
        "--registry",
        &registry_s,
        "--symbols",
        "BTC/USD,ETH/USD",
        "--input-file",
        &input_s,
        "--output-dir",
        &out_dir_s,
    ]);

    let _ = std::fs::remove_file(&input);

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
    let value: serde_json::Value = serde_json::from_str(&content).expect("evidence must be valid JSON");
    assert_eq!(value["schema_version"], "coinlore-latest-mark-v1");
    assert_eq!(value["provider_id"], "coinlore");
    assert_eq!(value["network_call_made"], false);
    assert_eq!(value["db_write"], false);
    assert_eq!(value["md_bars_write"], false);
    assert_eq!(value["completed_bar_claim"], false);
    assert_eq!(value["marks"].as_array().unwrap().len(), 2);

    let _ = std::fs::remove_dir_all(&out_dir);
}

// CLI-05: a malformed fixture (missing configured ETH/USD asset) exits
// nonzero rather than silently succeeding with partial data.
#[test]
fn cli05_bad_fixture_exits_nonzero() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let btc_only = r#"[{"id":"90","symbol":"BTC","price_usd":"62906.61","volume24":123.0}]"#;
    let input = write_temp_file("btc_only", btc_only);
    let input_s = input.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "coinlore-latest-mark",
        "--registry",
        &registry_s,
        "--symbols",
        "BTC/USD,ETH/USD",
        "--input-file",
        &input_s,
    ]);

    let _ = std::fs::remove_file(&input);

    assert!(
        !output.status.success(),
        "a response missing a requested configured asset must exit nonzero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("parse failed") || stderr.contains("missing"),
        "unexpected stderr:\n{stderr}"
    );
}

// CLI-06: an unknown --symbols entry with no configured alias exits nonzero.
#[test]
fn cli06_unconfigured_symbol_exits_nonzero() {
    let registry = registry_fixture_path();
    let registry_s = registry.to_string_lossy().to_string();
    let input = write_temp_file("unconfigured_symbol", VALID_TICKER_BODY);
    let input_s = input.to_string_lossy().to_string();

    let output = run(&[
        "md",
        "coinlore-latest-mark",
        "--registry",
        &registry_s,
        "--symbols",
        "DOGE/USD",
        "--input-file",
        &input_s,
    ]);

    let _ = std::fs::remove_file(&input);

    assert!(
        !output.status.success(),
        "an unconfigured symbol must exit nonzero"
    );
}
