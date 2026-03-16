//! Scenario: Deterministic Replay Proof — I9-4
//!
//! # Invariants under test
//!
//! Given the same broker event log and an injected (fixed) timestamp, two
//! independent in-process replay runs produce:
//!
//! - **S1** — Identical portfolio state (`PortfolioState` field-level equality).
//! - **S2** — Byte-identical audit log and identical audit chain hash.
//! - **S3** — Byte-identical `manifest.json` from `init_run_artifacts`.
//!
//! All scenarios are pure in-memory / temp-file: no live DB, no live broker,
//! no wall-clock time reads.
//!
//! ## Why this matters
//!
//! After an unexpected restart the operator can replay the same inbox log and
//! verify that:
//! - Portfolio state converges to the exact same value (no silent drift).
//! - The audit chain final hash is identical (chain integrity provable without
//!   a live process).
//! - The run manifest is deterministically reproducible.

use chrono::DateTime;
use mqk_artifacts::{init_run_artifacts, InitRunArtifactsArgs};
use mqk_audit::{verify_hash_chain, AuditWriter, DurabilityPolicy, VerifyResult};
use mqk_portfolio::{apply_entry, Fill, LedgerEntry, PortfolioState, Side, MICROS_SCALE};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use tempfile::tempdir;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixed deterministic inputs — never wall-clock, never RNG.
// ---------------------------------------------------------------------------

/// Fixed run ID — deterministic across all test runs.
const RUN_ID_STR: &str = "19400001-0000-0000-0000-000000000000";

/// Fixed creation timestamp — injected wherever `now_utc` is required.
const FIXED_TS_STR: &str = "2025-01-15T09:30:00Z";

fn fixed_ts() -> DateTime<chrono::Utc> {
    FIXED_TS_STR.parse().expect("fixed_ts: parse failed")
}

fn run_id() -> Uuid {
    RUN_ID_STR.parse().expect("run_id: parse failed")
}

// ---------------------------------------------------------------------------
// Canonical inbox log — a fixed sequence of broker event payloads.
//
// Each entry mirrors what `inbox_insert_deduped` / `fetch_events` would
// write: an arbitrary JSON payload representing a broker event.
// ---------------------------------------------------------------------------

fn make_inbox_log() -> Vec<Value> {
    vec![
        json!({
            "type": "ack",
            "broker_message_id": "msg-001",
            "internal_order_id": "ord-001"
        }),
        json!({
            "type":              "fill",
            "broker_message_id": "msg-002",
            "internal_order_id": "ord-001",
            "symbol":            "SPY",
            "side":              "Buy",
            "qty":               10_i64,
            "price_micros":      100_000_000_i64,
            "fee_micros":        0_i64
        }),
        json!({
            "type": "ack",
            "broker_message_id": "msg-003",
            "internal_order_id": "ord-002"
        }),
        json!({
            "type":              "fill",
            "broker_message_id": "msg-004",
            "internal_order_id": "ord-002",
            "symbol":            "AAPL",
            "side":              "Buy",
            "qty":               5_i64,
            "price_micros":      150_000_000_i64,
            "fee_micros":        500_000_i64
        }),
    ]
}

// ---------------------------------------------------------------------------
// Helpers — pure deterministic replayers.
// ---------------------------------------------------------------------------

/// Extract a `Fill` from an inbox event JSON, or `None` for non-fill events.
fn extract_fill(ev: &Value) -> Option<Fill> {
    if ev["type"].as_str() != Some("fill") {
        return None;
    }
    let qty = ev["qty"].as_i64()?;
    let price = ev["price_micros"].as_i64()?;
    let fee = ev["fee_micros"].as_i64().unwrap_or(0);
    let sym = ev["symbol"].as_str()?;
    let side = match ev["side"].as_str()? {
        "Buy" => Side::Buy,
        _ => Side::Sell,
    };
    Some(Fill::new(sym, side, qty, price, fee))
}

/// Apply the inbox log to a fresh portfolio; return the final state.
fn replay_portfolio(log: &[Value]) -> PortfolioState {
    let mut state = PortfolioState::new(1_000_000 * MICROS_SCALE);
    for ev in log {
        if let Some(fill) = extract_fill(ev) {
            apply_entry(&mut state, LedgerEntry::Fill(fill));
        }
    }
    state
}

/// Write the inbox log into an `AuditWriter` using the fixed timestamp.
///
/// Returns the final chain hash (last `hash_self`).
fn replay_audit(log: &[Value], dir: &Path) -> String {
    let mut writer = AuditWriter::with_durability(
        dir.join("audit.jsonl"),
        true, // hash_chain enabled
        DurabilityPolicy::permissive(),
    )
    .expect("AuditWriter::with_durability failed");

    for ev in log {
        writer
            .append_at(run_id(), "inbox", "event", ev.clone(), fixed_ts())
            .expect("append_at failed");
    }

    writer
        .last_hash()
        .expect("audit log must have at least one event")
}

// ---------------------------------------------------------------------------
// S1: Portfolio state is identical across two independent replays.
// ---------------------------------------------------------------------------

/// Two in-process replays of the same inbox log must produce identical
/// `PortfolioState` — field-level equality on all accounting values.
#[test]
fn portfolio_replay_is_deterministic() {
    let log = make_inbox_log();

    let state_a = replay_portfolio(&log);
    let state_b = replay_portfolio(&log);

    assert_eq!(
        state_a, state_b,
        "S1: PortfolioState must be identical across two independent replays"
    );

    // Sanity: events were actually applied (portfolio is not empty).
    assert!(
        state_a.cash_micros < 1_000_000 * MICROS_SCALE,
        "S1: fills must have reduced cash below initial value"
    );
    assert!(
        state_a.positions.contains_key("SPY"),
        "S1: SPY position must exist after fill events"
    );
    assert!(
        state_a.positions.contains_key("AAPL"),
        "S1: AAPL position must exist after fill events"
    );
}

// ---------------------------------------------------------------------------
// S2: Audit chain hash and log bytes are identical with injected time.
// ---------------------------------------------------------------------------

/// Two independent `AuditWriter` runs over the same inbox log with the same
/// injected timestamp must produce:
/// - Identical final chain hash.
/// - Byte-identical audit log files.
/// - Valid (unbroken) hash chains in both logs.
#[test]
fn audit_chain_is_deterministic_with_injected_time() {
    let log = make_inbox_log();

    let tmp_a = tempdir().expect("tempdir a");
    let tmp_b = tempdir().expect("tempdir b");

    let hash_a = replay_audit(&log, tmp_a.path());
    let hash_b = replay_audit(&log, tmp_b.path());

    assert_eq!(
        hash_a, hash_b,
        "S2: audit chain final hash must be identical across two replays with injected time"
    );

    // Both chains must be independently valid.
    let result_a =
        verify_hash_chain(tmp_a.path().join("audit.jsonl")).expect("verify_hash_chain a failed");
    let result_b =
        verify_hash_chain(tmp_b.path().join("audit.jsonl")).expect("verify_hash_chain b failed");

    assert!(
        matches!(result_a, VerifyResult::Valid { .. }),
        "S2: audit chain A must be valid, got: {:?}",
        result_a
    );
    assert!(
        matches!(result_b, VerifyResult::Valid { .. }),
        "S2: audit chain B must be valid, got: {:?}",
        result_b
    );

    // Audit log files must be byte-identical (same lines, same hashes, same ts_utc).
    let bytes_a = fs::read(tmp_a.path().join("audit.jsonl")).expect("read audit.jsonl a");
    let bytes_b = fs::read(tmp_b.path().join("audit.jsonl")).expect("read audit.jsonl b");
    assert_eq!(
        bytes_a, bytes_b,
        "S2: audit log bytes must be identical across two replays with fixed injected time"
    );
}

// ---------------------------------------------------------------------------
// S3: Manifest JSON is byte-identical with injected time.
// ---------------------------------------------------------------------------

/// Two independent `init_run_artifacts` calls with the same fixed `now_utc`
/// and identical arguments must produce byte-identical `manifest.json` files.
#[test]
fn manifest_is_deterministic_with_injected_time() {
    let tmp_a = tempdir().expect("tempdir a");
    let tmp_b = tempdir().expect("tempdir b");

    let out_a = init_run_artifacts(InitRunArtifactsArgs {
        exports_root: tmp_a.path(),
        schema_version: 1,
        run_id: run_id(),
        strategy_name: "replay_test_strategy",
        engine_id: "REPLAY_TEST",
        mode: "PAPER",
        git_hash: "i94-test-hash",
        config_hash: "i94-config-hash",
        host_fingerprint: "replay-test-host",
        now_utc: fixed_ts(), // I9-4: same fixed time → byte-identical manifest
    })
    .expect("init_run_artifacts a");

    let out_b = init_run_artifacts(InitRunArtifactsArgs {
        exports_root: tmp_b.path(),
        schema_version: 1,
        run_id: run_id(),
        strategy_name: "replay_test_strategy",
        engine_id: "REPLAY_TEST",
        mode: "PAPER",
        git_hash: "i94-test-hash",
        config_hash: "i94-config-hash",
        host_fingerprint: "replay-test-host",
        now_utc: fixed_ts(), // I9-4: same fixed time → byte-identical manifest
    })
    .expect("init_run_artifacts b");

    let manifest_a = fs::read_to_string(&out_a.manifest_path).expect("read manifest a");
    let manifest_b = fs::read_to_string(&out_b.manifest_path).expect("read manifest b");

    assert_eq!(
        manifest_a, manifest_b,
        "S3: manifest.json must be byte-identical across two runs with fixed injected time"
    );

    // Sanity: manifest contains the expected run_id.
    let v: serde_json::Value = serde_json::from_str(&manifest_a).expect("parse manifest");
    assert_eq!(
        v["run_id"].as_str().unwrap(),
        RUN_ID_STR,
        "S3: manifest run_id must match the fixed run ID"
    );
    assert_eq!(
        v["created_at_utc"].as_str().unwrap(),
        FIXED_TS_STR,
        "S3: manifest created_at_utc must equal the injected fixed timestamp"
    );
}
