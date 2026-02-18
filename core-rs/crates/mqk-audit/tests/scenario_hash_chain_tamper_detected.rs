//! PATCH 15c — Audit hash chain integrity test
//!
//! Validates: docs/specs/run_artifacts_and_reproducibility.md (audit hash chain)
//!
//! GREEN when:
//! - Writing 5 events with hash_chain=true, then verifying, succeeds.
//! - Mutating line 3's payload in the file, then verifying, detects the break.
//! - An untampered log verifies cleanly with correct line count.

use mqk_audit::{AuditWriter, VerifyResult, verify_hash_chain};
use serde_json::json;
use uuid::Uuid;

fn temp_audit_path(suffix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "mqk_audit_test_{}_{}_{}",
        suffix,
        std::process::id(),
        Uuid::new_v4().as_simple()
    ))
}

#[test]
fn untampered_chain_verifies_valid() {
    let path = temp_audit_path("untampered");
    let run_id = Uuid::new_v4();

    // Write 5 events with hash chain enabled
    {
        let mut writer = AuditWriter::new(&path, true).unwrap();
        for i in 0..5 {
            writer
                .append(
                    run_id,
                    "AUDIT",
                    &format!("TEST_EVENT_{i}"),
                    json!({"index": i, "data": format!("payload_{i}")}),
                )
                .unwrap();
        }
    }

    // Verify the chain is intact
    let result = verify_hash_chain(&path).unwrap();
    assert_eq!(
        result,
        VerifyResult::Valid { lines: 5 },
        "untampered chain should verify as valid with 5 lines"
    );

    // Cleanup
    let _ = std::fs::remove_file(&path);
}

#[test]
fn tampered_payload_detected() {
    let path = temp_audit_path("tampered");
    let run_id = Uuid::new_v4();

    // Write 5 events with hash chain enabled
    {
        let mut writer = AuditWriter::new(&path, true).unwrap();
        for i in 0..5 {
            writer
                .append(
                    run_id,
                    "AUDIT",
                    &format!("TEST_EVENT_{i}"),
                    json!({"index": i, "data": format!("payload_{i}")}),
                )
                .unwrap();
        }
    }

    // Tamper with line 3 (0-indexed line 2): modify the payload
    {
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<&str> = content.lines().collect();
        assert!(lines.len() >= 5, "should have 5 lines");

        // Parse line 3, modify payload, write back
        let mut ev: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        ev["payload"]["data"] = json!("TAMPERED_VALUE");
        let tampered_line = serde_json::to_string(&ev).unwrap();

        // Replace line 3 with tampered version
        lines[2] = &tampered_line;
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    }

    // Verify should detect the tamper
    let result = verify_hash_chain(&path).unwrap();
    match result {
        VerifyResult::Broken { line, reason } => {
            // The break should be detected at line 3 (hash_self mismatch)
            // because we changed the payload but didn't recompute hash_self.
            assert_eq!(
                line, 3,
                "tamper should be detected at line 3, got line {line}: {reason}"
            );
            assert!(
                reason.contains("hash_self mismatch"),
                "reason should mention hash_self mismatch, got: {reason}"
            );
        }
        VerifyResult::Valid { lines } => {
            panic!("tampered chain should NOT verify as valid (got {lines} valid lines)");
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(&path);
}

#[test]
fn deleted_line_detected() {
    let path = temp_audit_path("deleted");
    let run_id = Uuid::new_v4();

    // Write 5 events
    {
        let mut writer = AuditWriter::new(&path, true).unwrap();
        for i in 0..5 {
            writer
                .append(
                    run_id,
                    "AUDIT",
                    &format!("TEST_EVENT_{i}"),
                    json!({"index": i}),
                )
                .unwrap();
        }
    }

    // Delete line 3 (0-indexed line 2)
    {
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        let mut new_lines = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if i != 2 {
                new_lines.push(*line);
            }
        }
        std::fs::write(&path, new_lines.join("\n") + "\n").unwrap();
    }

    // Verify should detect the deleted line via hash_prev chain break
    let result = verify_hash_chain(&path).unwrap();
    match result {
        VerifyResult::Broken { line, reason } => {
            // Line 4 (originally line 4, now at position 3) should have hash_prev
            // pointing to line 3's hash, but line 3 was deleted so the chain breaks.
            assert!(
                reason.contains("hash_prev mismatch"),
                "reason should mention hash_prev mismatch, got: {reason}"
            );
            assert!(
                line >= 3,
                "break should be at line 3 or later (was at {line})"
            );
        }
        VerifyResult::Valid { lines } => {
            panic!("chain with deleted line should NOT verify as valid (got {lines} lines)");
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(&path);
}

#[test]
fn empty_log_is_valid() {
    let path = temp_audit_path("empty");
    std::fs::write(&path, "").unwrap();

    let result = verify_hash_chain(&path).unwrap();
    assert_eq!(
        result,
        VerifyResult::Valid { lines: 0 },
        "empty log should verify as valid with 0 lines"
    );

    // Cleanup
    let _ = std::fs::remove_file(&path);
}

#[test]
fn single_event_verifies() {
    let path = temp_audit_path("single");
    let run_id = Uuid::new_v4();

    {
        let mut writer = AuditWriter::new(&path, true).unwrap();
        writer
            .append(run_id, "AUDIT", "SINGLE", json!({"ok": true}))
            .unwrap();
    }

    let result = verify_hash_chain(&path).unwrap();
    assert_eq!(
        result,
        VerifyResult::Valid { lines: 1 },
        "single-event chain should verify as valid"
    );

    // Cleanup
    let _ = std::fs::remove_file(&path);
}
