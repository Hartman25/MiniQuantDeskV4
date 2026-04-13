//! DATA-INTEGRITY-01 (DB) — Durable inbox/portfolio truth proofs.
//!
//! Closes the proof gap left by the pure in-process tests in
//! `scenario_data_integrity_01.rs`, which use HashSet / SimInboxEntry
//! substitutes and cannot prove durable row lifecycle or cross-restart
//! portfolio convergence.
//!
//! # Invariants under test
//!
//! DB-DI-01: Replay after restart does not produce extra durable inbox rows
//!           or extra portfolio effects.  The `inbox_load_all_applied_for_run`
//!           lane returns exactly the same rows before and after a replay
//!           storm; the portfolio rebuilt from those rows is identical.
//!
//! DB-DI-02: Duplicate broker events across a restart boundary collapse to
//!           exactly one durable inbox row and therefore exactly one portfolio
//!           effect regardless of how many times the event is re-delivered.
//!
//! DB-DI-03: Recovery from a mixed applied/unapplied inbox state
//!           (`inbox_load_all_applied_for_run` + `inbox_load_unapplied_for_run`)
//!           produces exactly the same portfolio truth as the clean path where
//!           all rows were applied without any crash window.
//!
//! All tests skip gracefully when `MQK_DATABASE_URL` is absent or unreachable.
//!
//! Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-testkit --test scenario_data_integrity_01_db \
//!     -- --include-ignored --test-threads=1

use anyhow::Result;
use chrono::Utc;
use mqk_portfolio::{apply_entry, Fill, LedgerEntry, PortfolioState, Side, MICROS_SCALE};
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn db_url_or_skip() -> Option<String> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            println!(
                "SKIP: MQK_DATABASE_URL not set \
                 — skipping DB-backed DATA-INTEGRITY-01 proofs"
            );
            None
        }
    }
}

async fn try_pool(url: &str) -> Option<PgPool> {
    match PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await
    {
        Ok(p) => Some(p),
        Err(e) => {
            println!("SKIP: cannot connect to DB: {e}");
            None
        }
    }
}

async fn make_run(pool: &PgPool) -> Result<Uuid> {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "di01-db-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "DI01-DB".to_string(),
            config_hash: "CFG-DI01".to_string(),
            config_json: json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;
    Ok(run_id)
}

/// Reconstruct a Fill from an inbox row's `message_json`.
///
/// Expected JSON keys: `symbol` (str), `side` (str, "buy"/"sell"),
/// `delta_qty` (i64), `price_micros` (i64), `fee_micros` (i64).
/// This mirrors the fields the orchestrator's Phase 3b reads from the inbox
/// payload when applying fills to in-process portfolio state.
fn fill_from_row(row: &mqk_db::InboxRow) -> Fill {
    let j = &row.message_json;
    let symbol = j["symbol"].as_str().unwrap_or("UNKNOWN");
    let side = match j["side"].as_str().unwrap_or("buy") {
        s if s.to_lowercase().starts_with('b') => Side::Buy,
        _ => Side::Sell,
    };
    let qty = j["delta_qty"].as_i64().unwrap_or(0);
    let price = j["price_micros"].as_i64().unwrap_or(0);
    let fee = j["fee_micros"].as_i64().unwrap_or(0);
    Fill::new(symbol, side, qty, price, fee)
}

/// Replay a slice of inbox rows (ordered inbox_id asc — as returned by both
/// `inbox_load_all_applied_for_run` and `inbox_load_unapplied_for_run`) into a
/// fresh PortfolioState and return it.
fn portfolio_from_rows(rows: &[mqk_db::InboxRow]) -> PortfolioState {
    let mut pf = PortfolioState::new(0);
    for row in rows {
        apply_entry(&mut pf, LedgerEntry::Fill(fill_from_row(row)));
    }
    pf
}

fn qty_of(pf: &PortfolioState, symbol: &str) -> i64 {
    pf.positions
        .get(symbol)
        .map(|p| p.qty_signed())
        .unwrap_or(0)
}

/// Best-effort cleanup: inbox rows must be deleted before the run row due to FK.
async fn cleanup(pool: &PgPool, run_ids: &[Uuid]) {
    for &rid in run_ids {
        let _ = sqlx::query("delete from oms_inbox where run_id = $1")
            .bind(rid)
            .execute(pool)
            .await;
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(rid)
            .execute(pool)
            .await;
    }
}

// ---------------------------------------------------------------------------
// DB-DI-01: replay_after_restart_does_not_duplicate_durable_effects_db
//
// A clean ingest cycle (insert → mark_applied × 3) is followed by a restart
// replay of the same three broker_message_ids.  After replay:
//
//  1. Every re-insert returns false  (durable dedupe held).
//  2. inbox_load_unapplied_for_run   → 0 rows (no phantom unapplied rows).
//  3. inbox_load_all_applied_for_run → still exactly 3 rows.
//  4. Portfolio rebuilt from (3) == the single-pass clean portfolio.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn replay_after_restart_does_not_duplicate_durable_effects_db() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id = make_run(&pool).await?;

    // 3-event sequence: partial fill ×2, final fill.  SPY total = 100.
    let events: &[(&str, serde_json::Value)] = &[
        (
            "di01-r1-pf1",
            json!({
                "symbol": "SPY", "side": "buy",
                "delta_qty": 30_i64,
                "price_micros": 500_i64 * MICROS_SCALE,
                "fee_micros": 0_i64
            }),
        ),
        (
            "di01-r1-pf2",
            json!({
                "symbol": "SPY", "side": "buy",
                "delta_qty": 40_i64,
                "price_micros": 501_i64 * MICROS_SCALE,
                "fee_micros": 0_i64
            }),
        ),
        (
            "di01-r1-fin",
            json!({
                "symbol": "SPY", "side": "buy",
                "delta_qty": 30_i64,
                "price_micros": 502_i64 * MICROS_SCALE,
                "fee_micros": 0_i64
            }),
        ),
    ];

    let applied_at = Utc::now();

    // Phase 1: single clean ingest cycle — insert + mark applied.
    for (msg_id, payload) in events {
        let ins = mqk_db::inbox_insert_deduped(&pool, run_id, msg_id, payload.clone()).await?;
        assert!(
            ins,
            "DB-DI-01: first insert of {msg_id} must be accepted (returned true)"
        );
        mqk_db::inbox_mark_applied(&pool, run_id, msg_id, applied_at).await?;
    }

    // Phase 2: simulated restart — same broker_message_ids re-delivered.
    for (msg_id, payload) in events {
        let ins = mqk_db::inbox_insert_deduped(&pool, run_id, msg_id, payload.clone()).await?;
        assert!(
            !ins,
            "DB-DI-01: restart replay of {msg_id} must be deduped (returned false) \
             — no second durable row created"
        );
    }

    // No phantom unapplied rows: replay did not inject new unapplied entries.
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert_eq!(
        unapplied.len(),
        0,
        "DB-DI-01: inbox_load_unapplied_for_run must return 0 rows after restart replay \
         — dedupe prevented new rows from being written"
    );

    // Applied lane unchanged: still exactly 3 rows — no duplicates.
    let applied = mqk_db::inbox_load_all_applied_for_run(&pool, run_id).await?;
    assert_eq!(
        applied.len(),
        3,
        "DB-DI-01: inbox_load_all_applied_for_run must return exactly 3 rows \
         — restart replay did not create extra durable effects"
    );

    // Portfolio rebuilt from the 3 applied rows matches the expected single-pass truth.
    let pf = portfolio_from_rows(&applied);
    assert_eq!(
        qty_of(&pf, "SPY"),
        100,
        "DB-DI-01: SPY qty must be exactly 100 — no duplicate portfolio effects \
         despite the restart replay storm"
    );

    cleanup(&pool, &[run_id]).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// DB-DI-02: duplicate_broker_events_across_restart_boundary_remain_single_effect_db
//
// The same broker_message_id is delivered once before a simulated restart and
// then 10 more times after.  All post-first inserts must return false.
//
//  1. Only one unapplied row exists (the original pre-restart insert).
//  2. Applying and marking that one row produces AAPL qty = 10, not 110.
//  3. After mark_applied: 0 unapplied rows, 1 applied row.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_broker_events_across_restart_boundary_remain_single_effect_db() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id = make_run(&pool).await?;

    // Unique message ID scoped to this test invocation.
    let msg_id = format!("di01-dup-{}", Uuid::new_v4());
    let payload = json!({
        "symbol": "AAPL", "side": "buy",
        "delta_qty": 10_i64,
        "price_micros": 150_i64 * MICROS_SCALE,
        "fee_micros": 0_i64
    });

    // Pre-restart: first and only legitimate delivery.
    let ins = mqk_db::inbox_insert_deduped(&pool, run_id, &msg_id, payload.clone()).await?;
    assert!(ins, "DB-DI-02: first delivery must be accepted");

    // Simulated restart: same broker event re-delivered 10 times.
    for i in 0..10_usize {
        let ins = mqk_db::inbox_insert_deduped(&pool, run_id, &msg_id, payload.clone()).await?;
        assert!(
            !ins,
            "DB-DI-02: restart re-delivery #{i} must be deduped (returned false)"
        );
    }

    // Exactly one unapplied row — the duplicate storm collapsed into one.
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    let matching: Vec<_> = unapplied
        .iter()
        .filter(|r| r.broker_message_id == msg_id)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "DB-DI-02: exactly one unapplied row must exist \
         — 10-delivery duplicate storm collapsed to one durable row"
    );

    // Apply that single row — produces exactly one fill effect.
    let mut pf = PortfolioState::new(0);
    for row in &unapplied {
        apply_entry(&mut pf, LedgerEntry::Fill(fill_from_row(row)));
        mqk_db::inbox_mark_applied(&pool, run_id, &row.broker_message_id, Utc::now()).await?;
    }

    // AAPL qty = 10 (one fill of 10), not 110 (one fill per delivery).
    assert_eq!(
        qty_of(&pf, "AAPL"),
        10,
        "DB-DI-02: AAPL qty must be 10 — duplicate delivery storm must not \
         produce multiple portfolio effects"
    );

    // Post-apply: unapplied lane is empty, applied lane has exactly one row.
    let unapplied_after = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert_eq!(
        unapplied_after
            .iter()
            .filter(|r| r.broker_message_id == msg_id)
            .count(),
        0,
        "DB-DI-02: no unapplied rows must remain after mark_applied"
    );

    let applied_after = mqk_db::inbox_load_all_applied_for_run(&pool, run_id).await?;
    assert_eq!(
        applied_after
            .iter()
            .filter(|r| r.broker_message_id == msg_id)
            .count(),
        1,
        "DB-DI-02: exactly one applied row must exist \
         — duplicate storm did not create extra durable rows"
    );

    cleanup(&pool, &[run_id]).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// DB-DI-03: applied_and_unapplied_inbox_recovery_matches_clean_portfolio_truth_db
//
// Two runs receive the same four fill events.
//
// clean_run: all four events inserted and marked applied (happy path).
//
// crash_run: all four events inserted; crash occurs after marking only the
//            first two applied (ev-a3 and ev-a4 remain unapplied).
//
// Recovery for crash_run:
//   1. load_all_applied_for_run  → 2 rows → rebuild partial portfolio
//   2. load_unapplied_for_run    → 2 rows → apply + mark_applied each
//
// Convergence assertion:
//   crash_pf == clean_pf (same SPY qty, same MSFT qty)
//   crash_run ends with 4 applied rows and 0 unapplied rows.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn applied_and_unapplied_inbox_recovery_matches_clean_portfolio_truth_db() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let clean_run = make_run(&pool).await?;
    let crash_run = make_run(&pool).await?;

    // 4 fill events:  SPY 30 + 40 + 30 = 100 shares, MSFT 5 shares.
    let fills: &[(&str, serde_json::Value)] = &[
        (
            "ev-a1",
            json!({
                "symbol": "SPY", "side": "buy",
                "delta_qty": 30_i64,
                "price_micros": 500_i64 * MICROS_SCALE,
                "fee_micros": 0_i64
            }),
        ),
        (
            "ev-a2",
            json!({
                "symbol": "SPY", "side": "buy",
                "delta_qty": 40_i64,
                "price_micros": 501_i64 * MICROS_SCALE,
                "fee_micros": 0_i64
            }),
        ),
        (
            "ev-a3",
            json!({
                "symbol": "SPY", "side": "buy",
                "delta_qty": 30_i64,
                "price_micros": 502_i64 * MICROS_SCALE,
                "fee_micros": 0_i64
            }),
        ),
        (
            "ev-a4",
            json!({
                "symbol": "MSFT", "side": "buy",
                "delta_qty": 5_i64,
                "price_micros": 300_i64 * MICROS_SCALE,
                "fee_micros": 0_i64
            }),
        ),
    ];

    let mark_ts = Utc::now();

    // --- Clean path: insert all 4, mark all 4 applied ---
    for (msg_id, payload) in fills {
        let ins = mqk_db::inbox_insert_deduped(&pool, clean_run, msg_id, payload.clone()).await?;
        assert!(ins, "DB-DI-03: clean_run insert {msg_id} must succeed");
        mqk_db::inbox_mark_applied(&pool, clean_run, msg_id, mark_ts).await?;
    }

    // --- Crash path: insert all 4, mark only first 2 applied ---
    for (msg_id, payload) in fills {
        let ins = mqk_db::inbox_insert_deduped(&pool, crash_run, msg_id, payload.clone()).await?;
        assert!(ins, "DB-DI-03: crash_run insert {msg_id} must succeed");
    }
    // Simulate crash after ev-a2 apply completed; ev-a3 and ev-a4 are in the
    // inbox but inbox_mark_applied was never reached for them.
    mqk_db::inbox_mark_applied(&pool, crash_run, "ev-a1", mark_ts).await?;
    mqk_db::inbox_mark_applied(&pool, crash_run, "ev-a2", mark_ts).await?;

    // --- Clean path portfolio ---
    let clean_applied = mqk_db::inbox_load_all_applied_for_run(&pool, clean_run).await?;
    assert_eq!(
        clean_applied.len(),
        4,
        "DB-DI-03: clean_run must have 4 applied rows"
    );
    let clean_pf = portfolio_from_rows(&clean_applied);

    assert_eq!(
        qty_of(&clean_pf, "SPY"),
        100,
        "DB-DI-03: clean path SPY qty must be 100"
    );
    assert_eq!(
        qty_of(&clean_pf, "MSFT"),
        5,
        "DB-DI-03: clean path MSFT qty must be 5"
    );

    // --- Crash recovery portfolio ---

    // Phase 1: cold-start replay of already-applied rows.
    let crash_applied_pre = mqk_db::inbox_load_all_applied_for_run(&pool, crash_run).await?;
    assert_eq!(
        crash_applied_pre.len(),
        2,
        "DB-DI-03: crash_run must have exactly 2 applied rows before recovery"
    );
    let mut crash_pf = portfolio_from_rows(&crash_applied_pre);

    // Phase 2: crash recovery — apply unapplied rows and stamp them.
    let crash_unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, crash_run).await?;
    assert_eq!(
        crash_unapplied.len(),
        2,
        "DB-DI-03: crash_run must have exactly 2 unapplied rows (crash window)"
    );
    for row in &crash_unapplied {
        apply_entry(&mut crash_pf, LedgerEntry::Fill(fill_from_row(row)));
        mqk_db::inbox_mark_applied(&pool, crash_run, &row.broker_message_id, Utc::now()).await?;
    }

    // --- Convergence proof ---
    assert_eq!(
        qty_of(&crash_pf, "SPY"),
        qty_of(&clean_pf, "SPY"),
        "DB-DI-03: crash recovery SPY qty must equal clean path \
         — applied+unapplied recovery converges to same portfolio truth"
    );
    assert_eq!(
        qty_of(&crash_pf, "MSFT"),
        qty_of(&clean_pf, "MSFT"),
        "DB-DI-03: crash recovery MSFT qty must equal clean path"
    );

    // Post-recovery invariant: crash_run must now have 4 applied, 0 unapplied.
    let crash_applied_final = mqk_db::inbox_load_all_applied_for_run(&pool, crash_run).await?;
    let crash_unapplied_final = mqk_db::inbox_load_unapplied_for_run(&pool, crash_run).await?;
    assert_eq!(
        crash_applied_final.len(),
        4,
        "DB-DI-03: crash_run must have 4 applied rows after full recovery"
    );
    assert_eq!(
        crash_unapplied_final.len(),
        0,
        "DB-DI-03: crash_run must have 0 unapplied rows after full recovery"
    );

    cleanup(&pool, &[clean_run, crash_run]).await;
    Ok(())
}
