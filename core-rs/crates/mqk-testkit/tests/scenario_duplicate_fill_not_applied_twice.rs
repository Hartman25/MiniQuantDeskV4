//! Scenario: Duplicate Fill Not Applied Twice — Patch L5
//!
//! # Invariant under test
//! The apply gate — keyed by broker fill ID — must prevent the same fill
//! from being applied to the portfolio ledger more than once, regardless of
//! how many times the broker message is received or the event stream is
//! replayed.
//!
//! The gate mirrors the semantics of `mqk_db::inbox_insert_deduped`:
//! - First call for a given `broker_fill_id` → returns `true` → apply runs.
//! - Any subsequent call for the same ID → returns `false` → apply is skipped.
//!
//! All tests are pure in-process; no DB or network required.

use std::collections::HashSet;

use mqk_portfolio::{Fill, Ledger, Side, MICROS_SCALE};

const M: i64 = MICROS_SCALE;

// ---------------------------------------------------------------------------
// Local gate helper — pure in-process stand-in for `inbox_insert_deduped`
// ---------------------------------------------------------------------------

/// Returns `true` the first time `broker_fill_id` is seen; `false` thereafter.
/// Mirrors `inbox_insert_deduped`'s idempotency contract without a database.
fn inbox_gate(seen: &mut HashSet<String>, broker_fill_id: &str) -> bool {
    seen.insert(broker_fill_id.to_string())
}

/// Apply a fill to `ledger` only when the inbox gate permits it.
///
/// Returns `true` if the fill was applied, `false` if it was skipped
/// (duplicate `broker_fill_id`).
fn apply_if_new(
    seen: &mut HashSet<String>,
    ledger: &mut Ledger,
    broker_fill_id: &str,
    fill: Fill,
) -> Result<bool, mqk_portfolio::LedgerError> {
    if inbox_gate(seen, broker_fill_id) {
        ledger.append_fill(fill)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// 1. Duplicate fill ID does not apply twice
// ---------------------------------------------------------------------------

#[test]
fn duplicate_fill_id_does_not_apply_twice() {
    let mut seen = HashSet::new();
    let mut ledger = Ledger::new(100_000 * M);

    let fill = Fill::new("SPY", Side::Buy, 10, 450 * M, 0);

    // First delivery: gate opens → apply runs.
    let applied = apply_if_new(&mut seen, &mut ledger, "BROKER-FILL-1", fill.clone()).unwrap();
    assert!(applied, "first delivery must be applied");
    assert_eq!(ledger.entry_count(), 1);
    assert_eq!(ledger.qty_signed("SPY"), 10);

    // Duplicate delivery (same ID): gate closed → apply skipped.
    let applied = apply_if_new(&mut seen, &mut ledger, "BROKER-FILL-1", fill.clone()).unwrap();
    assert!(!applied, "duplicate delivery must be skipped");
    assert_eq!(
        ledger.entry_count(),
        1,
        "entry count must not increase on duplicate fill"
    );
    assert_eq!(
        ledger.qty_signed("SPY"),
        10,
        "position must not change on duplicate fill"
    );

    // Verify ledger internal consistency.
    assert!(ledger.verify_integrity());
}

// ---------------------------------------------------------------------------
// 2. Distinct fill IDs each apply exactly once
// ---------------------------------------------------------------------------

#[test]
fn distinct_fill_ids_each_apply_exactly_once() {
    let mut seen = HashSet::new();
    let mut ledger = Ledger::new(100_000 * M);

    let fills = vec![
        ("FILL-1", Fill::new("AAPL", Side::Buy, 5, 150 * M, 0)),
        ("FILL-2", Fill::new("AAPL", Side::Buy, 5, 155 * M, 0)),
        ("FILL-3", Fill::new("AAPL", Side::Sell, 3, 160 * M, 0)),
    ];

    // First pass: all apply.
    for (id, fill) in &fills {
        let applied = apply_if_new(&mut seen, &mut ledger, id, fill.clone()).unwrap();
        assert!(applied, "first delivery of fill {id} must be applied");
    }
    assert_eq!(ledger.entry_count(), 3);
    assert_eq!(ledger.qty_signed("AAPL"), 7); // 5 + 5 - 3

    // Replay: none must double-apply.
    for (id, fill) in &fills {
        let applied = apply_if_new(&mut seen, &mut ledger, id, fill.clone()).unwrap();
        assert!(!applied, "replay of fill {id} must be a no-op");
    }
    assert_eq!(
        ledger.entry_count(),
        3,
        "full replay must not change entry count"
    );
    assert_eq!(
        ledger.qty_signed("AAPL"),
        7,
        "position must be unchanged after replay"
    );
}

// ---------------------------------------------------------------------------
// 3. Repeated replay produces identical ledger state
// ---------------------------------------------------------------------------

#[test]
fn repeated_replay_produces_identical_ledger_state() {
    let mut seen = HashSet::new();
    let mut ledger = Ledger::new(50_000 * M);

    let events = vec![
        ("F-1", Fill::new("QQQ", Side::Buy, 20, 300 * M, M)),
        ("F-2", Fill::new("QQQ", Side::Buy, 10, 305 * M, 0)),
        ("F-3", Fill::new("QQQ", Side::Sell, 15, 310 * M, 0)),
    ];

    // Apply the event stream once.
    for (id, fill) in &events {
        apply_if_new(&mut seen, &mut ledger, id, fill.clone()).unwrap();
    }
    let snapshot_after_first_pass = ledger.snapshot();

    // Replay the same stream three more times — state must be immutable.
    for _ in 0..3 {
        for (id, fill) in &events {
            apply_if_new(&mut seen, &mut ledger, id, fill.clone()).unwrap();
        }
    }
    let snapshot_after_replays = ledger.snapshot();

    assert_eq!(
        snapshot_after_first_pass, snapshot_after_replays,
        "ledger state must be identical after arbitrary replay"
    );
    assert!(
        ledger.verify_integrity(),
        "ledger must pass integrity check after replays"
    );
}

// ---------------------------------------------------------------------------
// 4. Identical fill content with distinct IDs applies twice (by design)
// ---------------------------------------------------------------------------

#[test]
fn same_content_different_fill_id_applies_twice() {
    // The gate is keyed on broker_fill_id, NOT on fill content.
    // If the broker issues two genuinely distinct fills that happen to have
    // the same symbol/qty/price, both must be applied.
    let mut seen = HashSet::new();
    let mut ledger = Ledger::new(200_000 * M);

    let fill_a = Fill::new("MSFT", Side::Buy, 10, 300 * M, 0);
    let fill_b = Fill::new("MSFT", Side::Buy, 10, 300 * M, 0); // identical content

    apply_if_new(&mut seen, &mut ledger, "FILL-A", fill_a).unwrap();
    apply_if_new(&mut seen, &mut ledger, "FILL-B", fill_b).unwrap();

    assert_eq!(
        ledger.entry_count(),
        2,
        "two distinct fill IDs must each apply once even when content is identical"
    );
    assert_eq!(
        ledger.qty_signed("MSFT"),
        20,
        "both fills must accumulate into position"
    );
}

// ---------------------------------------------------------------------------
// 5. Gate survives multi-symbol mixed fills and partial replay
// ---------------------------------------------------------------------------

#[test]
fn multi_symbol_partial_replay_is_idempotent() {
    let mut seen = HashSet::new();
    let mut ledger = Ledger::new(500_000 * M);

    let events = vec![
        ("f1", Fill::new("AAPL", Side::Buy, 10, 150 * M, 0)),
        ("f2", Fill::new("MSFT", Side::Buy, 20, 300 * M, 0)),
        ("f3", Fill::new("AAPL", Side::Sell, 5, 155 * M, 0)),
        ("f4", Fill::new("TSLA", Side::Buy, 3, 250 * M, M)),
        ("f5", Fill::new("MSFT", Side::Sell, 10, 310 * M, 0)),
    ];

    // Full first pass.
    for (id, fill) in &events {
        apply_if_new(&mut seen, &mut ledger, id, fill.clone()).unwrap();
    }

    let snap1 = ledger.snapshot();
    assert_eq!(snap1.entry_count, 5);

    // Replay only the first two events (partial replay).
    for (id, fill) in &events[..2] {
        apply_if_new(&mut seen, &mut ledger, id, fill.clone()).unwrap();
    }

    let snap2 = ledger.snapshot();
    assert_eq!(snap1, snap2, "partial replay must not alter ledger state");

    // Full replay.
    for (id, fill) in &events {
        apply_if_new(&mut seen, &mut ledger, id, fill.clone()).unwrap();
    }

    let snap3 = ledger.snapshot();
    assert_eq!(
        snap1, snap3,
        "full replay must produce same state as after initial pass"
    );

    assert!(ledger.verify_integrity());
}
