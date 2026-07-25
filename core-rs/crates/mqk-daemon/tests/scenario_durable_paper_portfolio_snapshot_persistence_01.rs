//! DURABLE-PAPER-PORTFOLIO-AND-PNL-01C: authoritative snapshot persistence
//! and restart reconstruction proof.
//!
//! Exercises the real production acceptance seam
//! (`AppState::accept_external_broker_snapshot_for_test`, a thin pass-through
//! to the same `state::snapshot::accept_external_broker_snapshot` the
//! run-start cold-fetch and periodic-refresh call sites use) with injected
//! `BrokerSnapshot` fixtures only. No real Alpaca/broker/provider/network
//! call is made anywhere in this file.
//!
//! All DB-backed tests skip gracefully without `MQK_DATABASE_URL` pointing
//! at the isolated test DB.

use chrono::{TimeZone, Utc};
use mqk_daemon::state::{AppState, BrokerKind};
use mqk_schemas::{BrokerAccount, BrokerPosition, BrokerSnapshot};
use uuid::Uuid;

fn fixture_snapshot(
    captured_at_utc: chrono::DateTime<Utc>,
    equity: &str,
    cash: &str,
) -> BrokerSnapshot {
    BrokerSnapshot {
        captured_at_utc,
        account: BrokerAccount {
            equity: equity.to_string(),
            cash: cash.to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![],
        fills: vec![],
        positions: vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "10".to_string(),
            avg_price: "150.00".to_string(),
        }],
    }
}

async fn test_pool() -> Option<sqlx::PgPool> {
    if std::env::var("MQK_DATABASE_URL").is_err() {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return None;
    }
    match mqk_db::testkit_db_pool().await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn fixed_run_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.durable-paper-portfolio-snapshot-persistence.v1|{seed}").as_bytes(),
    )
}

/// Mirrors `persist_external_broker_snapshot_best_effort`'s deterministic
/// snapshot_id derivation exactly, so tests can look a specific snapshot up
/// directly instead of relying on `fetch_latest_paper_portfolio_snapshot`
/// (which is global across *all* runs sharing deployment_mode+source, and
/// therefore inherently racy under cargo test's default parallelism when
/// multiple tests in this file persist concurrently -- see B4-0 for the
/// same class of fix applied to the completed-bar-driver scenario).
fn expected_snapshot_id(captured_at_utc: chrono::DateTime<Utc>, run_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "mqk.paper-portfolio-snapshot.v1|{}|{}|{}",
            captured_at_utc.to_rfc3339(),
            run_id,
            mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        )
        .as_bytes(),
    )
}

async fn fixture_run(pool: &sqlx::PgPool, run_id: Uuid) {
    let _ = sqlx::query("delete from sys_paper_portfolio_snapshots where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "test-durable-snapshot-persistence".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("fixture run insert should succeed");
}

async fn cleanup(pool: &sqlx::PgPool, run_id: Uuid) {
    // DURABLE-PAPER-PORTFOLIO-AND-PNL-01D: accept_external_broker_snapshot
    // (the seam every test in this file exercises) now also refreshes
    // sys_paper_portfolio_accounting_state as a side effect of persisting a
    // snapshot -- this row must be deleted before `runs`, or the `runs`
    // delete below fails on its REFERENCES constraint (silently, since it's
    // wrapped in `let _ =`), leaking a stale run_id row that collides with
    // this same deterministic run_id on the next test invocation.
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

/// A real-source (Paper+Alpaca) accepted snapshot persists durably, and the
/// in-memory cache is also updated (unchanged existing behavior).
#[tokio::test]
async fn real_source_accepted_snapshot_persists() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("real_source_accepted_snapshot_persists");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state.db = Some(pool.clone());

    let captured_at_utc = Utc.with_ymd_and_hms(2099, 3, 1, 13, 0, 0).unwrap();
    let snapshot = fixture_snapshot(captured_at_utc, "100000.00", "40000.00");
    state
        .accept_external_broker_snapshot_for_test(snapshot.clone(), Some(run_id), None)
        .await;

    // In-memory cache updated (existing behavior, unchanged).
    let cached = state.broker_snapshot.read().await.clone();
    assert!(
        cached.is_some(),
        "in-memory broker_snapshot must still be set"
    );

    // Durably persisted as authoritative Paper+Alpaca truth. Looked up by
    // its exact deterministic identity rather than "latest" (see
    // expected_snapshot_id's doc comment for why).
    let durable = mqk_db::fetch_paper_portfolio_snapshot_by_id(
        &pool,
        expected_snapshot_id(captured_at_utc, run_id),
    )
    .await
    .expect("fetch should succeed")
    .expect("durable snapshot for this run_id should exist");
    assert_eq!(durable.snapshot.cash_micros, 40_000_000_000);
    assert_eq!(durable.positions.len(), 1);
    assert_eq!(durable.positions[0].symbol, "AAPL");
    assert_eq!(durable.positions[0].qty_signed, 10);

    cleanup(&pool, run_id).await;
}

/// A synthesized (Paper broker, non-Alpaca) source is never persisted as
/// authoritative Paper+Alpaca truth by the same seam, even though the
/// in-memory cache is still updated. This is the source-authority gate
/// enforced by `persist_external_broker_snapshot_best_effort` itself, not
/// just documentation.
#[tokio::test]
async fn synthesized_source_never_masquerades_as_authoritative() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("synthesized_source_never_masquerades_as_authoritative");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    // BrokerKind::Paper -> Synthetic source, not eligible for durable
    // authoritative persistence regardless of what is passed to the seam.
    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Paper);
    state.db = Some(pool.clone());

    let snapshot = fixture_snapshot(
        Utc.with_ymd_and_hms(2099, 3, 1, 13, 0, 0).unwrap(),
        "100000.00",
        "40000.00",
    );
    state
        .accept_external_broker_snapshot_for_test(snapshot, Some(run_id), None)
        .await;

    // In-memory cache is still updated -- the seam does exactly what it
    // says (accept), the persistence half is what's gated.
    assert!(state.broker_snapshot.read().await.is_some());

    let count: i64 =
        sqlx::query_scalar("select count(*) from sys_paper_portfolio_snapshots where run_id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("count query should succeed");
    assert_eq!(
        count, 0,
        "a Paper-broker (synthetic) source must never produce a durable authoritative row"
    );

    cleanup(&pool, run_id).await;
}

/// Missing DB (AppState.db == None) is bounded: the in-memory acceptance
/// still succeeds, no panic occurs, and there is nothing to durably persist.
#[tokio::test]
async fn missing_db_remains_bounded() {
    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state.db = None;

    let snapshot = fixture_snapshot(
        Utc.with_ymd_and_hms(2099, 3, 1, 13, 0, 0).unwrap(),
        "100000.00",
        "40000.00",
    );
    state
        .accept_external_broker_snapshot_for_test(snapshot, None, None)
        .await;

    assert!(
        state.broker_snapshot.read().await.is_some(),
        "in-memory acceptance must still succeed even with no DB configured"
    );
}

/// A DB that is configured but unreachable at query time is bounded the
/// same way: no panic, in-memory acceptance still succeeds, and the caller
/// is never blocked by the failed persistence attempt.
#[tokio::test]
async fn db_failure_remains_bounded() {
    // A lazily-connecting pool against an unreachable address: connect()
    // itself never blocks (lazy), but any query against it will fail --
    // this is a self-contained "unreachable DB" fixture that never touches
    // the real isolated test pool (so it cannot affect other tests running
    // in this binary).
    let unreachable_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/mqk_test?sslmode=disable")
        .expect("lazy connect should not fail synchronously");

    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state.db = Some(unreachable_pool);

    let snapshot = fixture_snapshot(
        Utc.with_ymd_and_hms(2099, 3, 1, 13, 0, 0).unwrap(),
        "100000.00",
        "40000.00",
    );
    state
        .accept_external_broker_snapshot_for_test(snapshot, None, None)
        .await;

    assert!(
        state.broker_snapshot.read().await.is_some(),
        "in-memory acceptance must still succeed even when durable persistence fails"
    );
}

/// Re-accepting the exact same snapshot (same captured_at_utc/run_id) is an
/// idempotent replay: zero duplicate rows.
#[tokio::test]
async fn exact_replay_is_idempotent() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("exact_replay_is_idempotent");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state.db = Some(pool.clone());

    let captured_at_utc = Utc.with_ymd_and_hms(2099, 3, 1, 13, 0, 0).unwrap();
    for _ in 0..2 {
        let snapshot = fixture_snapshot(captured_at_utc, "100000.00", "40000.00");
        state
            .accept_external_broker_snapshot_for_test(snapshot, Some(run_id), None)
            .await;
    }

    let count: i64 =
        sqlx::query_scalar("select count(*) from sys_paper_portfolio_snapshots where run_id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("count query should succeed");
    assert_eq!(
        count, 1,
        "re-accepting the identical snapshot must not duplicate the row"
    );

    cleanup(&pool, run_id).await;
}

/// Restart reconstruction: a fresh `AppState` (simulating a new process,
/// with an empty in-memory cache) reads back the durable snapshot a prior
/// `AppState` persisted, via a completely separate connection pool.
#[tokio::test]
async fn restart_reads_back_durable_snapshot() {
    let Some(pool_before) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("restart_reads_back_durable_snapshot");
    cleanup(&pool_before, run_id).await;
    fixture_run(&pool_before, run_id).await;

    let mut state_before = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state_before.db = Some(pool_before.clone());
    let captured_at_utc = Utc.with_ymd_and_hms(2099, 3, 1, 13, 0, 0).unwrap();
    let snapshot = fixture_snapshot(captured_at_utc, "100000.00", "40000.00");
    state_before
        .accept_external_broker_snapshot_for_test(snapshot, Some(run_id), None)
        .await;

    drop(state_before);
    drop(pool_before);

    // A brand new AppState + brand new pool stand in for a fresh process.
    let Some(pool_after) = test_pool().await else {
        return;
    };
    let state_after = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    assert!(
        state_after.broker_snapshot.read().await.is_none(),
        "a fresh AppState must start with no in-memory broker_snapshot"
    );

    let durable = mqk_db::fetch_paper_portfolio_snapshot_by_id(
        &pool_after,
        expected_snapshot_id(captured_at_utc, run_id),
    )
    .await
    .expect("fetch should succeed")
    .expect("durable snapshot must survive across AppState/process replacement");
    assert_eq!(durable.snapshot.cash_micros, 40_000_000_000);
    assert_eq!(durable.positions.len(), 1);

    cleanup(&pool_after, run_id).await;
}

/// Position rows round-trip in the exact order they were supplied
/// (fetch_paper_portfolio_snapshot_by_id orders by symbol -- proven here
/// with a multi-position snapshot).
#[tokio::test]
async fn position_ordering_round_trips() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("position_ordering_round_trips");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state.db = Some(pool.clone());

    let captured_at_utc = Utc.with_ymd_and_hms(2099, 3, 1, 13, 0, 0).unwrap();
    let mut snapshot = fixture_snapshot(captured_at_utc, "100000.00", "40000.00");
    snapshot.positions = vec![
        BrokerPosition {
            symbol: "MSFT".to_string(),
            qty: "5".to_string(),
            avg_price: "300.00".to_string(),
        },
        BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "10".to_string(),
            avg_price: "150.00".to_string(),
        },
    ];
    state
        .accept_external_broker_snapshot_for_test(snapshot, Some(run_id), None)
        .await;

    let durable = mqk_db::fetch_paper_portfolio_snapshot_by_id(
        &pool,
        expected_snapshot_id(captured_at_utc, run_id),
    )
    .await
    .expect("fetch should succeed")
    .expect("durable snapshot should exist");
    let symbols: Vec<&str> = durable
        .positions
        .iter()
        .map(|p| p.symbol.as_str())
        .collect();
    assert_eq!(
        symbols,
        vec!["AAPL", "MSFT"],
        "positions must be returned in deterministic symbol order"
    );

    cleanup(&pool, run_id).await;
}

/// Multiple distinct snapshots for the same run form a durable history: each
/// is independently readable by its own deterministic identity, and none
/// overwrites another.
///
/// (`fetch_latest_paper_portfolio_snapshot` itself is intentionally *not*
/// exercised here for the "is it the newest" check: that helper reports the
/// single latest row across *all* runs sharing deployment_mode+source, which
/// under cargo test's default parallelism can legitimately belong to a
/// different, concurrently-running test in this same file. Fetching each of
/// this test's own three rows by their exact deterministic snapshot_id
/// proves the same history/non-overwrite property without that race.)
#[tokio::test]
async fn snapshot_replacement_forms_durable_history() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("snapshot_replacement_forms_durable_history");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state.db = Some(pool.clone());

    let mut captured_timestamps = Vec::new();
    for i in 0..3i64 {
        let captured_at_utc =
            Utc.with_ymd_and_hms(2099, 3, 1, 13, 0, 0).unwrap() + chrono::Duration::minutes(i);
        captured_timestamps.push(captured_at_utc);
        let snapshot =
            fixture_snapshot(captured_at_utc, &format!("{}.00", 100_000 + i), "40000.00");
        state
            .accept_external_broker_snapshot_for_test(snapshot, Some(run_id), None)
            .await;
    }

    let count: i64 =
        sqlx::query_scalar("select count(*) from sys_paper_portfolio_snapshots where run_id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("count query should succeed");
    assert_eq!(
        count, 3,
        "three distinct captures must produce three durable rows"
    );

    for (i, captured_at_utc) in captured_timestamps.iter().enumerate() {
        let row = mqk_db::fetch_paper_portfolio_snapshot_by_id(
            &pool,
            expected_snapshot_id(*captured_at_utc, run_id),
        )
        .await
        .expect("fetch should succeed")
        .unwrap_or_else(|| panic!("snapshot {i} of the history should exist"));
        assert_eq!(
            row.snapshot.equity_micros,
            (100_000 + i as i64) * 1_000_000,
            "each historical snapshot must retain its own captured value, not the latest one's"
        );
    }

    cleanup(&pool, run_id).await;
}

/// Zero OMS outbox/inbox writes and zero broker/provider calls: this seam
/// only ever touches the durable paper-portfolio-snapshot tables (plus the
/// fixture `runs` row this test manages itself), and only ever consumes an
/// already-constructed `BrokerSnapshot` -- it never calls a provider or
/// broker itself.
#[tokio::test]
async fn zero_oms_delta_and_zero_broker_calls() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("zero_oms_delta_and_zero_broker_calls");
    cleanup(&pool, run_id).await;
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

    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state.db = Some(pool.clone());
    let snapshot = fixture_snapshot(
        Utc.with_ymd_and_hms(2099, 3, 1, 13, 0, 0).unwrap(),
        "100000.00",
        "40000.00",
    );
    // This function signature takes an already-constructed BrokerSnapshot --
    // there is no provider/broker handle anywhere in this test file, so a
    // real network/broker call is structurally impossible here.
    state
        .accept_external_broker_snapshot_for_test(snapshot, Some(run_id), None)
        .await;

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

    cleanup(&pool, run_id).await;
}
