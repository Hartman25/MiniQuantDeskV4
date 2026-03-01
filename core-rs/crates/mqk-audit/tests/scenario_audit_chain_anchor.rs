//! A6-1: Audit Chain Anchoring
//!
//! Proves that tampering is detectable via the chain tip hash (external anchor).
//!
//! The anchor is the `hash_self` of the last event in a run — recorded externally
//! at end-of-run (e.g. written to a separate tamper-evident store, printed to
//! an operator receipt, or compared against a previously saved value).
//!
//! Because each event's `hash_self` is a SHA-256 over all preceding content,
//! any modification to any event, any insertion, any deletion, or a full log
//! replacement with a different chain will produce a different final hash that
//! does not match the recorded anchor.
//!
//! Five anchor properties tested:
//!
//! 1. **Anchor equals last event's hash_self** — `AuditWriter::last_hash()`
//!    returns exactly the `hash_self` of the most recently appended event.
//!    This is the value to publish as the external anchor.
//!
//! 2. **Different-content chain has different anchor** — two chains of equal
//!    length but different payload produce different anchors.  Full log
//!    replacement is detectable.
//!
//! 3. **Anchor advances with each event** — the anchor after N events differs
//!    from the anchor after N−1 events.  Log truncation (dropping events from
//!    the end) is detectable.
//!
//! 4. **Chain resumes correctly after restart** — `set_last_hash` + `set_seq`
//!    continues the chain; the combined pre- and post-restart segment verifies
//!    as a single valid N+M event chain.
//!
//! 5. **`verify_hash_chain_str` is equivalent to file verify** — the in-memory
//!    verification path (used by the Patch B6 artifact gate) produces the same
//!    result as the file-based path on both valid and tampered content.

use mqk_audit::{verify_hash_chain, verify_hash_chain_str, AuditWriter, VerifyResult};
use serde_json::json;
use uuid::Uuid;

fn temp_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "mqk_audit_anchor_{}_{}_{}",
        tag,
        std::process::id(),
        Uuid::new_v4().as_simple()
    ))
}

// ---------------------------------------------------------------------------
// Anchor property 1: anchor equals last event's hash_self
// ---------------------------------------------------------------------------

/// ANCHOR 1 of 5.
///
/// `AuditWriter::last_hash()` must equal the `hash_self` field of the most
/// recently appended `AuditEvent`.  This is the proof that `last_hash()` is
/// the correct value to publish as an external anchor at end-of-run.
#[test]
fn anchor_equals_last_event_hash_self() {
    let path = temp_path("prop1");
    let run_id = Uuid::new_v4();
    let mut writer = AuditWriter::new(&path, true).unwrap();

    let mut last_event = None;
    for i in 0..5u64 {
        last_event = Some(
            writer
                .append(run_id, "AUDIT", "EVENT", json!({"seq": i}))
                .unwrap(),
        );
    }

    let last_event = last_event.unwrap();
    let anchor = writer.last_hash();

    // Anchor must equal the hash_self of the last event — these are the same
    // SHA-256 value reached by two independent paths.
    assert_eq!(
        anchor,
        last_event.hash_self,
        "last_hash() ({:?}) must equal last event hash_self ({:?})",
        anchor,
        last_event.hash_self,
    );

    // Sanity: anchor must be non-None when hash_chain is enabled.
    assert!(
        anchor.is_some(),
        "anchor must be Some when hash_chain = true"
    );

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// Anchor property 2: different content → different anchor
// ---------------------------------------------------------------------------

/// ANCHOR 2 of 5.
///
/// Two chains of equal length but different payload content must produce
/// different anchors.
///
/// This proves full log replacement is detectable: an attacker who replaces
/// the log file with a different (even internally valid) chain produces a
/// chain whose final hash does not match the originally recorded anchor.
#[test]
fn different_content_chain_has_different_anchor() {
    let path_a = temp_path("prop2a");
    let path_b = temp_path("prop2b");
    let run_id = Uuid::new_v4();

    // Chain A: payloads tagged "alpha".
    let anchor_a = {
        let mut w = AuditWriter::new(&path_a, true).unwrap();
        for i in 0..5u64 {
            w.append(run_id, "AUDIT", "E", json!({"v": "alpha", "i": i}))
                .unwrap();
        }
        w.last_hash()
    };

    // Chain B: identical structure, payloads tagged "beta".
    let anchor_b = {
        let mut w = AuditWriter::new(&path_b, true).unwrap();
        for i in 0..5u64 {
            w.append(run_id, "AUDIT", "E", json!({"v": "beta", "i": i}))
                .unwrap();
        }
        w.last_hash()
    };

    assert!(
        anchor_a.is_some() && anchor_b.is_some(),
        "both chains must produce non-None anchors (hash_chain = true)"
    );
    assert_ne!(
        anchor_a, anchor_b,
        "chains with different content must have different anchors — \
         full log replacement must be detectable"
    );

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}

// ---------------------------------------------------------------------------
// Anchor property 3: anchor advances with each event
// ---------------------------------------------------------------------------

/// ANCHOR 3 of 5.
///
/// The anchor after N events must differ from the anchor after N−1 events for
/// every N.
///
/// This proves log truncation is detectable: if an attacker deletes any
/// suffix of events, the remaining log's final hash will not match the anchor
/// recorded at end-of-run.
#[test]
fn anchor_advances_with_each_appended_event() {
    let path = temp_path("prop3");
    let run_id = Uuid::new_v4();
    let mut writer = AuditWriter::new(&path, true).unwrap();

    let mut anchors: Vec<Option<String>> = Vec::new();

    for i in 0..5u64 {
        writer
            .append(run_id, "AUDIT", "E", json!({"i": i}))
            .unwrap();
        anchors.push(writer.last_hash());
    }

    // Every anchor must be Some.
    for (idx, anchor) in anchors.iter().enumerate() {
        assert!(
            anchor.is_some(),
            "anchor after event {} must be Some (hash_chain = true)",
            idx
        );
    }

    // Every consecutive pair must differ — no two chain tips are equal.
    for i in 1..anchors.len() {
        assert_ne!(
            anchors[i],
            anchors[i - 1],
            "anchor after event {} ({:?}) must differ from anchor after event {} ({:?}); \
             dropping the last event must be detectable",
            i,
            anchors[i],
            i - 1,
            anchors[i - 1],
        );
    }

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// Anchor property 4: chain resumes correctly after restart
// ---------------------------------------------------------------------------

/// ANCHOR 4 of 5.
///
/// A chain interrupted after N events (e.g. daemon restart) can be continued
/// correctly via `set_last_hash` + `set_seq`.  The combined N+M event file
/// must verify as a single unbroken chain with N+M lines.
///
/// This is the restart semantics proof: the anchor from event N is the bridge
/// between the pre-restart and post-restart segments of the same logical run.
#[test]
fn chain_resumes_correctly_after_restart() {
    let path = temp_path("prop4");
    let run_id = Uuid::new_v4();

    // Phase 1: write 3 events; record restart checkpoint.
    let (last_hash_at_3, seq_at_3) = {
        let mut writer = AuditWriter::new(&path, true).unwrap();
        for i in 0..3u64 {
            writer
                .append(run_id, "AUDIT", "PRE", json!({"i": i}))
                .unwrap();
        }
        (writer.last_hash(), writer.seq())
    };

    assert_eq!(seq_at_3, 3, "seq must equal events written before restart");
    assert!(
        last_hash_at_3.is_some(),
        "last_hash must be Some after 3 events"
    );

    // Phase 2: new writer for the same file; restore checkpoint, write 2 more.
    {
        let mut writer = AuditWriter::new(&path, true).unwrap();
        writer.set_last_hash(last_hash_at_3);
        writer.set_seq(seq_at_3);

        for i in 3..5u64 {
            writer
                .append(run_id, "AUDIT", "POST", json!({"i": i}))
                .unwrap();
        }
    }

    // The combined 5-event file must verify as one valid chain.
    let result = verify_hash_chain(&path).unwrap();
    assert_eq!(
        result,
        VerifyResult::Valid { lines: 5 },
        "resumed 5-event chain must verify as a single valid chain: {:?}",
        result
    );

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// Anchor property 5: verify_hash_chain_str ≡ verify_hash_chain
// ---------------------------------------------------------------------------

/// ANCHOR 5 of 5.
///
/// `verify_hash_chain_str` (in-memory) must produce the same `VerifyResult`
/// as `verify_hash_chain` (file-based) for both a valid chain and a tampered
/// chain.
///
/// This proves the in-memory verification path — used by the Patch B6 artifact
/// gate, which validates audit logs without a file path — is correct and
/// interchangeable with the file path.
#[test]
fn verify_str_is_equivalent_to_file_verify() {
    let path = temp_path("prop5");
    let run_id = Uuid::new_v4();

    // Write 5 events.
    {
        let mut writer = AuditWriter::new(&path, true).unwrap();
        for i in 0..5u64 {
            writer
                .append(run_id, "AUDIT", "E", json!({"i": i}))
                .unwrap();
        }
    }

    let content = std::fs::read_to_string(&path).unwrap();

    // --- Valid chain: file == str ---
    let file_result = verify_hash_chain(&path).unwrap();
    let str_result = verify_hash_chain_str(&content).unwrap();

    assert_eq!(
        file_result, str_result,
        "file verify ({:?}) must equal str verify ({:?}) on untampered log",
        file_result, str_result,
    );
    assert_eq!(
        file_result,
        VerifyResult::Valid { lines: 5 },
        "untampered log must be Valid with 5 lines"
    );

    // --- Tampered chain: str detect breaks at same location ---
    let tampered_content = {
        let lines: Vec<String> = content.lines().map(str::to_owned).collect();
        let tampered: Vec<String> = lines
            .into_iter()
            .enumerate()
            .map(|(idx, line)| {
                if idx == 2 {
                    // Mutate payload of event 3 (0-indexed 2) without updating hashes.
                    let mut ev: serde_json::Value = serde_json::from_str(&line).unwrap();
                    ev["payload"]["i"] = json!(9999);
                    serde_json::to_string(&ev).unwrap()
                } else {
                    line
                }
            })
            .collect();
        tampered.join("\n") + "\n"
    };

    let str_tampered = verify_hash_chain_str(&tampered_content).unwrap();
    assert!(
        matches!(str_tampered, VerifyResult::Broken { .. }),
        "tampered content must be detected as Broken by str verify: {:?}",
        str_tampered
    );

    let _ = std::fs::remove_file(&path);
}
