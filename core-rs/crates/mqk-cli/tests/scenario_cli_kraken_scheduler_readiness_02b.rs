//! CRYPTO-DATA-02B-KRAKEN-SCHEDULER-READINESS-CLI-01.
//!
//! Proves `mqk-cli md kraken-scheduler-readiness` is a read-only readiness
//! surface for a *future*, not-yet-authorized Kraken scheduled sync: it
//! never opens a DB connection, never makes a provider/network call, never
//! mutates `--policy`/`--registry`/`--providers`, never registers a
//! scheduler, classifies the current (disabled/policy-compliant) fixture
//! state as `active` / `scheduler_ready_manual_registration_blocked` (never
//! implying a scheduler is actually registered), and fails closed on a
//! missing/invalid policy, an unsafe provider/registry/trading-flag state,
//! or (only when explicitly required) unsafe Kraken evidence.
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

fn real_policy_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join(
        "../../../docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json",
    )
}

fn write_temp_json(label: &str, content: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_cli_kraken_scheduler_readiness_{label}_{}.json",
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

/// A registry-v2 fixture carrying BTC/USD and ETH/USD crypto rows with
/// configurable Kraken aliases and trading flags.
#[allow(clippy::too_many_arguments)]
fn registry_json(
    btc_alias: bool,
    eth_alias: bool,
    btc_paper: bool,
    btc_live: bool,
    eth_paper: bool,
    eth_live: bool,
) -> String {
    let btc_symbols = if btc_alias {
        r#"{"kraken_pair": "XBTUSD", "kraken_result_key": "XXBTZUSD"}"#
    } else {
        "{}"
    };
    let eth_symbols = if eth_alias {
        r#"{"kraken_pair": "ETHUSD", "kraken_result_key": "XETHZUSD"}"#
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
      "provider_symbols": {btc_symbols},
      "enabled": false,
      "paper_trading_enabled": {btc_paper},
      "live_trading_enabled": {btc_live},
      "timeframes": ["1D"],
      "contract": {{"kind": "crypto_pair", "base": "BTC", "quote": "USD"}}
    }},
    {{
      "instrument_id": "crypto:GLOBAL:ETHUSD",
      "symbol": "ETH/USD",
      "asset_class": "crypto",
      "currency": "USD",
      "quote_currency": "USD",
      "provider_symbols": {eth_symbols},
      "enabled": false,
      "paper_trading_enabled": {eth_paper},
      "live_trading_enabled": {eth_live},
      "timeframes": ["1D"],
      "contract": {{"kind": "crypto_pair", "base": "ETH", "quote": "USD"}}
    }}
  ]
}}"#
    )
}

/// A `CRYPTO-DATA-02A` policy fixture with configurable cadence fields and
/// `scheduler_registration_status`, otherwise contract-compliant.
fn policy_json(
    min_seconds_between_pair_calls: i64,
    min_seconds_between_scheduled_runs: i64,
    scheduler_registration_status: &str,
) -> String {
    format!(
        r#"{{
  "schema_version": "crypto-data-02a-kraken-scheduler-rate-limit-decision-v1",
  "patch_id": "CRYPTO-DATA-02A-KRAKEN-SCHEDULER-RATE-LIMIT-DECISION-01",
  "decision": "scheduler_not_registered_readiness_only",
  "provider": "kraken",
  "symbols": ["BTC/USD", "ETH/USD"],
  "timeframe": "1D",
  "rate_limit_verification_status": "verified",
  "recommended_default_cadence": "daily",
  "min_seconds_between_pair_calls": {min_seconds_between_pair_calls},
  "min_seconds_between_scheduled_runs": {min_seconds_between_scheduled_runs},
  "max_ohlc_calls_per_run": 2,
  "max_total_network_calls_per_run": 2,
  "concurrency": "sequential_only",
  "scheduler_registration_status": "{scheduler_registration_status}",
  "safety": {{
    "no_scheduled_task_registered": true,
    "no_daemon_job_added": true,
    "no_network_call_to_kraken_api": true,
    "no_db_write": true,
    "no_trading_enabled": true,
    "kraken_provider_enabled": false,
    "paper_trading_enabled": false,
    "live_trading_enabled": false
  }}
}}"#
    )
}

fn cleanup(paths: &[&PathBuf]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}

// KS-01: happy path against the real, committed policy/registry/providers
// files -> active / scheduler_ready_manual_registration_blocked. `active`
// must never be conflated with "a scheduler is registered".
#[test]
fn ks_01_happy_path_real_fixtures_is_active_but_not_registered() {
    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &real_policy_path().to_string_lossy(),
        "--registry",
        &real_registry_path().to_string_lossy(),
        "--providers",
        &real_providers_path().to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    assert!(
        output.status.success(),
        "readiness check must succeed against real fixtures\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("truth_state=active"));
    assert!(stdout.contains(
        "scheduler_readiness_state=scheduler_ready_manual_registration_blocked"
    ));
    assert!(stdout.contains("scheduler_registration_status=not_registered"));
    assert!(stdout.contains("daemon_job_status=absent"));
    assert!(stdout.contains("network_call_made=false"));
    assert!(stdout.contains("db_write=false"));
    assert!(stdout.contains("trading_enabled=false"));
    assert!(stdout.contains("all_passed=true"));
}

// KS-02: missing --policy file fails closed with truth_state=policy_missing.
#[test]
fn ks_02_missing_policy_file_fails_closed() {
    let missing_policy = std::env::temp_dir().join(format!(
        "mqk_cli_kraken_scheduler_readiness_missing_policy_{}.json",
        Uuid::new_v4()
    ));

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &missing_policy.to_string_lossy(),
        "--registry",
        &real_registry_path().to_string_lossy(),
        "--providers",
        &real_providers_path().to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    assert!(
        !output.status.success(),
        "missing policy file must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=policy_missing"));
    assert!(stdout.contains("all_passed=false"));
}

// KS-03: policy with too-fast cadence fails closed as policy_invalid.
#[test]
fn ks_03_policy_too_fast_cadence_is_invalid() {
    let policy = write_temp_json("ks03_policy", &policy_json(1, 86_400, "not_registered"));

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &policy.to_string_lossy(),
        "--registry",
        &real_registry_path().to_string_lossy(),
        "--providers",
        &real_providers_path().to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    cleanup(&[&policy]);

    assert!(
        !output.status.success(),
        "too-fast cadence must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=policy_invalid"));
    assert!(stdout.contains("rate_limit_policy_state=invalid"));
    assert!(stdout.contains("all_passed=false"));
}

// KS-04: kraken.enabled=true fails closed as provider_unsafe.
#[test]
fn ks_04_kraken_provider_enabled_true_is_unsafe() {
    let policy = write_temp_json(
        "ks04_policy",
        &policy_json(2, 86_400, "not_registered"),
    );
    let registry = write_temp_json(
        "ks04_registry",
        &registry_json(true, true, false, false, false, false),
    );
    let providers = write_temp_json("ks04_providers", &providers_json(true));

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &policy.to_string_lossy(),
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    cleanup(&[&policy, &registry, &providers]);

    assert!(
        !output.status.success(),
        "kraken.enabled=true must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=provider_unsafe"));
    assert!(stdout.contains("all_passed=false"));
}

// KS-05: BTC/USD paper_trading_enabled=true fails closed as
// trading_flags_unsafe.
#[test]
fn ks_05_btc_paper_trading_enabled_true_is_unsafe() {
    let policy = write_temp_json("ks05_policy", &policy_json(2, 86_400, "not_registered"));
    let registry = write_temp_json(
        "ks05_registry",
        &registry_json(true, true, true, false, false, false),
    );
    let providers = write_temp_json("ks05_providers", &providers_json(false));

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &policy.to_string_lossy(),
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    cleanup(&[&policy, &registry, &providers]);

    assert!(
        !output.status.success(),
        "BTC paper_trading_enabled=true must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=trading_flags_unsafe"));
    assert!(stdout.contains("all_passed=false"));
}

// KS-06: ETH/USD live_trading_enabled=true fails closed as
// trading_flags_unsafe.
#[test]
fn ks_06_eth_live_trading_enabled_true_is_unsafe() {
    let policy = write_temp_json("ks06_policy", &policy_json(2, 86_400, "not_registered"));
    let registry = write_temp_json(
        "ks06_registry",
        &registry_json(true, true, false, false, false, true),
    );
    let providers = write_temp_json("ks06_providers", &providers_json(false));

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &policy.to_string_lossy(),
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    cleanup(&[&policy, &registry, &providers]);

    assert!(
        !output.status.success(),
        "ETH live_trading_enabled=true must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=trading_flags_unsafe"));
    assert!(stdout.contains("all_passed=false"));
}

// KS-07: missing BTC/USD Kraken alias fails closed as registry_unsafe.
#[test]
fn ks_07_missing_btc_alias_is_registry_unsafe() {
    let policy = write_temp_json("ks07_policy", &policy_json(2, 86_400, "not_registered"));
    let registry = write_temp_json(
        "ks07_registry",
        &registry_json(false, true, false, false, false, false),
    );
    let providers = write_temp_json("ks07_providers", &providers_json(false));

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &policy.to_string_lossy(),
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    cleanup(&[&policy, &registry, &providers]);

    assert!(
        !output.status.success(),
        "missing BTC alias must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=registry_unsafe"));
    assert!(stdout.contains("symbol=BTC/USD found=true asset_class_ok=true alias_ok=false"));
    assert!(stdout.contains("all_passed=false"));
}

// KS-08: missing ETH/USD Kraken alias fails closed as registry_unsafe.
#[test]
fn ks_08_missing_eth_alias_is_registry_unsafe() {
    let policy = write_temp_json("ks08_policy", &policy_json(2, 86_400, "not_registered"));
    let registry = write_temp_json(
        "ks08_registry",
        &registry_json(true, false, false, false, false, false),
    );
    let providers = write_temp_json("ks08_providers", &providers_json(false));

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &policy.to_string_lossy(),
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);

    cleanup(&[&policy, &registry, &providers]);

    assert!(
        !output.status.success(),
        "missing ETH alias must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=registry_unsafe"));
    assert!(stdout.contains("symbol=ETH/USD found=true asset_class_ok=true alias_ok=false"));
    assert!(stdout.contains("all_passed=false"));
}

// KS-09: unsafe latest Kraken evidence (all_passed=false in the evidence
// file) fails closed as evidence_unsafe only when --require-fresh-evidence
// is explicitly set.
#[test]
fn ks_09_unsafe_evidence_fails_closed_when_required() {
    let evidence_dir = std::env::temp_dir().join(format!(
        "mqk_cli_kraken_scheduler_readiness_evidence_unsafe_{}",
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&evidence_dir).expect("create evidence dir");
    let evidence_file = evidence_dir.join("kraken_ohlc_sync_1700000000.json");
    std::fs::write(
        &evidence_file,
        r#"{"schema_version":"kraken-ohlc-sync-v2","provider":"kraken","all_passed":false}"#,
    )
    .expect("write unsafe evidence fixture");

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &real_policy_path().to_string_lossy(),
        "--registry",
        &real_registry_path().to_string_lossy(),
        "--providers",
        &real_providers_path().to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
        "--evidence-dir",
        &evidence_dir.to_string_lossy(),
        "--require-fresh-evidence",
    ]);

    let _ = std::fs::remove_dir_all(&evidence_dir);

    assert!(
        !output.status.success(),
        "unsafe evidence with --require-fresh-evidence must fail closed with nonzero exit"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("truth_state=evidence_unsafe"));
    assert!(stdout.contains("evidence_readiness_state=unsafe"));
    assert!(stdout.contains("all_passed=false"));
}

// KS-10: evidence missing (empty --evidence-dir) without
// --require-fresh-evidence is only a warning; the command still reports
// active and exits 0.
#[test]
fn ks_10_missing_evidence_not_required_is_warning_only() {
    let evidence_dir = std::env::temp_dir().join(format!(
        "mqk_cli_kraken_scheduler_readiness_evidence_missing_{}",
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&evidence_dir).expect("create empty evidence dir");

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &real_policy_path().to_string_lossy(),
        "--registry",
        &real_registry_path().to_string_lossy(),
        "--providers",
        &real_providers_path().to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
        "--evidence-dir",
        &evidence_dir.to_string_lossy(),
    ]);

    let _ = std::fs::remove_dir_all(&evidence_dir);

    assert!(
        output.status.success(),
        "missing evidence without --require-fresh-evidence must not fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("truth_state=active"));
    assert!(stdout.contains("evidence_readiness_state=missing"));
    assert!(stdout.contains("all_passed=true"));
    assert!(
        stdout.lines().any(|l| l.starts_with("warning=")),
        "missing-but-not-required evidence must still surface a warning line, stdout:\n{stdout}"
    );
}

// KS-11: evidence JSON written via --output-dir contains no DB/network/
// trading claims and mirrors the real-fixture happy-path result.
#[test]
fn ks_11_evidence_json_contains_no_db_network_trading_claims() {
    let out_dir = std::env::temp_dir().join(format!(
        "mqk_cli_kraken_scheduler_readiness_output_{}",
        Uuid::new_v4()
    ));

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &real_policy_path().to_string_lossy(),
        "--registry",
        &real_registry_path().to_string_lossy(),
        "--providers",
        &real_providers_path().to_string_lossy(),
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
        .find(|l| l.starts_with("evidence_path_out="))
        .expect("stdout must print evidence_path_out=...");
    let evidence_path = PathBuf::from(evidence_line.trim_start_matches("evidence_path_out="));

    let content = std::fs::read_to_string(&evidence_path).expect("evidence file must exist");
    let value: serde_json::Value =
        serde_json::from_str(&content).expect("evidence must be valid JSON");

    assert_eq!(value["schema_version"], "kraken-scheduler-readiness-v1");
    assert_eq!(value["provider"], "kraken");
    assert_eq!(value["truth_state"], "active");
    assert_eq!(
        value["scheduler_readiness_state"],
        "scheduler_ready_manual_registration_blocked"
    );
    assert_eq!(value["scheduler_registration_status"], "not_registered");
    assert_eq!(value["daemon_job_status"], "absent");
    assert_eq!(value["network_call_made"], false);
    assert_eq!(value["db_write"], false);
    assert_eq!(value["trading_enabled"], false);
    assert_eq!(value["all_passed"], true);
    assert_eq!(value["safety"]["no_scheduled_task_registered"], true);
    assert_eq!(value["safety"]["no_daemon_job_added"], true);
    assert_eq!(value["safety"]["no_network_call_made"], true);
    assert_eq!(value["safety"]["no_db_connection"], true);
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

    let _ = std::fs::remove_dir_all(&out_dir);
}

// KS-12: --policy/--registry/--providers files are never mutated by the
// command.
#[test]
fn ks_12_input_files_are_never_mutated() {
    let policy = real_policy_path();
    let registry = real_registry_path();
    let providers = real_providers_path();
    let policy_before = std::fs::read_to_string(&policy).expect("read real policy fixture");
    let registry_before =
        std::fs::read_to_string(&registry).expect("read real registry fixture");
    let providers_before =
        std::fs::read_to_string(&providers).expect("read real providers fixture");

    let output = run(&[
        "md",
        "kraken-scheduler-readiness",
        "--policy",
        &policy.to_string_lossy(),
        "--registry",
        &registry.to_string_lossy(),
        "--providers",
        &providers.to_string_lossy(),
        "--symbols",
        "BTC/USD,ETH/USD",
    ]);
    assert!(output.status.success());

    assert_eq!(
        policy_before,
        std::fs::read_to_string(&policy).unwrap(),
        "policy file must be unchanged"
    );
    assert_eq!(
        registry_before,
        std::fs::read_to_string(&registry).unwrap(),
        "registry file must be unchanged"
    );
    assert_eq!(
        providers_before,
        std::fs::read_to_string(&providers).unwrap(),
        "providers file must be unchanged"
    );
}
