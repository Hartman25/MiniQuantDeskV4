//! DURABLE-PAPER-PORTFOLIO-AND-PNL-01B: durable paper portfolio store proof.
//!
//! Proves `insert_or_confirm_paper_portfolio_snapshot` /
//! `fetch_latest_paper_portfolio_snapshot` / `fetch_paper_portfolio_snapshot_by_id` /
//! `fetch_recent_paper_portfolio_snapshots` and
//! `upsert_paper_portfolio_accounting_state` / `fetch_paper_portfolio_accounting_state`
//! round-trip, idempotent-replay, conflict-detection, and ordering behavior —
//! with zero provider/broker/network calls and zero writes to
//! `oms_outbox`/`oms_inbox`/`runs` beyond the one fixture run row each test
//! creates for itself.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored --test-threads=1

use chrono::{TimeZone, Utc};
use mqk_db::{
    fetch_latest_paper_portfolio_snapshot, fetch_latest_paper_portfolio_snapshot_for_run,
    fetch_paper_portfolio_accounting_state, fetch_paper_portfolio_snapshot_by_id,
    fetch_recent_paper_portfolio_snapshots, insert_or_confirm_paper_portfolio_snapshot, insert_run,
    upsert_paper_portfolio_accounting_state, InsertPaperPortfolioSnapshotOutcome,
    NewPaperPortfolioSnapshot, NewRun, PaperPortfolioSnapshotPosition,
    UpsertPaperPortfolioAccountingStateArgs, UpsertPaperPortfolioAccountingStateOutcome,
    ENV_DB_URL, PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE,
    PAPER_PORTFOLIO_ACCOUNTING_EPOCH_INCOMPLETE, PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
    PAPER_PORTFOLIO_SNAPSHOT_SOURCE_SYNTHETIC_DIAGNOSTIC,
};
use uuid::Uuid;

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var(ENV_DB_URL).is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    let pool = mqk_db::testkit_db_pool().await?;
    Ok(pool)
}

async fn cleanup(pool: &sqlx::PgPool, run_ids: &[Uuid]) {
    for run_id in run_ids {
        let _ = sqlx::query("delete from sys_paper_portfolio_accounting_state where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
        let _ = sqlx::query(
            "delete from sys_paper_portfolio_snapshot_positions where snapshot_id in (select snapshot_id from sys_paper_portfolio_snapshots where run_id = $1)",
        )
        .bind(run_id)
        .execute(pool)
        .await;
        let _ = sqlx::query("delete from sys_paper_portfolio_snapshots where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
    }
}

async fn fixture_run(pool: &sqlx::PgPool, run_id: Uuid) {
    insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: "test-paper-portfolio-store".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("fixture run insert should succeed");
}

fn fixed_run_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.paper-portfolio-store.v1|{seed}").as_bytes(),
    )
}

/// Inserts a minimal durable snapshot for `run_id` and returns its
/// `snapshot_id` -- `source_snapshot_id` is a real FK
/// (`sys_paper_portfolio_accounting_state.source_snapshot_id` references
/// `sys_paper_portfolio_snapshots.snapshot_id`), so any accounting-only
/// test that supplies a `source_snapshot_id` must first insert the
/// snapshot row it points at.
async fn fixture_snapshot(pool: &sqlx::PgPool, run_id: Uuid, seed: &str) -> Uuid {
    fixture_snapshot_at(
        pool,
        run_id,
        seed,
        Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
    )
    .await
}

/// Same as [`fixture_snapshot`], but with an explicit `captured_at_utc` --
/// needed by tests proving the B4 closure repair's snapshot-authority
/// ordering (A4/A5: newer `captured_at_utc` wins), where two fixture
/// snapshots for the same run must have a well-defined relative order.
async fn fixture_snapshot_at(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    seed: &str,
    captured_at_utc: chrono::DateTime<Utc>,
) -> Uuid {
    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.paper-portfolio-store.v1|snapshot|{seed}").as_bytes(),
    );
    insert_or_confirm_paper_portfolio_snapshot(
        pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc,
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("fixture snapshot insert should succeed");
    snapshot_id
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

/// Empty store: no snapshot exists yet for a fresh run.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn empty_store_returns_none() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("empty_store_returns_none");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let latest = fetch_latest_paper_portfolio_snapshot(
        &pool,
        "paper",
        PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
    )
    .await
    .expect("fetch should succeed");
    // Other tests' rows may exist for "paper"/"external_alpaca" globally, so
    // only assert this specific run_id never appears.
    if let Some(latest) = latest {
        assert_ne!(latest.snapshot.run_id, Some(run_id));
    }

    let accounting = fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed");
    assert!(accounting.is_none(), "no accounting row should exist yet");

    cleanup(&pool, &[run_id]).await;
}

/// First snapshot insert, with a non-flat position, round-trips exactly.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn first_snapshot_insert_round_trips() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("first_snapshot_insert_round_trips");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-snapshot.v1|first_insert",
    );
    let captured_at_utc = Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap();

    let outcome = insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc,
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![PaperPortfolioSnapshotPosition {
                symbol: "AAPL".to_string(),
                qty_signed: 10,
                avg_entry_price_micros: 150_000_000,
                provenance: "external_alpaca".to_string(),
            }],
        },
    )
    .await
    .expect("insert should succeed");

    match outcome {
        InsertPaperPortfolioSnapshotOutcome::Inserted {
            record,
            positions_inserted,
        } => {
            assert_eq!(record.snapshot_id, snapshot_id);
            assert_eq!(record.equity_micros, 100_000_000_000);
            assert_eq!(positions_inserted, 1);
        }
        other => panic!("expected Inserted, got {other:?}"),
    }

    let fetched = fetch_paper_portfolio_snapshot_by_id(&pool, snapshot_id)
        .await
        .expect("fetch should succeed")
        .expect("snapshot should exist");
    assert_eq!(fetched.snapshot.cash_micros, 40_000_000_000);
    assert_eq!(fetched.positions.len(), 1);
    assert_eq!(fetched.positions[0].symbol, "AAPL");
    assert_eq!(fetched.positions[0].qty_signed, 10);

    cleanup(&pool, &[run_id]).await;
}

/// Exact idempotent replay of an identical snapshot (same id, same scalar
/// fields, same position set) is a no-op confirmation, not a new row.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn exact_replay_is_idempotent() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("exact_replay_is_idempotent");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-snapshot.v1|exact_replay",
    );
    let make_args = || NewPaperPortfolioSnapshot {
        snapshot_id,
        captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
        deployment_mode: "paper".to_string(),
        source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
        equity_micros: 100_000_000_000,
        cash_micros: 40_000_000_000,
        currency: "USD".to_string(),
        truth_state: "active".to_string(),
        run_id: Some(run_id),
        operation_id: None,
        positions: vec![PaperPortfolioSnapshotPosition {
            symbol: "AAPL".to_string(),
            qty_signed: 10,
            avg_entry_price_micros: 150_000_000,
            provenance: "external_alpaca".to_string(),
        }],
    };

    let first = insert_or_confirm_paper_portfolio_snapshot(&pool, make_args())
        .await
        .expect("first insert should succeed");
    assert!(matches!(
        first,
        InsertPaperPortfolioSnapshotOutcome::Inserted { .. }
    ));

    let second = insert_or_confirm_paper_portfolio_snapshot(&pool, make_args())
        .await
        .expect("second insert should succeed");
    assert!(matches!(
        second,
        InsertPaperPortfolioSnapshotOutcome::AlreadyExists { .. }
    ));

    let count: i64 = sqlx::query_scalar(
        "select count(*) from sys_paper_portfolio_snapshots where snapshot_id = $1",
    )
    .bind(snapshot_id)
    .fetch_one(&pool)
    .await
    .expect("count query should succeed");
    assert_eq!(count, 1, "exactly one row must exist for this snapshot_id");

    let position_count: i64 = sqlx::query_scalar(
        "select count(*) from sys_paper_portfolio_snapshot_positions where snapshot_id = $1",
    )
    .bind(snapshot_id)
    .fetch_one(&pool)
    .await
    .expect("count query should succeed");
    assert_eq!(
        position_count, 1,
        "positions must not be duplicated on replay"
    );

    cleanup(&pool, &[run_id]).await;
}

/// A conflicting replay (same snapshot_id, different content) is a typed
/// conflict, and the original row is never overwritten.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn conflicting_replay_is_rejected() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("conflicting_replay_is_rejected");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-snapshot.v1|conflict",
    );

    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("first insert should succeed");

    let conflicting = insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 999_000_000_000, // different content, same id
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("conflicting insert call should succeed at the Rust level (returns Conflict)");
    assert!(matches!(
        conflicting,
        InsertPaperPortfolioSnapshotOutcome::Conflict { .. }
    ));

    let fetched = fetch_paper_portfolio_snapshot_by_id(&pool, snapshot_id)
        .await
        .expect("fetch should succeed")
        .expect("snapshot should exist");
    assert_eq!(
        fetched.snapshot.equity_micros, 100_000_000_000,
        "original row must be unchanged after a conflicting replay attempt"
    );

    cleanup(&pool, &[run_id]).await;
}

/// Multiple snapshots for the same deployment_mode/source are returned in
/// deterministic newest-first order by `fetch_recent_paper_portfolio_snapshots`,
/// and `fetch_latest_paper_portfolio_snapshot` returns exactly the newest one.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn multiple_snapshots_ordered_deterministically() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("multiple_snapshots_ordered_deterministically");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let mut ids = Vec::new();
    for i in 0..3i64 {
        let snapshot_id = Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!("test.paper-portfolio-snapshot.v1|ordered|{i}").as_bytes(),
        );
        ids.push(snapshot_id);
        insert_or_confirm_paper_portfolio_snapshot(
            &pool,
            NewPaperPortfolioSnapshot {
                snapshot_id,
                captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap()
                    + chrono::Duration::minutes(i),
                deployment_mode: "paper".to_string(),
                source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
                equity_micros: 100_000_000_000 + i * 1_000_000,
                cash_micros: 40_000_000_000,
                currency: "USD".to_string(),
                truth_state: "active".to_string(),
                run_id: Some(run_id),
                operation_id: None,
                positions: vec![],
            },
        )
        .await
        .expect("insert should succeed");
    }

    let latest = fetch_latest_paper_portfolio_snapshot(
        &pool,
        "paper",
        PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
    )
    .await
    .expect("fetch should succeed")
    .expect("latest should exist");
    assert_eq!(
        latest.snapshot.snapshot_id, ids[2],
        "latest must be the most recently captured"
    );

    let recent = fetch_recent_paper_portfolio_snapshots(
        &pool,
        "paper",
        PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        10,
    )
    .await
    .expect("fetch should succeed");
    let this_run_ids: Vec<Uuid> = recent
        .iter()
        .filter(|r| r.run_id == Some(run_id))
        .map(|r| r.snapshot_id)
        .collect();
    assert_eq!(
        this_run_ids,
        vec![ids[2], ids[1], ids[0]],
        "must be newest-first"
    );

    cleanup(&pool, &[run_id]).await;
}

/// `fetch_latest_paper_portfolio_snapshot_for_run` never returns a snapshot
/// belonging to a different run, even when that other run's snapshot is
/// strictly newer -- proving the B4 closure repair's cross-run isolation.
/// Also proves a snapshot with `run_id = None` can never satisfy a
/// run-scoped query.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn run_scoped_snapshot_never_crosses_runs() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_a = fixed_run_id("run_scoped_snapshot_never_crosses_runs.a");
    let run_b = fixed_run_id("run_scoped_snapshot_never_crosses_runs.b");
    cleanup(&pool, &[run_a, run_b]).await;
    fixture_run(&pool, run_a).await;
    fixture_run(&pool, run_b).await;

    let snapshot_a = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-store.v1|run-scoped-a",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id: snapshot_a,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_a),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert should succeed");

    // run_b's snapshot is strictly NEWER than run_a's.
    let snapshot_b = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-store.v1|run-scoped-b",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id: snapshot_b,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 14, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 200_000_000_000,
            cash_micros: 80_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_b),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert should succeed");

    // A run-scoped query for run_a must return run_a's snapshot, never
    // run_b's newer one.
    let fetched_a = fetch_latest_paper_portfolio_snapshot_for_run(
        &pool,
        "paper",
        PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        run_a,
    )
    .await
    .expect("fetch should succeed")
    .expect("run_a snapshot should exist");
    assert_eq!(
        fetched_a.snapshot.snapshot_id, snapshot_a,
        "run-scoped query for run_a must never return run_b's newer snapshot"
    );
    assert_eq!(fetched_a.snapshot.run_id, Some(run_a));

    // A snapshot with run_id = None must never satisfy a run-scoped query.
    let snapshot_null_run = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-store.v1|run-scoped-null-run",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id: snapshot_null_run,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 15, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 300_000_000_000,
            cash_micros: 90_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: None,
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert should succeed");

    let fetched_a_again = fetch_latest_paper_portfolio_snapshot_for_run(
        &pool,
        "paper",
        PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        run_a,
    )
    .await
    .expect("fetch should succeed")
    .expect("run_a snapshot should still exist");
    assert_eq!(
        fetched_a_again.snapshot.snapshot_id, snapshot_a,
        "a null-run_id snapshot must never satisfy a run-scoped query, even when newest"
    );

    // A run with no snapshot at all resolves to None, never a fabricated row.
    let run_c = fixed_run_id("run_scoped_snapshot_never_crosses_runs.c");
    fixture_run(&pool, run_c).await;
    let fetched_c = fetch_latest_paper_portfolio_snapshot_for_run(
        &pool,
        "paper",
        PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        run_c,
    )
    .await
    .expect("fetch should succeed");
    assert!(fetched_c.is_none());

    cleanup(&pool, &[run_a, run_b, run_c]).await;
    let _ = sqlx::query("delete from sys_paper_portfolio_snapshots where snapshot_id = $1")
        .bind(snapshot_null_run)
        .execute(&pool)
        .await;
}

/// A flat account (no positions) round-trips with zero position rows.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn flat_account_has_zero_positions() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("flat_account_has_zero_positions");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-snapshot.v1|flat",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 100_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert should succeed");

    let fetched = fetch_paper_portfolio_snapshot_by_id(&pool, snapshot_id)
        .await
        .expect("fetch should succeed")
        .expect("snapshot should exist");
    assert!(fetched.positions.is_empty());
    assert_eq!(fetched.snapshot.cash_micros, fetched.snapshot.equity_micros);

    cleanup(&pool, &[run_id]).await;
}

/// A synthesized/diagnostic snapshot is stored with a distinct `source` tag
/// and never appears when querying for `external_alpaca`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn synthetic_source_never_masquerades_as_external() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("synthetic_source_never_masquerades_as_external");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-snapshot.v1|synthetic",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_SYNTHETIC_DIAGNOSTIC.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 100_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert should succeed");

    let latest_external = fetch_latest_paper_portfolio_snapshot(
        &pool,
        "paper",
        PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
    )
    .await
    .expect("fetch should succeed");
    if let Some(latest_external) = latest_external {
        assert_ne!(latest_external.snapshot.snapshot_id, snapshot_id);
    }

    let latest_synthetic = fetch_latest_paper_portfolio_snapshot(
        &pool,
        "paper",
        PAPER_PORTFOLIO_SNAPSHOT_SOURCE_SYNTHETIC_DIAGNOSTIC,
    )
    .await
    .expect("fetch should succeed")
    .expect("synthetic snapshot should exist");
    assert_eq!(latest_synthetic.snapshot.snapshot_id, snapshot_id);

    cleanup(&pool, &[run_id]).await;
}

/// An invalid `source` value is rejected before any write.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn invalid_source_rejected() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("invalid_source_rejected");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-snapshot.v1|invalid_source",
    );
    let result = insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: "made_up_source".to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 100_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await;
    assert!(result.is_err(), "an invalid source must be rejected");

    let count: i64 = sqlx::query_scalar(
        "select count(*) from sys_paper_portfolio_snapshots where snapshot_id = $1",
    )
    .bind(snapshot_id)
    .fetch_one(&pool)
    .await
    .expect("count query should succeed");
    assert_eq!(count, 0, "no row must be written when validation fails");

    cleanup(&pool, &[run_id]).await;
}

// ---------------------------------------------------------------------------
// Accounting state
// ---------------------------------------------------------------------------

/// First accounting-state write for a run is an Insert.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn first_accounting_state_write_inserts() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("first_accounting_state_write_inserts");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;
    let snapshot_id = fixture_snapshot(&pool, run_id, "first_accounting_state_write_inserts").await;

    let outcome = upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 40_000_000_000,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 5,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("upsert should succeed");
    assert!(matches!(
        outcome,
        UpsertPaperPortfolioAccountingStateOutcome::Inserted { .. }
    ));

    let fetched = fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("row should exist");
    assert_eq!(fetched.last_applied_inbox_id, 5);
    assert_eq!(
        fetched.accounting_epoch,
        PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE
    );

    cleanup(&pool, &[run_id]).await;
}

/// Re-confirming the same watermark with identical content is an idempotent
/// no-op (AlreadyCurrent), advancing to a higher watermark is a real Update,
/// and attempting to regress the watermark is Rejected without mutating the
/// row. Also proves the B4 closure repair: a same-watermark replay whose
/// fill-derived values differ from the stored row is a Conflict (not a
/// silent AlreadyCurrent) -- the previous behavior compared the watermark
/// only and returned AlreadyCurrent unconditionally, which could mask a
/// nondeterministic-replay bug.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn accounting_state_watermark_idempotency_and_ordering() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("accounting_state_watermark_idempotency_and_ordering");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    // snapshot_b is deliberately captured strictly later than snapshot_a --
    // the B4 closure repair (A5) requires a different-snapshot
    // same-watermark update to be authorized only by a strictly newer
    // snapshot under the canonical (captured_at_utc, snapshot_id) order.
    let snapshot_a = fixture_snapshot_at(
        &pool,
        run_id,
        "accounting_state_watermark_idempotency_and_ordering.a",
        Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
    )
    .await;
    let snapshot_b = fixture_snapshot_at(
        &pool,
        run_id,
        "accounting_state_watermark_idempotency_and_ordering.b",
        Utc.with_ymd_and_hms(2099, 2, 1, 13, 5, 0).unwrap(),
    )
    .await;

    let base_args =
        |watermark: i64, cash: i64, snapshot_id: Uuid| UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: cash,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: watermark,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_id,
        };

    let inserted =
        upsert_paper_portfolio_accounting_state(&pool, base_args(5, 40_000_000_000, snapshot_a))
            .await
            .expect("insert should succeed");
    assert!(matches!(
        inserted,
        UpsertPaperPortfolioAccountingStateOutcome::Inserted { .. }
    ));

    // Exact replay at the same watermark, same snapshot, identical content:
    // idempotent no-op.
    let replay =
        upsert_paper_portfolio_accounting_state(&pool, base_args(5, 40_000_000_000, snapshot_a))
            .await
            .expect("replay should succeed");
    match replay {
        UpsertPaperPortfolioAccountingStateOutcome::AlreadyCurrent { record } => {
            assert_eq!(record.cash_micros, 40_000_000_000);
        }
        other => panic!("expected AlreadyCurrent, got {other:?}"),
    }

    // Same watermark, same snapshot, but DIFFERENT fill-derived value:
    // this is nondeterministic-replay territory -- must fail closed as a
    // Conflict, never silently accepted as AlreadyCurrent.
    let conflict =
        upsert_paper_portfolio_accounting_state(&pool, base_args(5, 999_999_999, snapshot_a))
            .await
            .expect("conflicting replay call should succeed at the Rust level (returns Conflict)");
    match conflict {
        UpsertPaperPortfolioAccountingStateOutcome::Conflict { record, .. } => {
            assert_eq!(
                record.cash_micros, 40_000_000_000,
                "Conflict must return the existing stored value, never the caller's differing input"
            );
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    let after_conflict = fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("row should exist");
    assert_eq!(
        after_conflict.cash_micros, 40_000_000_000,
        "a Conflict must never mutate the stored row"
    );

    // Same watermark, same fill-derived values, but a NEW confirmed
    // snapshot: only the snapshot-dependent fields advance.
    let updated_for_snapshot =
        upsert_paper_portfolio_accounting_state(&pool, base_args(5, 40_000_000_000, snapshot_b))
            .await
            .expect("updated-for-snapshot call should succeed");
    match updated_for_snapshot {
        UpsertPaperPortfolioAccountingStateOutcome::UpdatedForSnapshot { record } => {
            assert_eq!(record.source_snapshot_id, Some(snapshot_b));
            assert_eq!(record.cash_micros, 40_000_000_000);
        }
        other => panic!("expected UpdatedForSnapshot, got {other:?}"),
    }

    // Advance to a higher watermark: real update.
    let advanced =
        upsert_paper_portfolio_accounting_state(&pool, base_args(9, 41_000_000_000, snapshot_b))
            .await
            .expect("advance should succeed");
    match advanced {
        UpsertPaperPortfolioAccountingStateOutcome::Updated {
            previous_last_applied_inbox_id,
            record,
        } => {
            assert_eq!(previous_last_applied_inbox_id, 5);
            assert_eq!(record.last_applied_inbox_id, 9);
            assert_eq!(record.cash_micros, 41_000_000_000);
        }
        other => panic!("expected Updated, got {other:?}"),
    }

    // Attempt to regress: rejected, existing row untouched.
    let regressed = upsert_paper_portfolio_accounting_state(&pool, base_args(3, 1, snapshot_b))
        .await
        .expect("regression attempt call should succeed at the Rust level (returns Rejected)");
    match regressed {
        UpsertPaperPortfolioAccountingStateOutcome::Rejected {
            current_last_applied_inbox_id,
            ..
        } => {
            assert_eq!(current_last_applied_inbox_id, 9);
        }
        other => panic!("expected Rejected, got {other:?}"),
    }

    let fetched = fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("row should exist");
    assert_eq!(
        fetched.last_applied_inbox_id, 9,
        "a rejected regression attempt must never mutate the stored watermark"
    );
    assert_eq!(fetched.cash_micros, 41_000_000_000);

    cleanup(&pool, &[run_id]).await;
}

/// An incomplete accounting epoch (pre-existing unmatched position) round-trips
/// its reason string, and is never silently promoted back to `complete`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn incomplete_accounting_epoch_round_trips_reason() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("incomplete_accounting_epoch_round_trips_reason");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;
    let snapshot_id = fixture_snapshot(
        &pool,
        run_id,
        "incomplete_accounting_epoch_round_trips_reason",
    )
    .await;

    upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 40_000_000_000,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 0,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_INCOMPLETE.to_string(),
            accounting_epoch_reason: Some(
                "pre_existing_position_no_opening_fill_in_inbox".to_string(),
            ),
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("insert should succeed");

    let fetched = fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("row should exist");
    assert_eq!(
        fetched.accounting_epoch,
        PAPER_PORTFOLIO_ACCOUNTING_EPOCH_INCOMPLETE
    );
    assert_eq!(
        fetched.accounting_epoch_reason,
        Some("pre_existing_position_no_opening_fill_in_inbox".to_string())
    );

    cleanup(&pool, &[run_id]).await;
}

/// An invalid `accounting_epoch` value is rejected before any write.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn invalid_accounting_epoch_rejected() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("invalid_accounting_epoch_rejected");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let result = upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 40_000_000_000,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 0,
            accounting_epoch: "made_up_epoch".to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: Uuid::new_v5(
                &Uuid::NAMESPACE_DNS,
                b"test.paper-portfolio-store.v1|invalid_accounting_epoch_rejected",
            ),
        },
    )
    .await;
    assert!(
        result.is_err(),
        "an invalid accounting_epoch must be rejected"
    );

    let count: i64 = sqlx::query_scalar(
        "select count(*) from sys_paper_portfolio_accounting_state where run_id = $1",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("count query should succeed");
    assert_eq!(count, 0, "no row must be written when validation fails");

    cleanup(&pool, &[run_id]).await;
}

/// Restart reconstruction: a fresh pool (simulating a new process) reads
/// back exactly what a prior pool wrote, for both snapshot and accounting
/// state.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn restart_reconstruction_reads_back_prior_writes() {
    let pool_before = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("restart_reconstruction_reads_back_prior_writes");
    cleanup(&pool_before, &[run_id]).await;
    fixture_run(&pool_before, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-snapshot.v1|restart",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool_before,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![PaperPortfolioSnapshotPosition {
                symbol: "AAPL".to_string(),
                qty_signed: 10,
                avg_entry_price_micros: 150_000_000,
                provenance: "external_alpaca".to_string(),
            }],
        },
    )
    .await
    .expect("insert should succeed");

    upsert_paper_portfolio_accounting_state(
        &pool_before,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 40_000_000_000,
            realized_pnl_micros: 1_000_000,
            fees_micros: 500,
            last_applied_inbox_id: 3,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("accounting upsert should succeed");

    drop(pool_before);

    // A brand-new pool/connection stands in for a fresh process after
    // restart -- no in-memory state is shared with the writer above.
    let pool_after = test_pool().await.expect("reconnect should succeed");

    let fetched_snapshot = fetch_paper_portfolio_snapshot_by_id(&pool_after, snapshot_id)
        .await
        .expect("fetch should succeed")
        .expect("snapshot should survive restart");
    assert_eq!(fetched_snapshot.snapshot.cash_micros, 40_000_000_000);
    assert_eq!(fetched_snapshot.positions.len(), 1);

    let fetched_accounting = fetch_paper_portfolio_accounting_state(&pool_after, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should survive restart");
    assert_eq!(fetched_accounting.last_applied_inbox_id, 3);
    assert_eq!(fetched_accounting.realized_pnl_micros, 1_000_000);
    assert_eq!(
        fetched_accounting.source_snapshot_id,
        Some(snapshot_id),
        "accounting snapshot provenance must survive a restart"
    );

    cleanup(&pool_after, &[run_id]).await;
}

/// This module never writes to `oms_outbox` or `oms_inbox` -- only its own
/// three tables (plus the fixture `runs` row each test manages itself).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn no_outbox_or_inbox_writes() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("no_outbox_or_inbox_writes");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let outbox_before: i64 =
        sqlx::query_scalar("select count(*) from oms_outbox where run_id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("count should succeed");
    let inbox_before: i64 = sqlx::query_scalar("select count(*) from oms_inbox where run_id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("count should succeed");

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-snapshot.v1|no_outbox_inbox",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert should succeed");

    upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 40_000_000_000,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 0,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("accounting upsert should succeed");

    let outbox_after: i64 = sqlx::query_scalar("select count(*) from oms_outbox where run_id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("count should succeed");
    let inbox_after: i64 = sqlx::query_scalar("select count(*) from oms_inbox where run_id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("count should succeed");

    assert_eq!(outbox_before, outbox_after, "no outbox row must be written");
    assert_eq!(inbox_before, inbox_after, "no inbox row must be written");

    cleanup(&pool, &[run_id]).await;
}

// ---------------------------------------------------------------------------
// B4 final closure repair (A3/A4/A5): source-snapshot integrity and
// deterministic authority ordering.
// ---------------------------------------------------------------------------

/// A3: a `source_snapshot_id` that does not resolve to any durable snapshot
/// is refused before any accounting write.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn missing_source_snapshot_rejected() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("missing_source_snapshot_rejected");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let nonexistent_snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-store.v1|missing_source_snapshot_rejected.nonexistent",
    );

    let outcome = upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 0,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 0,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: nonexistent_snapshot_id,
        },
    )
    .await
    .expect("call should succeed at the Rust level (returns InvalidSourceSnapshot)");
    assert!(matches!(
        outcome,
        UpsertPaperPortfolioAccountingStateOutcome::InvalidSourceSnapshot { .. }
    ));

    let count: i64 = sqlx::query_scalar(
        "select count(*) from sys_paper_portfolio_accounting_state where run_id = $1",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("count query should succeed");
    assert_eq!(count, 0, "no accounting row must be written");

    cleanup(&pool, &[run_id]).await;
}

/// A3: a snapshot belonging to a *different* run than the accounting write's
/// own `run_id` is refused (cross-run source snapshot).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn cross_run_source_snapshot_rejected() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_a = fixed_run_id("cross_run_source_snapshot_rejected.a");
    let run_b = fixed_run_id("cross_run_source_snapshot_rejected.b");
    cleanup(&pool, &[run_a, run_b]).await;
    fixture_run(&pool, run_a).await;
    fixture_run(&pool, run_b).await;

    // Snapshot belongs to run_b.
    let snapshot_b = fixture_snapshot(&pool, run_b, "cross_run_source_snapshot_rejected").await;

    // Accounting write targets run_a, but supplies run_b's snapshot.
    let outcome = upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id: run_a,
            cash_micros: 0,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 0,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_b,
        },
    )
    .await
    .expect("call should succeed at the Rust level (returns InvalidSourceSnapshot)");
    assert!(matches!(
        outcome,
        UpsertPaperPortfolioAccountingStateOutcome::InvalidSourceSnapshot { .. }
    ));

    let count: i64 = sqlx::query_scalar(
        "select count(*) from sys_paper_portfolio_accounting_state where run_id = $1",
    )
    .bind(run_a)
    .fetch_one(&pool)
    .await
    .expect("count query should succeed");
    assert_eq!(count, 0, "no accounting row must be written for run_a");

    cleanup(&pool, &[run_a, run_b]).await;
}

/// A3: a snapshot with `run_id = NULL` can never authorize any accounting
/// write (a null-run source snapshot is a subset of cross-run: it can never
/// equal `Some(args.run_id)`).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn null_run_source_snapshot_rejected() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("null_run_source_snapshot_rejected");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-store.v1|null_run_source_snapshot_rejected.snapshot",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: None,
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert should succeed");

    let outcome = upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 0,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 0,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("call should succeed at the Rust level (returns InvalidSourceSnapshot)");
    assert!(matches!(
        outcome,
        UpsertPaperPortfolioAccountingStateOutcome::InvalidSourceSnapshot { .. }
    ));

    cleanup(&pool, &[run_id]).await;
    let _ = sqlx::query("delete from sys_paper_portfolio_snapshots where snapshot_id = $1")
        .bind(snapshot_id)
        .execute(&pool)
        .await;
}

/// A3: a non-`external_alpaca` source snapshot (synthetic/diagnostic) can
/// never authorize an accounting write, even when run/mode/currency match.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn non_alpaca_source_snapshot_rejected() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("non_alpaca_source_snapshot_rejected");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-store.v1|non_alpaca_source_snapshot_rejected.snapshot",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_SYNTHETIC_DIAGNOSTIC.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert should succeed");

    let outcome = upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 0,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 0,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("call should succeed at the Rust level (returns InvalidSourceSnapshot)");
    assert!(matches!(
        outcome,
        UpsertPaperPortfolioAccountingStateOutcome::InvalidSourceSnapshot { .. }
    ));

    cleanup(&pool, &[run_id]).await;
}

/// A3: a non-`"paper"` deployment_mode snapshot can never authorize an
/// accounting write.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn non_paper_deployment_mode_snapshot_rejected() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("non_paper_deployment_mode_snapshot_rejected");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-store.v1|non_paper_deployment_mode_snapshot_rejected.snapshot",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "live-shadow".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "USD".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert should succeed");

    let outcome = upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 0,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 0,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("call should succeed at the Rust level (returns InvalidSourceSnapshot)");
    assert!(matches!(
        outcome,
        UpsertPaperPortfolioAccountingStateOutcome::InvalidSourceSnapshot { .. }
    ));

    cleanup(&pool, &[run_id]).await;
}

/// A3: a non-USD currency snapshot can never authorize an accounting write.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn non_usd_currency_snapshot_rejected() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("non_usd_currency_snapshot_rejected");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"test.paper-portfolio-store.v1|non_usd_currency_snapshot_rejected.snapshot",
    );
    insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            deployment_mode: "paper".to_string(),
            source: PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros: 100_000_000_000,
            cash_micros: 40_000_000_000,
            currency: "EUR".to_string(),
            truth_state: "active".to_string(),
            run_id: Some(run_id),
            operation_id: None,
            positions: vec![],
        },
    )
    .await
    .expect("insert should succeed");

    let outcome = upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 0,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 0,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_id,
        },
    )
    .await
    .expect("call should succeed at the Rust level (returns InvalidSourceSnapshot)");
    assert!(matches!(
        outcome,
        UpsertPaperPortfolioAccountingStateOutcome::InvalidSourceSnapshot { .. }
    ));

    cleanup(&pool, &[run_id]).await;
}

/// A5: same snapshot, same fill-derived values, but the accounting_epoch
/// (or reason) drifted -- this must be a Conflict, never a silent update,
/// since an identical snapshot replayed against identical fill history must
/// always reproduce the identical epoch classification.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn same_snapshot_epoch_drift_is_conflict() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("same_snapshot_epoch_drift_is_conflict");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;
    let snapshot_id = fixture_snapshot(&pool, run_id, "same_snapshot_epoch_drift_is_conflict").await;

    let base_args = |epoch: &str, reason: Option<String>| UpsertPaperPortfolioAccountingStateArgs {
        run_id,
        cash_micros: 40_000_000_000,
        realized_pnl_micros: 0,
        fees_micros: 0,
        last_applied_inbox_id: 5,
        accounting_epoch: epoch.to_string(),
        accounting_epoch_reason: reason,
        updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
        source_snapshot_id: snapshot_id,
    };

    upsert_paper_portfolio_accounting_state(
        &pool,
        base_args(PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE, None),
    )
    .await
    .expect("insert should succeed");

    // Same snapshot, same watermark, same fill-derived cash/pnl/fees, but a
    // DIFFERENT epoch: must be Conflict, not UpdatedForSnapshot.
    let epoch_drift = upsert_paper_portfolio_accounting_state(
        &pool,
        base_args(PAPER_PORTFOLIO_ACCOUNTING_EPOCH_INCOMPLETE, None),
    )
    .await
    .expect("epoch-drift call should succeed at the Rust level (returns Conflict)");
    match epoch_drift {
        UpsertPaperPortfolioAccountingStateOutcome::Conflict { record, .. } => {
            assert_eq!(
                record.accounting_epoch, PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE,
                "Conflict must return the existing stored epoch, never the caller's differing input"
            );
        }
        other => panic!("expected Conflict, got {other:?}"),
    }

    // Same snapshot, same epoch, but a DIFFERENT reason: also Conflict.
    let reason_drift = upsert_paper_portfolio_accounting_state(
        &pool,
        base_args(
            PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE,
            Some("unexpected_reason".to_string()),
        ),
    )
    .await
    .expect("reason-drift call should succeed at the Rust level (returns Conflict)");
    assert!(matches!(
        reason_drift,
        UpsertPaperPortfolioAccountingStateOutcome::Conflict { .. }
    ));

    let fetched = fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("row should exist");
    assert_eq!(
        fetched.accounting_epoch, PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE,
        "an epoch/reason Conflict must never mutate the stored row"
    );

    cleanup(&pool, &[run_id]).await;
}

/// A5: an older snapshot (by `(captured_at_utc, snapshot_id)` authority
/// order) can never replace a newer one at the same watermark, even with
/// identical fill-derived values.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn older_snapshot_cannot_replace_newer_provenance() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("older_snapshot_cannot_replace_newer_provenance");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_older = fixture_snapshot_at(
        &pool,
        run_id,
        "older_snapshot_cannot_replace_newer_provenance.older",
        Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
    )
    .await;
    let snapshot_newer = fixture_snapshot_at(
        &pool,
        run_id,
        "older_snapshot_cannot_replace_newer_provenance.newer",
        Utc.with_ymd_and_hms(2099, 2, 1, 13, 5, 0).unwrap(),
    )
    .await;

    let base_args = |snapshot_id: Uuid| UpsertPaperPortfolioAccountingStateArgs {
        run_id,
        cash_micros: 40_000_000_000,
        realized_pnl_micros: 0,
        fees_micros: 0,
        last_applied_inbox_id: 5,
        accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
        accounting_epoch_reason: None,
        updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
        source_snapshot_id: snapshot_id,
    };

    // Establish the newer snapshot as the recorded provenance first.
    upsert_paper_portfolio_accounting_state(&pool, base_args(snapshot_newer))
        .await
        .expect("insert should succeed");

    // Attempting to move to the OLDER snapshot at the same watermark must be
    // rejected -- provenance must never move backward.
    let outcome = upsert_paper_portfolio_accounting_state(&pool, base_args(snapshot_older))
        .await
        .expect("call should succeed at the Rust level (returns RejectedStaleSnapshot)");
    assert!(matches!(
        outcome,
        UpsertPaperPortfolioAccountingStateOutcome::RejectedStaleSnapshot { .. }
    ));

    let fetched = fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("row should exist");
    assert_eq!(
        fetched.source_snapshot_id,
        Some(snapshot_newer),
        "a stale-snapshot rejection must never regress recorded provenance"
    );

    cleanup(&pool, &[run_id]).await;
}

/// A4: two snapshots with the EXACT same `captured_at_utc` are ordered
/// deterministically by `snapshot_id` -- the lower-UUID snapshot can never
/// supersede the higher-UUID one at the same instant.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn equal_captured_at_utc_uses_deterministic_snapshot_id_ordering() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("equal_captured_at_utc_uses_deterministic_snapshot_id_ordering");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let same_instant = Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap();
    let snapshot_x = fixture_snapshot_at(
        &pool,
        run_id,
        "equal_captured_at_utc_uses_deterministic_snapshot_id_ordering.x",
        same_instant,
    )
    .await;
    let snapshot_y = fixture_snapshot_at(
        &pool,
        run_id,
        "equal_captured_at_utc_uses_deterministic_snapshot_id_ordering.y",
        same_instant,
    )
    .await;

    // Deterministically identify the lower and higher UUID of the pair --
    // this test must pass regardless of which named seed happens to hash
    // to the greater UUID.
    let (lower, higher) = if snapshot_x < snapshot_y {
        (snapshot_x, snapshot_y)
    } else {
        (snapshot_y, snapshot_x)
    };

    let base_args = |snapshot_id: Uuid| UpsertPaperPortfolioAccountingStateArgs {
        run_id,
        cash_micros: 40_000_000_000,
        realized_pnl_micros: 0,
        fees_micros: 0,
        last_applied_inbox_id: 5,
        accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
        accounting_epoch_reason: None,
        updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
        source_snapshot_id: snapshot_id,
    };

    // Record the higher-UUID snapshot as current provenance first.
    upsert_paper_portfolio_accounting_state(&pool, base_args(higher))
        .await
        .expect("insert should succeed");

    // The lower-UUID snapshot, at the identical captured_at_utc, must never
    // be treated as newer.
    let outcome = upsert_paper_portfolio_accounting_state(&pool, base_args(lower))
        .await
        .expect("call should succeed at the Rust level (returns RejectedStaleSnapshot)");
    assert!(matches!(
        outcome,
        UpsertPaperPortfolioAccountingStateOutcome::RejectedStaleSnapshot { .. }
    ));

    cleanup(&pool, &[run_id]).await;
}

/// A5: a higher inbox watermark must never regress source-snapshot
/// provenance, even though the watermark itself is a valid advance.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn higher_watermark_cannot_regress_snapshot_provenance() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("higher_watermark_cannot_regress_snapshot_provenance");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_older = fixture_snapshot_at(
        &pool,
        run_id,
        "higher_watermark_cannot_regress_snapshot_provenance.older",
        Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
    )
    .await;
    let snapshot_newer = fixture_snapshot_at(
        &pool,
        run_id,
        "higher_watermark_cannot_regress_snapshot_provenance.newer",
        Utc.with_ymd_and_hms(2099, 2, 1, 13, 5, 0).unwrap(),
    )
    .await;

    upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 40_000_000_000,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 5,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_newer,
        },
    )
    .await
    .expect("insert should succeed");

    // Watermark advances (9 > 5), but the supplied snapshot is OLDER than
    // the one already recorded -- must be rejected despite the higher
    // watermark.
    let outcome = upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 41_000_000_000,
            realized_pnl_micros: 1_000_000,
            fees_micros: 0,
            last_applied_inbox_id: 9,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 10, 0).unwrap(),
            source_snapshot_id: snapshot_older,
        },
    )
    .await
    .expect("call should succeed at the Rust level (returns RejectedStaleSnapshot)");
    assert!(matches!(
        outcome,
        UpsertPaperPortfolioAccountingStateOutcome::RejectedStaleSnapshot { .. }
    ));

    let fetched = fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("row should exist");
    assert_eq!(
        fetched.last_applied_inbox_id, 5,
        "a rejected stale-snapshot attempt must never advance the watermark either"
    );
    assert_eq!(fetched.source_snapshot_id, Some(snapshot_newer));

    cleanup(&pool, &[run_id]).await;
}

/// A5: a strictly newer snapshot at the same watermark, with unchanged
/// fill-derived values, is accepted as `UpdatedForSnapshot` -- confirming
/// the ordering gate does not block the legitimate case (complements
/// `accounting_state_watermark_idempotency_and_ordering`'s coverage with an
/// explicit standalone proof).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored"]
async fn newer_snapshot_updates_completeness_with_unchanged_fills() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let run_id = fixed_run_id("newer_snapshot_updates_completeness_with_unchanged_fills");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let snapshot_a = fixture_snapshot_at(
        &pool,
        run_id,
        "newer_snapshot_updates_completeness_with_unchanged_fills.a",
        Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
    )
    .await;
    let snapshot_b = fixture_snapshot_at(
        &pool,
        run_id,
        "newer_snapshot_updates_completeness_with_unchanged_fills.b",
        Utc.with_ymd_and_hms(2099, 2, 1, 13, 5, 0).unwrap(),
    )
    .await;

    upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 40_000_000_000,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 5,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_INCOMPLETE.to_string(),
            accounting_epoch_reason: Some("position_quantity_mismatch:AAPL".to_string()),
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 0, 0).unwrap(),
            source_snapshot_id: snapshot_a,
        },
    )
    .await
    .expect("insert should succeed");

    let outcome = upsert_paper_portfolio_accounting_state(
        &pool,
        UpsertPaperPortfolioAccountingStateArgs {
            run_id,
            cash_micros: 40_000_000_000,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 5,
            accounting_epoch: PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 13, 5, 0).unwrap(),
            source_snapshot_id: snapshot_b,
        },
    )
    .await
    .expect("updated-for-snapshot call should succeed");
    match outcome {
        UpsertPaperPortfolioAccountingStateOutcome::UpdatedForSnapshot { record } => {
            assert_eq!(record.source_snapshot_id, Some(snapshot_b));
            assert_eq!(record.accounting_epoch, PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE);
            assert_eq!(record.cash_micros, 40_000_000_000, "fill-derived cash must be unchanged");
        }
        other => panic!("expected UpdatedForSnapshot, got {other:?}"),
    }

    cleanup(&pool, &[run_id]).await;
}
