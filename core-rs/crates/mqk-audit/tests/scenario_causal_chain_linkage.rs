//! Scenario: Causal Chain Linkage — Section H
//!
//! # Mission
//!
//! Prove that the audit chain can carry and preserve the cross-layer causal
//! identifiers that make the full economic mutation sequence reconstructable
//! from the audit log alone.
//!
//! The DB tables (`oms_outbox`, `oms_inbox`, `broker_order_map`, `runs`,
//! `arm_state`) are the primary durable causal evidence store.  The audit chain
//! (`audit.jsonl`) is the supplementary, tamper-evident record.  Section H
//! requires proof that the audit chain honours its causal linkage contract —
//! that is, every event whose payload includes a causal identifier (`intent_id`,
//! `outbox_id`, `broker_message_id`, `internal_order_id`, `fill_qty`,
//! `halt_reason`) preserves those identifiers correctly through append, hash,
//! persist, and replay.
//!
//! # Invariants under test
//!
//! ## H1 + H4 — Causal identifiers survive the hash chain
//!
//! A representative six-step causal sequence is written:
//!
//! ```text
//!   INTENT_CREATED     {intent_id}
//!   OUTBOX_INSERTED    {outbox_id, intent_id}
//!   BROKER_EVENT       {broker_message_id, internal_order_id}
//!   OMS_TRANSITION     {broker_message_id, internal_order_id, new_state}
//!   PORTFOLIO_FILL     {broker_message_id, internal_order_id, symbol, fill_qty}
//!   RECONCILE_RESULT   {result}
//! ```
//!
//! After writing, every line is deserialized and the causal identifiers are
//! asserted to be present and correct.  The hash chain is valid throughout.
//!
//! ## H2 — Determinism with injected time
//!
//! The same six-step sequence written twice with `append_at` and an identical
//! fixed timestamp must produce byte-identical audit logs and identical final
//! chain hashes.
//!
//! ## H3 — Economic payload tamper detection
//!
//! Mutating `fill_qty` in the `PORTFOLIO_FILL` event breaks the hash chain at
//! that event (`hash_self` mismatch) and is detected by `verify_hash_chain_str`.
//!
//! ## H5 — Restart preserves causal payload linkage
//!
//! A chain interrupted after three events (INTENT → OUTBOX → BROKER_EVENT) is
//! resumed after a simulated restart (`set_last_hash` + `set_seq`).  Three more
//! events (OMS_TRANSITION → PORTFOLIO_FILL → HALT_DECISION) are appended.  The
//! combined six-event chain is valid, and each event's payload causal
//! identifiers — from both before and after the restart — are intact.
//!
//! ## H6 — Halt audit event is tamper-evident
//!
//! A `HALT_DECISION` event with payload `{halt_reason: "IntegrityViolation"}`
//! is written to the hash chain.  Mutating the `halt_reason` field in the
//! serialized JSONL breaks the chain (`hash_self` mismatch).
//!
//! All tests are pure in-process; no DB or network required.

use chrono::DateTime;
use mqk_audit::{
    verify_hash_chain, verify_hash_chain_str, AuditWriter, DurabilityPolicy, VerifyResult,
};
use serde_json::{json, Value};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Deterministic test fixtures
// ---------------------------------------------------------------------------

/// Fixed run ID — never RNG.
const RUN_ID_STR: &str = "c0de0001-0000-0000-0000-000000000000";

/// Fixed timestamp — injected wherever `now_utc` is required.
const FIXED_TS_STR: &str = "2025-03-01T09:30:00Z";

fn run_id() -> Uuid {
    RUN_ID_STR.parse().expect("run_id: parse")
}

fn fixed_ts() -> DateTime<chrono::Utc> {
    FIXED_TS_STR.parse().expect("fixed_ts: parse")
}

fn temp_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "mqk_h_causal_{}_{}_{}.jsonl",
        tag,
        std::process::id(),
        Uuid::new_v4().as_simple()
    ))
}

// ---------------------------------------------------------------------------
// Canonical causal-chain payloads
// ---------------------------------------------------------------------------

/// Fixed causal identifiers shared across all payloads in the chain.
const INTENT_ID: &str = "intent-h-001";
const OUTBOX_ID: &str = "outbox-h-001";
const BROKER_MSG_ID: &str = "broker-msg-h-001";
const INTERNAL_ORDER_ID: &str = "ord-h-001";
const SYMBOL: &str = "SPY";
const FILL_QTY: i64 = 10;
const HALT_REASON: &str = "IntegrityViolation";

/// Ordered payloads for the six-step causal chain.
/// Each tuple is (topic, event_type, payload).
fn causal_chain_payloads() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "execution",
            "INTENT_CREATED",
            json!({ "intent_id": INTENT_ID, "symbol": SYMBOL, "qty": FILL_QTY }),
        ),
        (
            "execution",
            "OUTBOX_INSERTED",
            json!({ "outbox_id": OUTBOX_ID, "intent_id": INTENT_ID }),
        ),
        (
            "broker",
            "BROKER_EVENT",
            json!({
                "broker_message_id": BROKER_MSG_ID,
                "internal_order_id": INTERNAL_ORDER_ID,
                "event_kind": "Fill"
            }),
        ),
        (
            "oms",
            "OMS_TRANSITION",
            json!({
                "broker_message_id": BROKER_MSG_ID,
                "internal_order_id": INTERNAL_ORDER_ID,
                "new_state": "Filled"
            }),
        ),
        (
            "portfolio",
            "PORTFOLIO_FILL",
            json!({
                "broker_message_id": BROKER_MSG_ID,
                "internal_order_id": INTERNAL_ORDER_ID,
                "symbol": SYMBOL,
                "fill_qty": FILL_QTY
            }),
        ),
        (
            "reconcile",
            "RECONCILE_RESULT",
            json!({ "result": "CLEAN", "checked_orders": 1 }),
        ),
    ]
}

/// Write `payloads` into a new `AuditWriter` at `path` using the fixed
/// timestamp.  Returns the final chain hash.
fn write_causal_chain(path: &std::path::Path) -> String {
    let mut writer = AuditWriter::with_durability(path, true, DurabilityPolicy::permissive())
        .expect("AuditWriter::with_durability");
    for (topic, event_type, payload) in causal_chain_payloads() {
        writer
            .append_at(run_id(), topic, event_type, payload, fixed_ts())
            .expect("append_at");
    }
    writer.last_hash().expect("chain hash must be Some")
}

// ---------------------------------------------------------------------------
// H1 + H4 — Causal identifiers survive in the hash chain
// ---------------------------------------------------------------------------

/// H1 + H4: A six-step causal sequence written to the audit chain can be
/// replayed and each event's payload causal identifiers are intact and correct.
///
/// Proves the audit infrastructure can carry the identifiers that allow an
/// operator to follow intent → outbox → broker_event → oms_transition →
/// portfolio_fill → reconcile_result.
#[test]
fn causal_identifiers_survive_in_hash_chain() {
    let path = temp_path("h1h4");
    write_causal_chain(&path);

    // Chain must be valid.
    let result = verify_hash_chain(&path).unwrap();
    assert_eq!(
        result,
        VerifyResult::Valid { lines: 6 },
        "causal chain must be Valid with 6 lines: {:?}",
        result
    );

    // Deserialize each event and assert causal identifiers are present.
    let content = std::fs::read_to_string(&path).unwrap();
    let events: Vec<Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("parse audit event"))
        .collect();

    assert_eq!(events.len(), 6, "must have exactly 6 events");

    // Event 0: INTENT_CREATED — must carry intent_id.
    assert_eq!(
        events[0]["event_type"].as_str().unwrap(),
        "INTENT_CREATED",
        "event 0 must be INTENT_CREATED"
    );
    assert_eq!(
        events[0]["payload"]["intent_id"].as_str().unwrap(),
        INTENT_ID,
        "INTENT_CREATED must carry intent_id"
    );

    // Event 1: OUTBOX_INSERTED — must carry outbox_id AND intent_id (causal link).
    assert_eq!(events[1]["event_type"].as_str().unwrap(), "OUTBOX_INSERTED");
    assert_eq!(
        events[1]["payload"]["outbox_id"].as_str().unwrap(),
        OUTBOX_ID,
        "OUTBOX_INSERTED must carry outbox_id"
    );
    assert_eq!(
        events[1]["payload"]["intent_id"].as_str().unwrap(),
        INTENT_ID,
        "OUTBOX_INSERTED must carry intent_id (causal link to INTENT_CREATED)"
    );

    // Event 2: BROKER_EVENT — must carry broker_message_id and internal_order_id.
    assert_eq!(events[2]["event_type"].as_str().unwrap(), "BROKER_EVENT");
    assert_eq!(
        events[2]["payload"]["broker_message_id"].as_str().unwrap(),
        BROKER_MSG_ID,
        "BROKER_EVENT must carry broker_message_id"
    );
    assert_eq!(
        events[2]["payload"]["internal_order_id"].as_str().unwrap(),
        INTERNAL_ORDER_ID,
        "BROKER_EVENT must carry internal_order_id"
    );

    // Event 3: OMS_TRANSITION — must carry broker_message_id (causal link from broker event).
    assert_eq!(events[3]["event_type"].as_str().unwrap(), "OMS_TRANSITION");
    assert_eq!(
        events[3]["payload"]["broker_message_id"].as_str().unwrap(),
        BROKER_MSG_ID,
        "OMS_TRANSITION must carry broker_message_id (causal link to BROKER_EVENT)"
    );
    assert_eq!(
        events[3]["payload"]["new_state"].as_str().unwrap(),
        "Filled",
        "OMS_TRANSITION must carry new_state"
    );

    // Event 4: PORTFOLIO_FILL — must carry broker_message_id, symbol, fill_qty.
    assert_eq!(events[4]["event_type"].as_str().unwrap(), "PORTFOLIO_FILL");
    assert_eq!(
        events[4]["payload"]["broker_message_id"].as_str().unwrap(),
        BROKER_MSG_ID,
        "PORTFOLIO_FILL must carry broker_message_id (causal link to BROKER_EVENT)"
    );
    assert_eq!(
        events[4]["payload"]["symbol"].as_str().unwrap(),
        SYMBOL,
        "PORTFOLIO_FILL must carry symbol"
    );
    assert_eq!(
        events[4]["payload"]["fill_qty"].as_i64().unwrap(),
        FILL_QTY,
        "PORTFOLIO_FILL must carry fill_qty"
    );

    // Event 5: RECONCILE_RESULT — must carry result.
    assert_eq!(
        events[5]["event_type"].as_str().unwrap(),
        "RECONCILE_RESULT"
    );
    assert_eq!(
        events[5]["payload"]["result"].as_str().unwrap(),
        "CLEAN",
        "RECONCILE_RESULT must carry result"
    );

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// H2 — Causal chain is deterministic with injected time
// ---------------------------------------------------------------------------

/// H2: Two independent writes of the same six-step causal sequence with the
/// same injected timestamp produce byte-identical audit logs and identical
/// final chain hashes.
///
/// Proves the audit chain is a deterministic function of the causal sequence —
/// two replays produce identical evidence and an operator can verify the chain
/// is unchanged.
#[test]
fn causal_chain_is_deterministic_with_injected_time() {
    let path_a = temp_path("h2a");
    let path_b = temp_path("h2b");

    let hash_a = write_causal_chain(&path_a);
    let hash_b = write_causal_chain(&path_b);

    assert_eq!(
        hash_a, hash_b,
        "H2: same causal sequence with fixed timestamp must produce identical chain hashes"
    );

    // Both chains must be independently valid.
    let r_a = verify_hash_chain(&path_a).unwrap();
    let r_b = verify_hash_chain(&path_b).unwrap();
    assert!(
        matches!(r_a, VerifyResult::Valid { lines: 6 }),
        "H2: chain A must be Valid(6): {:?}",
        r_a
    );
    assert!(
        matches!(r_b, VerifyResult::Valid { lines: 6 }),
        "H2: chain B must be Valid(6): {:?}",
        r_b
    );

    // Audit log files must be byte-identical.
    let bytes_a = std::fs::read(&path_a).unwrap();
    let bytes_b = std::fs::read(&path_b).unwrap();
    assert_eq!(
        bytes_a, bytes_b,
        "H2: audit log bytes must be identical across two replays with fixed injected time"
    );

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

// ---------------------------------------------------------------------------
// H3 — Economic payload tamper detection
// ---------------------------------------------------------------------------

/// H3: Mutating `fill_qty` in the `PORTFOLIO_FILL` audit event (the most
/// economically sensitive field) breaks the hash chain at that event.
///
/// An attacker attempting to alter the recorded fill quantity to cover their
/// tracks cannot produce a valid chain — the mismatch is detected by
/// `verify_hash_chain_str`.
#[test]
fn economic_payload_fill_qty_tamper_is_detected() {
    let path = temp_path("h3");
    write_causal_chain(&path);

    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<String> = content.lines().map(str::to_owned).collect();

    // Find the PORTFOLIO_FILL line (index 4) and mutate fill_qty.
    let tampered_lines: Vec<String> = lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 4 {
                // Parse, mutate fill_qty, re-serialize WITHOUT updating hashes.
                let mut ev: Value = serde_json::from_str(&line).expect("parse line 4");
                let original_qty = ev["payload"]["fill_qty"].as_i64().unwrap();
                ev["payload"]["fill_qty"] = json!(original_qty + 999);
                serde_json::to_string(&ev).expect("re-serialize")
            } else {
                line
            }
        })
        .collect();

    let tampered = tampered_lines.join("\n") + "\n";

    let result = verify_hash_chain_str(&tampered).unwrap();
    assert!(
        matches!(result, VerifyResult::Broken { line: 5, .. }),
        "H3: mutated fill_qty must break chain at line 5 (PORTFOLIO_FILL event): {:?}",
        result
    );

    // Confirm it's a hash_self mismatch (not hash_prev — the event itself is corrupt).
    if let VerifyResult::Broken { reason, .. } = result {
        assert!(
            reason.contains("hash_self"),
            "H3: break reason must mention hash_self mismatch; got: {reason}"
        );
    }

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// H6 — Halt audit event is tamper-evident
// ---------------------------------------------------------------------------

/// H6: A HALT_DECISION audit event with `halt_reason` in its payload is
/// tamper-evident — mutating the reason field breaks the hash chain.
///
/// This proves that the durable audit record of why the system was halted
/// cannot be silently altered: any post-hoc modification of the halt reason
/// is detected by chain verification.
#[test]
fn halt_reason_in_audit_chain_is_tamper_evident() {
    let path = temp_path("h6");
    {
        let mut writer = AuditWriter::with_durability(&path, true, DurabilityPolicy::permissive())
            .expect("AuditWriter");

        // Pre-halt causal context.
        writer
            .append_at(
                run_id(),
                "execution",
                "INTENT_CREATED",
                json!({ "intent_id": INTENT_ID }),
                fixed_ts(),
            )
            .expect("append intent");

        writer
            .append_at(
                run_id(),
                "portfolio",
                "PORTFOLIO_FILL",
                json!({
                    "broker_message_id": BROKER_MSG_ID,
                    "symbol": SYMBOL,
                    "fill_qty": FILL_QTY
                }),
                fixed_ts(),
            )
            .expect("append fill");

        // Halt event — this is the H6 subject.
        writer
            .append_at(
                run_id(),
                "system",
                "HALT_DECISION",
                json!({
                    "halt_reason": HALT_REASON,
                    "run_id": RUN_ID_STR,
                    "disarmed": true
                }),
                fixed_ts(),
            )
            .expect("append halt");
    }

    // Verify untampered chain.
    let result = verify_hash_chain(&path).unwrap();
    assert_eq!(
        result,
        VerifyResult::Valid { lines: 3 },
        "untampered halt chain must be Valid(3): {:?}",
        result
    );

    // Read the halt event (line index 2) and mutate halt_reason.
    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<String> = content.lines().map(str::to_owned).collect();

    let tampered_lines: Vec<String> = lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 2 {
                let mut ev: Value = serde_json::from_str(&line).expect("parse halt line");
                // Attacker tries to erase the reason.
                ev["payload"]["halt_reason"] = json!("Operational_Maintenance");
                serde_json::to_string(&ev).expect("re-serialize")
            } else {
                line
            }
        })
        .collect();

    let tampered = tampered_lines.join("\n") + "\n";

    let broken = verify_hash_chain_str(&tampered).unwrap();
    assert!(
        matches!(broken, VerifyResult::Broken { line: 3, .. }),
        "H6: mutated halt_reason must break chain at line 3: {:?}",
        broken
    );
    if let VerifyResult::Broken { reason, .. } = broken {
        assert!(
            reason.contains("hash_self"),
            "H6: break must be hash_self mismatch; got: {reason}"
        );
    }

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// H5 — Restart preserves causal payload linkage
// ---------------------------------------------------------------------------

/// H5: A chain interrupted after three events is resumed after a simulated
/// restart (`set_last_hash` + `set_seq`).  Three more events are appended.
/// The combined six-event chain is valid, AND every event's payload causal
/// identifiers — written both before and after the restart boundary — are
/// intact.
///
/// Proves that daemon restart does not sever causal audit linkage for events
/// already persisted before the crash.
#[test]
fn restart_preserves_causal_payload_linkage() {
    let path = temp_path("h5");

    // Phase 1: write first three events; save restart checkpoint.
    let (last_hash_at_3, seq_at_3) = {
        let mut writer = AuditWriter::with_durability(&path, true, DurabilityPolicy::permissive())
            .expect("AuditWriter phase 1");

        let events_pre = [
            (
                "execution",
                "INTENT_CREATED",
                json!({ "intent_id": INTENT_ID }),
            ),
            (
                "execution",
                "OUTBOX_INSERTED",
                json!({ "outbox_id": OUTBOX_ID, "intent_id": INTENT_ID }),
            ),
            (
                "broker",
                "BROKER_EVENT",
                json!({
                    "broker_message_id": BROKER_MSG_ID,
                    "internal_order_id": INTERNAL_ORDER_ID
                }),
            ),
        ];
        for (topic, event_type, payload) in events_pre {
            writer
                .append_at(run_id(), topic, event_type, payload, fixed_ts())
                .expect("append phase 1");
        }
        (writer.last_hash(), writer.seq())
    };

    assert_eq!(seq_at_3, 3, "seq must be 3 after phase-1 events");
    assert!(
        last_hash_at_3.is_some(),
        "last_hash must be Some after phase 1"
    );

    // Phase 2: simulate restart — new writer, restore checkpoint, write 3 more.
    {
        let mut writer = AuditWriter::with_durability(&path, true, DurabilityPolicy::permissive())
            .expect("AuditWriter phase 2");
        writer.set_last_hash(last_hash_at_3);
        writer.set_seq(seq_at_3);

        let events_post = [
            (
                "oms",
                "OMS_TRANSITION",
                json!({
                    "broker_message_id": BROKER_MSG_ID,
                    "internal_order_id": INTERNAL_ORDER_ID,
                    "new_state": "Filled"
                }),
            ),
            (
                "portfolio",
                "PORTFOLIO_FILL",
                json!({
                    "broker_message_id": BROKER_MSG_ID,
                    "symbol": SYMBOL,
                    "fill_qty": FILL_QTY
                }),
            ),
            (
                "system",
                "HALT_DECISION",
                json!({
                    "halt_reason": HALT_REASON,
                    "disarmed": true
                }),
            ),
        ];
        for (topic, event_type, payload) in events_post {
            writer
                .append_at(run_id(), topic, event_type, payload, fixed_ts())
                .expect("append phase 2");
        }
    }

    // The combined 6-event chain must be valid.
    let result = verify_hash_chain(&path).unwrap();
    assert_eq!(
        result,
        VerifyResult::Valid { lines: 6 },
        "H5: resumed 6-event chain must be Valid(6): {:?}",
        result
    );

    // Deserialize all events and verify causal identifiers from BOTH phases.
    let content = std::fs::read_to_string(&path).unwrap();
    let events: Vec<Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("parse"))
        .collect();

    assert_eq!(events.len(), 6, "must have 6 events after restart resume");

    // Pre-restart events: intent_id preserved.
    assert_eq!(
        events[0]["payload"]["intent_id"].as_str().unwrap(),
        INTENT_ID,
        "H5: INTENT_CREATED intent_id must be intact after restart"
    );
    // Phase-1 outbox event: both outbox_id and intent_id preserved.
    assert_eq!(
        events[1]["payload"]["outbox_id"].as_str().unwrap(),
        OUTBOX_ID,
        "H5: OUTBOX_INSERTED outbox_id must be intact after restart"
    );
    assert_eq!(
        events[1]["payload"]["intent_id"].as_str().unwrap(),
        INTENT_ID,
        "H5: OUTBOX_INSERTED intent_id must be intact after restart"
    );
    // Phase-1 broker event: broker_message_id preserved.
    assert_eq!(
        events[2]["payload"]["broker_message_id"].as_str().unwrap(),
        BROKER_MSG_ID,
        "H5: BROKER_EVENT broker_message_id must be intact after restart"
    );

    // Post-restart events: causal identifiers match their pre-restart counterparts.
    assert_eq!(
        events[3]["payload"]["broker_message_id"].as_str().unwrap(),
        BROKER_MSG_ID,
        "H5: OMS_TRANSITION broker_message_id must match BROKER_EVENT (post-restart)"
    );
    assert_eq!(
        events[4]["payload"]["broker_message_id"].as_str().unwrap(),
        BROKER_MSG_ID,
        "H5: PORTFOLIO_FILL broker_message_id must match BROKER_EVENT (post-restart)"
    );
    assert_eq!(
        events[4]["payload"]["fill_qty"].as_i64().unwrap(),
        FILL_QTY,
        "H5: PORTFOLIO_FILL fill_qty must be intact (post-restart)"
    );
    assert_eq!(
        events[5]["payload"]["halt_reason"].as_str().unwrap(),
        HALT_REASON,
        "H5: HALT_DECISION halt_reason must be intact (post-restart)"
    );

    let _ = std::fs::remove_file(&path);
}
