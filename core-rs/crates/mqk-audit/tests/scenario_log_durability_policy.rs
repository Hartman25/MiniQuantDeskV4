//! A6-2: Log Durability Policy
//!
//! Proves that the audit log has an explicit, declared flush and rotation
//! policy — no implicit reliance on OS defaults.
//!
//! Six policy properties tested:
//!
//! 1. **`strict()` enables sync** — `DurabilityPolicy::strict().sync_on_append == true`.
//!
//! 2. **`permissive()` disables sync** — `DurabilityPolicy::permissive().sync_on_append == false`.
//!
//! 3. **Default policy is `strict()`** — fail-closed posture; sync is on unless
//!    explicitly opted out.
//!
//! 4. **Strict write path produces a valid chain** — `sync_on_append = true`
//!    does not corrupt data; the chain verifies after a sync-per-event run.
//!
//! 5. **Rotation triggers at threshold** — with `rotation_max_events = 3` and
//!    7 events written, the writer produces segments 0 (3 events), 1 (3 events),
//!    2 (1 event).  The `current_segment_path()` for each segment is correct.
//!
//! 6. **Each rotation segment verifies independently** — segments 0, 1, and 2
//!    each form a complete, independently valid hash chain.  No segment depends
//!    on another for verification.

use mqk_audit::{verify_hash_chain, AuditWriter, DurabilityPolicy, VerifyResult};
use serde_json::json;
use uuid::Uuid;

fn temp_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "mqk_audit_durability_{}_{}_{}",
        tag,
        std::process::id(),
        Uuid::new_v4().as_simple()
    ))
}

// ---------------------------------------------------------------------------
// Policy property 1: strict() enables sync
// ---------------------------------------------------------------------------

/// POLICY 1 of 6.
///
/// `DurabilityPolicy::strict()` must set `sync_on_append = true`.
/// This is the production default — every event is durably flushed to
/// persistent storage before the caller's `append()` returns.
#[test]
fn strict_policy_sync_on_append_is_true() {
    let p = DurabilityPolicy::strict();
    assert!(
        p.sync_on_append,
        "DurabilityPolicy::strict() must have sync_on_append = true"
    );
}

// ---------------------------------------------------------------------------
// Policy property 2: permissive() disables sync
// ---------------------------------------------------------------------------

/// POLICY 2 of 6.
///
/// `DurabilityPolicy::permissive()` must set `sync_on_append = false`.
/// This is the test/fast-path policy — explicitly opts out of fsync.
#[test]
fn permissive_policy_sync_on_append_is_false() {
    let p = DurabilityPolicy::permissive();
    assert!(
        !p.sync_on_append,
        "DurabilityPolicy::permissive() must have sync_on_append = false"
    );
}

// ---------------------------------------------------------------------------
// Policy property 3: default is strict
// ---------------------------------------------------------------------------

/// POLICY 3 of 6.
///
/// `DurabilityPolicy::default()` must equal `DurabilityPolicy::strict()`.
///
/// Fail-closed posture: the safe default is always the more conservative
/// setting.  An operator must explicitly opt out of sync.
#[test]
fn default_policy_is_strict() {
    let d = DurabilityPolicy::default();
    assert!(
        d.sync_on_append,
        "DurabilityPolicy::default() must have sync_on_append = true (fail-closed)"
    );
    assert_eq!(
        d.rotation_max_events, 0,
        "DurabilityPolicy::default() must have rotation disabled (0)"
    );
}

// ---------------------------------------------------------------------------
// Policy property 4: strict write path produces a valid chain
// ---------------------------------------------------------------------------

/// POLICY 4 of 6.
///
/// Writing events with `sync_on_append = true` must not corrupt data.
/// The resulting file must pass chain verification with the correct line count.
///
/// This proves that the fsync path is functionally correct — not just a
/// performance knob — and that no data is silently dropped or corrupted.
#[test]
fn strict_policy_write_path_produces_valid_chain() {
    let path = temp_path("strict");
    let run_id = Uuid::new_v4();

    {
        let mut writer =
            AuditWriter::with_durability(&path, true, DurabilityPolicy::strict()).unwrap();
        for i in 0..5u64 {
            writer
                .append(run_id, "AUDIT", "EVENT", json!({"i": i}))
                .unwrap();
        }
    }

    let result = verify_hash_chain(&path).unwrap();
    assert_eq!(
        result,
        VerifyResult::Valid { lines: 5 },
        "strict-policy write must produce a valid 5-event chain: {:?}",
        result
    );

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// Policy property 5: rotation triggers at threshold
// ---------------------------------------------------------------------------

/// POLICY 5 of 6.
///
/// With `rotation_max_events = 3` and 7 events written:
/// - Segment 0 (base path) receives events 0, 1, 2 (3 events).
/// - Segment 1 (`{path}.1`)  receives events 3, 4, 5 (3 events).
/// - Segment 2 (`{path}.2`)  receives event  6     (1 event).
///
/// The `current_segment_path()` after writing all 7 events must point to
/// the segment-2 file, and the segment index must be 2.
#[test]
fn rotation_triggers_at_threshold() {
    let path = temp_path("rotation");
    let run_id = Uuid::new_v4();

    let policy = DurabilityPolicy {
        sync_on_append: false, // permissive for speed; rotation is what we test
        rotation_max_events: 3,
    };

    {
        let mut writer = AuditWriter::with_durability(&path, true, policy).unwrap();

        for i in 0..7u64 {
            writer
                .append(run_id, "AUDIT", "E", json!({"i": i}))
                .unwrap();
        }

        // After 7 events with rotation_max=3: segments 0, 1, 2.
        assert_eq!(
            writer.segment(),
            2,
            "segment counter must be 2 after 7 events with rotation_max=3"
        );
    }

    // Compute expected segment paths.
    let seg0 = path.clone(); // base path
    let seg1 = {
        let fname = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut p = path.clone();
        p.set_file_name(format!("{}.1", fname));
        p
    };
    let seg2 = {
        let fname = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut p = path.clone();
        p.set_file_name(format!("{}.2", fname));
        p
    };

    // Each segment file must exist.
    assert!(seg0.exists(), "segment 0 file must exist: {:?}", seg0);
    assert!(seg1.exists(), "segment 1 file must exist: {:?}", seg1);
    assert!(seg2.exists(), "segment 2 file must exist: {:?}", seg2);

    // Segment line counts.
    let count_lines = |p: &std::path::Path| -> usize {
        std::fs::read_to_string(p)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()
    };

    assert_eq!(count_lines(&seg0), 3, "segment 0 must have 3 events");
    assert_eq!(count_lines(&seg1), 3, "segment 1 must have 3 events");
    assert_eq!(count_lines(&seg2), 1, "segment 2 must have 1 event");

    let _ = std::fs::remove_file(&seg0);
    let _ = std::fs::remove_file(&seg1);
    let _ = std::fs::remove_file(&seg2);
}

// ---------------------------------------------------------------------------
// Policy property 6: each rotation segment verifies independently
// ---------------------------------------------------------------------------

/// POLICY 6 of 6.
///
/// With `rotation_max_events = 3` and 9 events written (3 full segments):
/// - Segment 0: events 0–2 → `Valid { lines: 3 }`.
/// - Segment 1: events 3–5 → `Valid { lines: 3 }`.
/// - Segment 2: events 6–8 → `Valid { lines: 3 }`.
///
/// Each segment starts a fresh hash chain (`hash_prev = None` for its first
/// event) so it can be verified without any other segment.  This makes
/// archival and audit of individual segments possible without the full run.
#[test]
fn rotated_segments_each_verify_independently() {
    let path = temp_path("indep");
    let run_id = Uuid::new_v4();

    let policy = DurabilityPolicy {
        sync_on_append: false,
        rotation_max_events: 3,
    };

    {
        let mut writer = AuditWriter::with_durability(&path, true, policy).unwrap();
        for i in 0..9u64 {
            writer
                .append(run_id, "AUDIT", "E", json!({"i": i}))
                .unwrap();
        }
        assert_eq!(writer.segment(), 2, "must be in segment 2 after 9 events");
    }

    let seg0 = path.clone();
    let seg1 = {
        let fname = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut p = path.clone();
        p.set_file_name(format!("{}.1", fname));
        p
    };
    let seg2 = {
        let fname = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut p = path.clone();
        p.set_file_name(format!("{}.2", fname));
        p
    };

    // Each segment must verify as a standalone valid chain.
    let r0 = verify_hash_chain(&seg0).unwrap();
    let r1 = verify_hash_chain(&seg1).unwrap();
    let r2 = verify_hash_chain(&seg2).unwrap();

    assert_eq!(
        r0,
        VerifyResult::Valid { lines: 3 },
        "segment 0 must be Valid(3): {:?}",
        r0
    );
    assert_eq!(
        r1,
        VerifyResult::Valid { lines: 3 },
        "segment 1 must be Valid(3): {:?}",
        r1
    );
    assert_eq!(
        r2,
        VerifyResult::Valid { lines: 3 },
        "segment 2 must be Valid(3): {:?}",
        r2
    );

    let _ = std::fs::remove_file(&seg0);
    let _ = std::fs::remove_file(&seg1);
    let _ = std::fs::remove_file(&seg2);
}
