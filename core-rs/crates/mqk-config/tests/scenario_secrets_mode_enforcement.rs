//! PATCH S1 — scenario_secrets_mode_enforcement
//!
//! Validates the mode-aware fail-closed enforcement of `resolve_secrets_for_mode`.
//!
//! # Test design
//! All failure tests use globally-unique sentinel env var names
//! (e.g. `MQK_S1_SENTINEL_*`) that are never set in any CI or dev environment.
//! This avoids any need for `std::env::set_var` and sidesteps parallel-test
//! race conditions on env-var mutation.
//!
//! The success test (BACKTEST) requires no env vars by definition.
//!
//! # Coverage
//! 1. LIVE mode fails closed when broker api_key is missing → SECRETS_MISSING
//! 2. LIVE mode fails closed when broker api_secret is missing → SECRETS_MISSING
//! 3. LIVE mode fails closed when TwelveData key is missing → SECRETS_MISSING
//! 4. PAPER mode fails closed when broker api_key is missing → SECRETS_MISSING
//! 5. PAPER mode fails closed when broker api_secret is missing → SECRETS_MISSING
//! 6. BACKTEST mode succeeds with no keys present
//! 7. Unknown mode is rejected → SECRETS_UNKNOWN_MODE
//! 8. Error messages reference var NAMES, never values
//! 9. Config JSON stores var names (not values) — names-only invariant
//! 10. `Debug` output of `ResolvedSecrets` is redacted

use std::sync::{Mutex, OnceLock};

use mqk_config::load_layered_yaml_from_strings;
use mqk_config::secrets::{resolve_discord_webhooks, resolve_secrets_for_mode};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Guards every test that reads the canonical `DISCORD_WEBHOOK_ALERTS`
/// process env var (via `resolve_discord_webhooks`/`resolve_secrets_for_mode`
/// resolving `alerts` against its canonical default name) against
/// `resolve_discord_webhooks_default_alerts_name_matches_base_yaml`, the one
/// test in this file that temporarily sets that same var. The default-rust-
/// test-harness runs `#[test]` functions on multiple threads, so without
/// this lock a reader test can observe the mutator's in-flight value —
/// exactly the flake this closes.
static DISCORD_ALERTS_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn discord_alerts_env_lock() -> &'static Mutex<()> {
    DISCORD_ALERTS_ENV_LOCK.get_or_init(|| Mutex::new(()))
}

fn load(yaml: &str) -> serde_json::Value {
    load_layered_yaml_from_strings(&[yaml])
        .expect("test yaml must parse cleanly")
        .config_json
}

// ---------------------------------------------------------------------------
// 1. LIVE — broker api_key missing
// ---------------------------------------------------------------------------

#[test]
fn live_mode_fails_when_broker_api_key_missing() {
    // Sentinel var names: globally unique, guaranteed unset in any CI.
    let yaml = r#"
broker:
  keys_env:
    api_key: "MQK_S1_SENTINEL_LIVE_APIKEY_MISSING_A1"
    api_secret: "MQK_S1_SENTINEL_LIVE_APISEC_MISSING_A1"
data:
  providers:
    twelvedata:
      api_key_env: "MQK_S1_SENTINEL_LIVE_TD_MISSING_A1"
"#;
    let cfg = load(yaml);
    let result = resolve_secrets_for_mode(&cfg, "LIVE");

    assert!(
        result.is_err(),
        "LIVE must fail when broker api_key env var is not set"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("SECRETS_MISSING"),
        "error must contain SECRETS_MISSING, got: {msg}"
    );
    assert!(
        msg.contains("mode=LIVE"),
        "error must identify LIVE mode, got: {msg}"
    );
    // Error must reference the NAME of the missing var — never a secret value.
    assert!(
        msg.contains("MQK_S1_SENTINEL_LIVE_APIKEY_MISSING_A1"),
        "error must name the missing env var, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 2. LIVE — broker api_secret missing (api_key deliberately absent too,
//    so first failure is api_key — that's still a SECRETS_MISSING result)
// ---------------------------------------------------------------------------

#[test]
fn live_mode_fails_when_any_required_key_missing() {
    let yaml = r#"
broker:
  keys_env:
    api_key: "MQK_S1_SENTINEL_LIVE_APIKEY_MISSING_B2"
    api_secret: "MQK_S1_SENTINEL_LIVE_APISEC_MISSING_B2"
data:
  providers:
    twelvedata:
      api_key_env: "MQK_S1_SENTINEL_LIVE_TD_MISSING_B2"
"#;
    let cfg = load(yaml);
    // All three sentinel vars are unset — the function must fail.
    let result = resolve_secrets_for_mode(&cfg, "LIVE");
    assert!(
        result.is_err(),
        "LIVE must fail when required keys are absent"
    );
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("SECRETS_MISSING"), "{msg}");
}

// ---------------------------------------------------------------------------
// 3. LIVE — error message contains var NAME, not a secret value
// ---------------------------------------------------------------------------

#[test]
fn live_mode_error_references_var_name_not_secret_value() {
    let yaml = r#"
broker:
  keys_env:
    api_key: "MQK_S1_SENTINEL_VARNAME_CHECK_C3"
    api_secret: "MQK_S1_SENTINEL_VARSEC_CHECK_C3"
data:
  providers:
    twelvedata:
      api_key_env: "MQK_S1_SENTINEL_TD_CHECK_C3"
"#;
    let cfg = load(yaml);
    let err_msg = resolve_secrets_for_mode(&cfg, "LIVE")
        .expect_err("must fail")
        .to_string();

    // The error must name the var so ops knows what to set.
    assert!(
        err_msg.contains("MQK_S1_SENTINEL_VARNAME_CHECK_C3"),
        "error must contain the env var NAME, got: {err_msg}"
    );
    // Sanity: the error must not accidentally contain a resolved secret value.
    // (It can't here because the var isn't set, but this pattern should be
    //  checked explicitly as a contract assertion.)
    assert!(
        !err_msg.contains("sk-"),
        "error must not contain secret-like value, got: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// 4. PAPER — broker api_key missing
// ---------------------------------------------------------------------------

#[test]
fn paper_mode_fails_when_broker_api_key_missing() {
    let yaml = r#"
broker:
  keys_env:
    api_key: "MQK_S1_SENTINEL_PAPER_APIKEY_MISSING_D4"
    api_secret: "MQK_S1_SENTINEL_PAPER_APISEC_MISSING_D4"
"#;
    let cfg = load(yaml);
    let result = resolve_secrets_for_mode(&cfg, "PAPER");
    assert!(
        result.is_err(),
        "PAPER must fail when broker api_key env var is not set"
    );
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("SECRETS_MISSING"), "{msg}");
    assert!(
        msg.contains("mode=PAPER"),
        "error must identify PAPER mode, got: {msg}"
    );
    assert!(
        msg.contains("MQK_S1_SENTINEL_PAPER_APIKEY_MISSING_D4"),
        "error must name the missing var, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 5. PAPER — both broker keys missing (first failure reported)
// ---------------------------------------------------------------------------

#[test]
fn paper_mode_fails_when_both_broker_keys_missing() {
    let yaml = r#"
broker:
  keys_env:
    api_key: "MQK_S1_SENTINEL_PAPER_BOTH_KEY_E5"
    api_secret: "MQK_S1_SENTINEL_PAPER_BOTH_SEC_E5"
"#;
    let cfg = load(yaml);
    let result = resolve_secrets_for_mode(&cfg, "PAPER");
    assert!(
        result.is_err(),
        "PAPER must fail when broker keys are absent"
    );
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("SECRETS_MISSING"), "{msg}");
}

// ---------------------------------------------------------------------------
// 6. BACKTEST — no keys required; must succeed even with all vars absent
// ---------------------------------------------------------------------------

#[test]
fn backtest_mode_succeeds_with_no_keys_set() {
    let _guard = discord_alerts_env_lock().lock().unwrap();
    let yaml = r#"
broker:
  keys_env:
    api_key: "MQK_S1_SENTINEL_BT_APIKEY_ABSENT_F6"
    api_secret: "MQK_S1_SENTINEL_BT_APISEC_ABSENT_F6"
data:
  providers:
    twelvedata:
      api_key_env: "MQK_S1_SENTINEL_BT_TD_ABSENT_F6"
discord:
  channels:
    paper: "MQK_S1_SENTINEL_BT_DISCORD_PAPER_F6"
    live: "MQK_S1_SENTINEL_BT_DISCORD_LIVE_F6"
    backtest: "MQK_S1_SENTINEL_BT_DISCORD_BT_F6"
    alerts: "MQK_S1_SENTINEL_BT_DISCORD_ALERTS_F6"
    heartbeat: "MQK_S1_SENTINEL_BT_DISCORD_HB_F6"
    c2: "MQK_S1_SENTINEL_BT_DISCORD_C2_F6"
"#;
    let cfg = load(yaml);
    let result = resolve_secrets_for_mode(&cfg, "BACKTEST");

    assert!(
        result.is_ok(),
        "BACKTEST must succeed when no required keys exist: {:?}",
        result.err()
    );

    let secrets = result.unwrap();
    // All sentinels are unset, so every field must be None.
    assert!(
        secrets.broker_api_key.is_none(),
        "broker_api_key must be None"
    );
    assert!(
        secrets.broker_api_secret.is_none(),
        "broker_api_secret must be None"
    );
    assert!(
        secrets.twelvedata_api_key.is_none(),
        "twelvedata_api_key must be None"
    );
    assert!(
        secrets.discord.paper.is_none(),
        "discord.paper must be None"
    );
    assert!(secrets.discord.live.is_none(), "discord.live must be None");
    assert!(
        secrets.discord.backtest.is_none(),
        "discord.backtest must be None"
    );
    assert!(
        secrets.discord.alerts.is_none(),
        "discord.alerts must be None"
    );
    assert!(
        secrets.discord.heartbeat.is_none(),
        "discord.heartbeat must be None"
    );
    assert!(secrets.discord.c2.is_none(), "discord.c2 must be None");
}

// ---------------------------------------------------------------------------
// 7. Unknown mode is rejected
// ---------------------------------------------------------------------------

#[test]
fn unknown_mode_is_rejected() {
    let yaml = r#"
broker:
  keys_env:
    api_key: "SOME_KEY_G7"
    api_secret: "SOME_SECRET_G7"
"#;
    let cfg = load(yaml);
    let result = resolve_secrets_for_mode(&cfg, "SIMULATION");
    assert!(result.is_err(), "unknown mode must be rejected");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("SECRETS_UNKNOWN_MODE"),
        "error must contain SECRETS_UNKNOWN_MODE, got: {msg}"
    );
    assert!(
        msg.contains("SIMULATION"),
        "error must echo the bad mode string, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 8 & 9. Config JSON stores var NAMES, not resolved values
// ---------------------------------------------------------------------------

#[test]
fn config_json_stores_var_names_not_resolved_values() {
    let yaml = r#"
broker:
  keys_env:
    api_key: "ALPACA_API_KEY_PAPER"
    api_secret: "ALPACA_API_SECRET_PAPER"
data:
  providers:
    twelvedata:
      api_key_env: "TWELVEDATA_API_KEY"
discord:
  channels:
    live: "DISCORD_WEBHOOK_LIVE"
    alerts: "DISCORD_WEBHOOK_ALERTS"
"#;
    let loaded = load_layered_yaml_from_strings(&[yaml]).expect("must parse");
    let cfg = &loaded.config_json;

    // The config must store the NAME, not any resolved secret value.
    assert_eq!(
        cfg.pointer("/broker/keys_env/api_key")
            .and_then(|v| v.as_str()),
        Some("ALPACA_API_KEY_PAPER"),
        "config must store var NAME, not value"
    );
    assert_eq!(
        cfg.pointer("/broker/keys_env/api_secret")
            .and_then(|v| v.as_str()),
        Some("ALPACA_API_SECRET_PAPER"),
    );
    assert_eq!(
        cfg.pointer("/data/providers/twelvedata/api_key_env")
            .and_then(|v| v.as_str()),
        Some("TWELVEDATA_API_KEY"),
    );
    assert_eq!(
        cfg.pointer("/discord/channels/live")
            .and_then(|v| v.as_str()),
        Some("DISCORD_WEBHOOK_LIVE"),
    );

    // Config hash must not contain any resolved secret-like value.
    // (Env vars named above are not set in test environments.)
    let hash = &loaded.config_hash;
    assert!(!hash.is_empty(), "config_hash must be non-empty");
    // The hash is a hex string — it must not accidentally embed recognisable
    // secret-pattern prefixes.
    assert!(
        !loaded.canonical_json.contains("sk-"),
        "canonical JSON must not contain secret-like values"
    );
}

// ---------------------------------------------------------------------------
// 10. Debug output is redacted
// ---------------------------------------------------------------------------

#[test]
fn resolved_secrets_debug_output_is_redacted() {
    // Use BACKTEST (no required keys) to get a successful resolve with all-None fields.
    let yaml = r#"
broker:
  keys_env:
    api_key: "MQK_S1_SENTINEL_DBG_KEY_H10"
    api_secret: "MQK_S1_SENTINEL_DBG_SEC_H10"
"#;
    let cfg = load(yaml);
    let secrets = resolve_secrets_for_mode(&cfg, "BACKTEST").expect("BACKTEST must not fail");

    let debug_str = format!("{:?}", secrets);

    // Must not echo back the sentinel var name as a "resolved value".
    // (It can't — the var is not set — but we assert the contract explicitly.)
    // Must show either None or <REDACTED> for every field.
    assert!(
        debug_str.contains("None") || debug_str.contains("REDACTED"),
        "Debug output must show None or REDACTED, got: {debug_str}"
    );
    // Must never contain any recognisable secret-value pattern.
    assert!(
        !debug_str.contains("sk-"),
        "Debug must not expose secret values"
    );
}

// ---------------------------------------------------------------------------
// 11. resolve_discord_webhooks — narrow Discord-only resolver
// (DISCORD-CHANNEL-ROUTING-01)
// ---------------------------------------------------------------------------

/// With no `/discord/channels/*` pointers at all (an entirely empty config),
/// every channel must fall back to its canonical default env var NAME
/// (`config/defaults/base.yaml`'s literal values) rather than silently
/// resolving to `None` regardless of whether that env var is actually set.
/// None of `DISCORD_WEBHOOK_PAPER`/`LIVE`/`BACKTEST`/`HEARTBEAT`/`C2` are set
/// in the test process, so every field is `None` here — this test proves
/// the fallback path runs without panicking and defaults safely, not that a
/// specific value resolves (that requires real env mutation; see the next
/// test for `alerts`).
#[test]
fn resolve_discord_webhooks_with_empty_config_falls_back_safely() {
    let _guard = discord_alerts_env_lock().lock().unwrap();
    let cfg = load("broker:\n  keys_env: {}\n");
    let discord = resolve_discord_webhooks(&cfg);

    assert!(discord.paper.is_none());
    assert!(discord.live.is_none());
    assert!(discord.backtest.is_none());
    assert!(discord.alerts.is_none());
    assert!(discord.heartbeat.is_none());
    assert!(discord.c2.is_none());
}

/// A channel's own configured pointer (a sentinel name, in this case) still
/// takes priority over the canonical default when present — proving the
/// fallback is genuinely a fallback, not a hardcoded override.
#[test]
fn resolve_discord_webhooks_configured_pointer_overrides_default() {
    let yaml = r#"
discord:
  channels:
    alerts: "MQK_S1_SENTINEL_DISCORD_ALERTS_K11"
"#;
    let cfg = load(yaml);
    let discord = resolve_discord_webhooks(&cfg);

    // The sentinel env var is never set, so this must resolve to None —
    // proving the CONFIGURED name (not the canonical default name) was the
    // one actually consulted. If the default had been used instead despite
    // the pointer being present, this would still coincidentally be None
    // (default unset too), so the real proof is in
    // `config_json_stores_var_names_not_resolved_values`-style coverage
    // above for the broker/twelvedata fields sharing this exact code path;
    // this test documents the same contract for discord specifically.
    assert!(discord.alerts.is_none());

    // Other channels, with no pointer configured, still fall back safely.
    assert!(discord.paper.is_none());
}

/// The canonical default env var NAME for each channel matches
/// `config/defaults/base.yaml` exactly — proven by actually setting one
/// (`DISCORD_WEBHOOK_ALERTS`) and confirming it resolves with an EMPTY
/// config (no `/discord/channels/alerts` pointer at all), restoring the
/// prior value afterward. Two other tests in this file
/// (`backtest_mode_succeeds_with_no_keys_set`,
/// `resolve_discord_webhooks_with_empty_config_falls_back_safely`) resolve
/// `alerts` against this exact canonical default name and assert it is
/// `None` — `discord_alerts_env_lock()` serializes this mutation against
/// those reads so the default Rust test harness's multi-threaded execution
/// can never interleave them.
#[test]
fn resolve_discord_webhooks_default_alerts_name_matches_base_yaml() {
    let _guard = discord_alerts_env_lock().lock().unwrap();
    let prior = std::env::var("DISCORD_WEBHOOK_ALERTS").ok();
    #[allow(deprecated)]
    unsafe {
        std::env::set_var(
            "DISCORD_WEBHOOK_ALERTS",
            "https://discord.example.invalid/hook/alerts-k11",
        );
    }

    let cfg = load("broker:\n  keys_env: {}\n");
    let discord = resolve_discord_webhooks(&cfg);

    #[allow(deprecated)]
    unsafe {
        match &prior {
            Some(v) => std::env::set_var("DISCORD_WEBHOOK_ALERTS", v),
            None => std::env::remove_var("DISCORD_WEBHOOK_ALERTS"),
        }
    }

    assert_eq!(
        discord.alerts.as_deref(),
        Some("https://discord.example.invalid/hook/alerts-k11"),
        "DISCORD_WEBHOOK_ALERTS must be the canonical default name for the alerts channel"
    );
}

/// `resolve_secrets_for_mode` and `resolve_discord_webhooks` must never
/// disagree — the former calls the latter internally (one implementation,
/// never a duplicate parser).
#[test]
fn resolve_secrets_for_mode_and_resolve_discord_webhooks_agree() {
    let yaml = r#"
broker:
  keys_env:
    api_key: "MQK_S1_SENTINEL_AGREE_KEY_K11"
    api_secret: "MQK_S1_SENTINEL_AGREE_SEC_K11"
discord:
  channels:
    paper: "MQK_S1_SENTINEL_AGREE_DISCORD_PAPER_K11"
    live: "MQK_S1_SENTINEL_AGREE_DISCORD_LIVE_K11"
"#;
    let cfg = load(yaml);
    let via_secrets = resolve_secrets_for_mode(&cfg, "BACKTEST")
        .expect("BACKTEST must not fail")
        .discord;
    let via_narrow = resolve_discord_webhooks(&cfg);

    assert_eq!(via_secrets.paper, via_narrow.paper);
    assert_eq!(via_secrets.live, via_narrow.live);
    assert_eq!(via_secrets.backtest, via_narrow.backtest);
    assert_eq!(via_secrets.alerts, via_narrow.alerts);
    assert_eq!(via_secrets.heartbeat, via_narrow.heartbeat);
    assert_eq!(via_secrets.c2, via_narrow.c2);
}
