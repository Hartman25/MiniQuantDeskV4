//! CRYPTO-REGISTRY-03-KRAKEN-DATA-ONLY-REGISTRY-READINESS-CLI-01.
//!
//! Proves `mqk-cli md crypto-registry-readiness` is a read-only readiness
//! surface: it never opens a DB connection, never makes a provider/network
//! call, never mutates `--registry`/`--providers`, classifies the current
//! (disabled) fixture state as `data_ready_manual_only` rather than a
//! failure, and fails closed on a missing provider, a missing/incomplete
//! Kraken alias, an unexpectedly-enabled trading flag, or an
//! unexpectedly-enabled provider.
//!
//! No `MQK_DATABASE_URL` is set anywhere in this file -- if the command
//! under test ever opened a DB connection, these tests would fail with a
//! connection error rather than the expected output.

use std::path::PathBuf;
use uuid::Uuid;

fn real_registry_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
}

fn real_providers_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../../../config/providers/providers.json")
}

fn write_temp_json(label: &str, content: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_cli_crypto_registry_readiness_{label}_{}.json",
        Uuid::new_v4()
    ));
    std::fs::write(&path, content).expect("write temp fixture");
    path
}

fn run(args: &[&str]) -> std::process::Output {
    assert_cmd::cargo::cargo_bin_cmd!("mqk-cli")
        .args(args)
        .env_remove("MQK_DATABASE_URL")
        .output()
        .expect("failed to run mqk-cli")
}

/// A providers.json fixture carrying a `kraken` entry with the given
/// `enabled` value, mirroring the real config's shape.
fn providers_json(kraken_enabled: bool) -> String {
    format!(
        r#"[
  {{
    "provider_id": "kraken",
    "display_name": "Kraken (public OHLC)",
    "asset_classes": ["crypto"],
    "free_tier_available": true,
    "api_key_required": false,
    "credential_env_vars": [],
    "rate_limit_notes": "no numeric public rate limit established",
    "supported_timeframes": ["1D"],
    "historical_depth_notes": "test fixture",
    "realtime_support_notes": "test fixture",
    "licensing_notes": "test fixture",
    "implementation_status": "ohlcv_adapter_fixture_proven_network_opt_in_only",
    "enabled": {kraken_enabled},
    "verification_status": "fixture_parser_proven_live_network_not_exercised_by_this_patch",
    "docs_url": "https://docs.kraken.com/api/docs/rest-api/get-ohlc-data"
  }}
]"#
    )
}

/// A registry-v2 fixture carrying one BTC/USD crypto row with configurable
/// `paper_trading_enabled`/`live_trading_enabled` flags and an optional
/// Kraken alias.
fn registry_json(
    paper_trading_enabled: bool,
    live_trading_enabled: bool,
    include_kraken_alias: bool,
) -> String {
    let provider_symbols = if include_kraken_alias {
        r#"{"kraken_pair": "XBTUSD", "kraken_result_key": "XXBTZUSD"}"#
    } else {
        "{}"
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
      "provider_symbols": {provider_symbols},
      "enabled": false,
      "paper_trading_enabled": {paper_trading_enabled},
      "live_trading_enabled": {live_trading_enabled},
      "timeframes": ["1D"],
      "contract": {{"kind": "crypto_pair", "base": "BTC", "quote": "USD"}}
    }}
  ]
}}"#
    )
}

// RC-01: happy path against the real, committed fixture files -> active /
// data_ready_manual_only, both symbols pass, exit 0.
#[test]
fn rc_01_happy_path_real_fixture_is_active_and_data_ready_manual_only() {
    let registry = real_registry_path();
    let providers = real_providers_path();

    let output = run(&[
        "md",
        "crypto-registry-readiness",
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--provider",
        "kraken",
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    assert!(
        output.status.success(),
        "readiness check must succeed against the real fixture\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("truth_state=active"));
    assert!(stdout.contains("data_readiness_state=data_ready_manual_only"));
    assert!(stdout.contains("trading_readiness_state=disabled"));
    assert!(stdout.contains("scheduler_readiness_state=absent"));
    assert!(stdout.contains("provider_enabled=false"));
    assert!(stdout.contains("api_key_required=false"));
    assert!(stdout.contains("all_passed=true"));
    assert!(stdout.contains("no_scheduler=true"));
    assert!(stdout.contains("no_db_connection=true"));
    assert!(stdout.contains("no_network_call=true"));
    assert!(stdout.contains("no_trading_enabled=true"));
    assert!(stdout.contains("symbol=BTC/USD"));
    assert!(stdout.contains("symbol=ETH/USD"));
    assert!(stdout.contains("passed=true"));
}

// RC-02: missing provider entry fails closed with nonzero exit and
// truth_state=missing_provider.
#[test]
fn rc_02_missing_provider_fails_closed() {
    let registry = write_temp_json("rc02_registry", &registry_json(false, false, true));
    let providers = write_temp_json("rc02_providers", "[]");

    let output = run(&[
        "md",
        "crypto-registry-readiness",
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--provider",
        "kraken",
        "--symbols",
        "BTC/USD",
    ]);

    let _ = std::fs::remove_file(&registry);
    let _ = std::fs::remove_file(&providers);

    assert!(
        !output.status.success(),
        "missing provider must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=missing_provider"));
    assert!(stdout.contains("all_passed=false"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("provider_not_found") || stdout.contains("reason_code=provider_not_found")
    );
}

// RC-03: missing BTC/USD Kraken alias fails closed with
// truth_state=missing_alias.
#[test]
fn rc_03_missing_kraken_alias_fails_closed() {
    let registry = write_temp_json("rc03_registry", &registry_json(false, false, false));
    let providers = write_temp_json("rc03_providers", &providers_json(false));

    let output = run(&[
        "md",
        "crypto-registry-readiness",
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--provider",
        "kraken",
        "--symbols",
        "BTC/USD",
    ]);

    let _ = std::fs::remove_file(&registry);
    let _ = std::fs::remove_file(&providers);

    assert!(
        !output.status.success(),
        "missing kraken alias must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=missing_alias"));
    assert!(stdout.contains("alias_ok=false"));
    assert!(stdout.contains("all_passed=false"));
}

// RC-04: missing ETH/USD symbol entirely (registry only has BTC/USD) fails
// closed with truth_state=missing_symbol.
#[test]
fn rc_04_missing_eth_symbol_fails_closed() {
    let registry = write_temp_json("rc04_registry", &registry_json(false, false, true));
    let providers = write_temp_json("rc04_providers", &providers_json(false));

    let output = run(&[
        "md",
        "crypto-registry-readiness",
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--provider",
        "kraken",
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    let _ = std::fs::remove_file(&registry);
    let _ = std::fs::remove_file(&providers);

    assert!(
        !output.status.success(),
        "missing ETH/USD symbol must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=missing_symbol"));
    assert!(stdout.contains("symbol=ETH/USD found=false"));
    assert!(stdout.contains("all_passed=false"));
}

// RC-05: paper_trading_enabled=true fails closed as unsafe_trading_enabled.
#[test]
fn rc_05_paper_trading_enabled_true_is_unsafe() {
    let registry = write_temp_json("rc05_registry", &registry_json(true, false, true));
    let providers = write_temp_json("rc05_providers", &providers_json(false));

    let output = run(&[
        "md",
        "crypto-registry-readiness",
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--provider",
        "kraken",
        "--symbols",
        "BTC/USD",
    ]);

    let _ = std::fs::remove_file(&registry);
    let _ = std::fs::remove_file(&providers);

    assert!(
        !output.status.success(),
        "paper_trading_enabled=true must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=unsafe_trading_enabled"));
    assert!(stdout.contains("all_passed=false"));
}

// RC-06: live_trading_enabled=true fails closed as unsafe_trading_enabled.
#[test]
fn rc_06_live_trading_enabled_true_is_unsafe() {
    let registry = write_temp_json("rc06_registry", &registry_json(false, true, true));
    let providers = write_temp_json("rc06_providers", &providers_json(false));

    let output = run(&[
        "md",
        "crypto-registry-readiness",
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--provider",
        "kraken",
        "--symbols",
        "BTC/USD",
    ]);

    let _ = std::fs::remove_file(&registry);
    let _ = std::fs::remove_file(&providers);

    assert!(
        !output.status.success(),
        "live_trading_enabled=true must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=unsafe_trading_enabled"));
    assert!(stdout.contains("all_passed=false"));
}

// RC-07: kraken.enabled=true fails closed as unsafe_provider_enabled, per
// CRYPTO-REGISTRY-02's decision not to treat provider-enablement as safe yet.
#[test]
fn rc_07_kraken_provider_enabled_true_is_unsafe() {
    let registry = write_temp_json("rc07_registry", &registry_json(false, false, true));
    let providers = write_temp_json("rc07_providers", &providers_json(true));

    let output = run(&[
        "md",
        "crypto-registry-readiness",
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--provider",
        "kraken",
        "--symbols",
        "BTC/USD",
    ]);

    let _ = std::fs::remove_file(&registry);
    let _ = std::fs::remove_file(&providers);

    assert!(
        !output.status.success(),
        "kraken.enabled=true must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=unsafe_provider_enabled"));
    assert!(stdout.contains("all_passed=false"));
}

// RC-08: evidence JSON written via --output-dir contains no DB/network/
// trading claims, and mirrors the real-fixture happy-path result.
#[test]
fn rc_08_evidence_json_contains_no_db_network_trading_claims() {
    let registry = real_registry_path();
    let providers = real_providers_path();
    let out_dir = std::env::temp_dir().join(format!(
        "mqk_cli_crypto_registry_readiness_evidence_{}",
        Uuid::new_v4()
    ));

    let output = run(&[
        "md",
        "crypto-registry-readiness",
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--provider",
        "kraken",
        "--symbols",
        "BTC/USD,ETH/USD",
        "--output-dir",
        &out_dir.to_string_lossy(),
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

    assert_eq!(value["schema_version"], "crypto-registry-readiness-v1");
    assert_eq!(value["provider"], "kraken");
    assert_eq!(value["truth_state"], "active");
    assert_eq!(value["data_readiness_state"], "data_ready_manual_only");
    assert_eq!(value["trading_readiness_state"], "disabled");
    assert_eq!(value["scheduler_readiness_state"], "absent");
    assert_eq!(value["provider_enabled"], false);
    assert_eq!(value["all_passed"], true);
    assert_eq!(value["safety"]["no_scheduler"], true);
    assert_eq!(value["safety"]["no_db_connection"], true);
    assert_eq!(value["safety"]["no_network_call"], true);
    assert_eq!(value["safety"]["no_trading_enabled"], true);
    assert_eq!(value["safety"]["no_config_file_mutated"], true);

    // No DB/network/trading claim fields anywhere in the top-level envelope.
    let forbidden_top_level_keys = [
        "db_connection_string",
        "network_response",
        "order_id",
        "broker_order_id",
        "fill_price",
        "position_id",
        "account_id",
    ];
    for key in forbidden_top_level_keys {
        assert!(
            value.get(key).is_none(),
            "evidence must not contain forbidden key '{key}'"
        );
    }

    let symbol_checks = value["symbol_checks"].as_array().unwrap();
    assert_eq!(symbol_checks.len(), 2);
    for check in symbol_checks {
        assert_eq!(check["trading_flags_safe"], true);
    }

    let _ = std::fs::remove_dir_all(&out_dir);
}

// RC-09: --registry/--providers files are never mutated by the command.
#[test]
fn rc_09_registry_and_providers_files_are_never_mutated() {
    let registry = real_registry_path();
    let providers = real_providers_path();
    let registry_before = std::fs::read_to_string(&registry).expect("read real registry fixture");
    let providers_before =
        std::fs::read_to_string(&providers).expect("read real providers fixture");

    let output = run(&[
        "md",
        "crypto-registry-readiness",
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--provider",
        "kraken",
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);
    assert!(output.status.success());

    let registry_after =
        std::fs::read_to_string(&registry).expect("read real registry fixture after run");
    let providers_after =
        std::fs::read_to_string(&providers).expect("read real providers fixture after run");
    assert_eq!(
        registry_before, registry_after,
        "registry file must be unchanged"
    );
    assert_eq!(
        providers_before, providers_after,
        "providers file must be unchanged"
    );
}
