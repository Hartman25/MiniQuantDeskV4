//! `sys_strategy_held_sizing_state` (migration 0091) store proof.
//!
//! Proves: idempotent retry, generation ordering (no skip / stale replay / entry
//! over an active entry), safe release and re-entry, atomic batches, scope
//! isolation, and the schema CHECK constraints.
//!
//! All tests require `MQK_DATABASE_URL` (a disposable test database) and are
//! `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/<disposable> \
//!   cargo test -p mqk-db --test scenario_held_sizing_state_01 -- --include-ignored --test-threads=1

use chrono::{DateTime, TimeZone, Utc};
use mqk_db::held_sizing_state::{
    held_sizing_apply, held_sizing_fetch, HeldSizingRow, HeldSizingTransitionRow,
    HELD_SIZING_STATUS_ACTIVE, HELD_SIZING_STATUS_RELEASED,
};
use mqk_db::ENV_DB_URL;

const USD: i64 = 1_000_000;

type RowMutation = Box<dyn Fn(&mut HeldSizingRow)>;

async fn pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var(ENV_DB_URL).is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    mqk_db::testkit_db_pool().await
}

/// Run-unique deployment id: the test database persists between runs.
fn d(name: &str) -> String {
    use std::sync::OnceLock;
    static RUN: OnceLock<u128> = OnceLock::new();
    let run = RUN.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|t| t.as_nanos())
            .unwrap_or(0)
    });
    format!("{name}-{run}")
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 3, 14, 0, 0).unwrap()
}

/// Unique deployment per test so tests never share rows.
fn row(dep: &str, symbol: &str, generation: i64, qty: i64) -> HeldSizingRow {
    HeldSizingRow {
        deployment_id: dep.to_string(),
        strategy_id: "swing_momentum".to_string(),
        symbol: symbol.to_string(),
        state_version: 1,
        entry_generation: generation,
        status: HELD_SIZING_STATUS_ACTIVE.to_string(),
        policy_id: "fixed_initial_capital_fraction_v1".to_string(),
        allocation_fraction_bps: 1_000,
        initial_allocated_capital_micros: 100_000 * USD,
        max_target_qty_micros: None,
        max_notional_usd: None,
        resolved_target_qty_micros: qty * USD,
        reference_bar_end_ts: 1_000 + generation,
        reference_price_micros: 125 * USD,
    }
}

fn released(r: &HeldSizingRow) -> HeldSizingRow {
    HeldSizingRow {
        status: HELD_SIZING_STATUS_RELEASED.to_string(),
        ..r.clone()
    }
}

fn entered(r: HeldSizingRow) -> HeldSizingTransitionRow {
    HeldSizingTransitionRow::Entered(r)
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn entry_roundtrips_and_a_retry_is_an_idempotent_noop() -> anyhow::Result<()> {
    let pool = pool().await?;
    let r = row(&d("dep-roundtrip"), "SPY", 1, 80);
    let first = held_sizing_apply(&pool, &[entered(r.clone())], now()).await?;
    assert_eq!((first.applied, first.already_applied), (1, 0));
    let retry = held_sizing_apply(&pool, &[entered(r.clone())], now()).await?;
    assert_eq!((retry.applied, retry.already_applied), (0, 1));
    let rows = held_sizing_fetch(&pool, &d("dep-roundtrip"), "swing_momentum").await?;
    assert_eq!(
        rows,
        vec![r],
        "exactly one row, byte-identical after the retry"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn conflicting_skipping_stale_and_overwriting_entries_are_refused() -> anyhow::Result<()> {
    let pool = pool().await?;
    let r = row(&d("dep-refuse"), "SPY", 1, 80);
    held_sizing_apply(&pool, &[entered(r.clone())], now()).await?;
    let attempts: Vec<(&str, HeldSizingTransitionRow)> = vec![
        (
            "same generation, different Q",
            entered(row(&d("dep-refuse"), "SPY", 1, 81)),
        ),
        (
            "entry over an ACTIVE entry (next generation)",
            entered(row(&d("dep-refuse"), "SPY", 2, 40)),
        ),
        (
            "generation skips ahead",
            entered(row(&d("dep-refuse"), "SPY", 3, 40)),
        ),
        (
            "first entry without generation 1",
            entered(row(&d("dep-refuse"), "QQQ", 2, 40)),
        ),
        (
            "entry carrying released status",
            entered(released(&row(&d("dep-refuse"), "QQQ", 1, 40))),
        ),
        (
            "release with no stored entry",
            HeldSizingTransitionRow::Released(released(&row(&d("dep-refuse"), "IWM", 1, 40))),
        ),
        (
            "release of a different Q",
            HeldSizingTransitionRow::Released(released(&row(&d("dep-refuse"), "SPY", 1, 79))),
        ),
        (
            "release carrying active status",
            HeldSizingTransitionRow::Released(r.clone()),
        ),
    ];
    for (name, t) in attempts {
        assert!(
            held_sizing_apply(&pool, &[t], now()).await.is_err(),
            "{name}"
        );
    }
    let rows = held_sizing_fetch(&pool, &d("dep-refuse"), "swing_momentum").await?;
    assert_eq!(
        rows,
        vec![r],
        "no refused transition changed the stored state"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn release_then_reentry_advances_the_generation_and_old_transitions_cannot_replay(
) -> anyhow::Result<()> {
    let pool = pool().await?;
    let g1 = row(&d("dep-cycle"), "SPY", 1, 80);
    held_sizing_apply(&pool, &[entered(g1.clone())], now()).await?;
    let rel = HeldSizingTransitionRow::Released(released(&g1));
    let r = held_sizing_apply(&pool, std::slice::from_ref(&rel), now()).await?;
    assert_eq!(r.applied, 1);
    let r = held_sizing_apply(&pool, std::slice::from_ref(&rel), now()).await?;
    assert_eq!(
        (r.applied, r.already_applied),
        (0, 1),
        "release retry is a no-op"
    );
    let stored = held_sizing_fetch(&pool, &d("dep-cycle"), "swing_momentum").await?;
    assert_eq!(stored, vec![released(&g1)]);

    let g2 = row(&d("dep-cycle"), "SPY", 2, 40);
    held_sizing_apply(&pool, &[entered(g2.clone())], now()).await?;
    assert_eq!(
        held_sizing_fetch(&pool, &d("dep-cycle"), "swing_momentum").await?,
        vec![g2.clone()]
    );
    // Replaying the generation-1 entry/release after generation 2 is stale.
    assert!(held_sizing_apply(&pool, &[entered(g1.clone())], now())
        .await
        .is_err());
    assert!(held_sizing_apply(&pool, &[rel], now()).await.is_err());
    assert_eq!(
        held_sizing_fetch(&pool, &d("dep-cycle"), "swing_momentum").await?,
        vec![g2]
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn a_batch_is_atomic_and_scopes_are_isolated() -> anyhow::Result<()> {
    let pool = pool().await?;
    // Second transition is invalid: the first must roll back with it.
    let bad_batch = [
        entered(row(&d("dep-atomic"), "SPY", 1, 80)),
        entered(row(&d("dep-atomic"), "QQQ", 5, 10)),
    ];
    assert!(held_sizing_apply(&pool, &bad_batch, now()).await.is_err());
    assert!(held_sizing_fetch(&pool, &d("dep-atomic"), "swing_momentum")
        .await?
        .is_empty());

    held_sizing_apply(
        &pool,
        &[
            entered(row(&d("dep-iso-a"), "SPY", 1, 80)),
            entered(row(&d("dep-iso-b"), "SPY", 1, 25)),
        ],
        now(),
    )
    .await?;
    let a = held_sizing_fetch(&pool, &d("dep-iso-a"), "swing_momentum").await?;
    let b = held_sizing_fetch(&pool, &d("dep-iso-b"), "swing_momentum").await?;
    assert_eq!((a.len(), a[0].resolved_target_qty_micros), (1, 80 * USD));
    assert_eq!((b.len(), b[0].resolved_target_qty_micros), (1, 25 * USD));
    assert!(held_sizing_fetch(&pool, &d("dep-iso-a"), "mean_reversion")
        .await?
        .is_empty());
    Ok(())
}

async fn raw_insert(
    pool: &sqlx::PgPool,
    r: &HeldSizingRow,
    released_at: Option<DateTime<Utc>>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into sys_strategy_held_sizing_state (deployment_id, strategy_id, symbol,          state_version, entry_generation, status, policy_id, allocation_fraction_bps,          initial_allocated_capital_micros, max_target_qty_micros, max_notional_usd,          resolved_target_qty_micros, reference_bar_end_ts, reference_price_micros,          recorded_at_utc, released_at_utc)          values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
    )
    .bind(&r.deployment_id)
    .bind(&r.strategy_id)
    .bind(&r.symbol)
    .bind(r.state_version)
    .bind(r.entry_generation)
    .bind(&r.status)
    .bind(&r.policy_id)
    .bind(r.allocation_fraction_bps)
    .bind(r.initial_allocated_capital_micros)
    .bind(r.max_target_qty_micros)
    .bind(r.max_notional_usd)
    .bind(r.resolved_target_qty_micros)
    .bind(r.reference_bar_end_ts)
    .bind(r.reference_price_micros)
    .bind(now())
    .bind(released_at)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The CHECK constraints themselves (raw SQL, bypassing the store's own
/// validation) refuse every malformed shape; the valid control is accepted.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn schema_constraints_reject_malformed_rows() -> anyhow::Result<()> {
    let pool = pool().await?;
    raw_insert(&pool, &row(&d("dep-schema-control"), "SPY", 1, 80), None).await?;
    let bad: Vec<(&str, RowMutation)> = vec![
        (
            "fractional quantity",
            Box::new(|r| r.resolved_target_qty_micros += 1),
        ),
        (
            "zero quantity",
            Box::new(|r| r.resolved_target_qty_micros = 0),
        ),
        ("bogus status", Box::new(|r| r.status = "pending".into())),
        (
            "wrong policy",
            Box::new(|r| r.policy_id = "fixed_quantity_v1".into()),
        ),
        ("fraction 0", Box::new(|r| r.allocation_fraction_bps = 0)),
        (
            "fraction 10001",
            Box::new(|r| r.allocation_fraction_bps = 10_001),
        ),
        (
            "capital 0",
            Box::new(|r| r.initial_allocated_capital_micros = 0),
        ),
        ("padded symbol", Box::new(|r| r.symbol = " SPY".into())),
        (
            "blank deployment",
            Box::new(|r| r.deployment_id = " ".into()),
        ),
        ("zero price", Box::new(|r| r.reference_price_micros = 0)),
        ("zero bar ts", Box::new(|r| r.reference_bar_end_ts = 0)),
        ("state version 2", Box::new(|r| r.state_version = 2)),
        ("generation 0", Box::new(|r| r.entry_generation = 0)),
        ("zero cap", Box::new(|r| r.max_target_qty_micros = Some(0))),
        (
            "zero notional cap",
            Box::new(|r| r.max_notional_usd = Some(0)),
        ),
    ];
    for (i, (name, mutate)) in bad.into_iter().enumerate() {
        let mut r = row(&d(&format!("dep-schema-{i}")), "SPY", 1, 80);
        mutate(&mut r);
        if r.deployment_id.trim().is_empty() {
            r.deployment_id = " ".into();
        }
        assert!(raw_insert(&pool, &r, None).await.is_err(), "{name}");
    }
    // release shape: released_at iff released
    let active_with_release = row(&d("dep-schema-shape-a"), "SPY", 1, 80);
    assert!(raw_insert(&pool, &active_with_release, Some(now()))
        .await
        .is_err());
    let released_without = released(&row(&d("dep-schema-shape-b"), "SPY", 1, 80));
    assert!(raw_insert(&pool, &released_without, None).await.is_err());
    raw_insert(
        &pool,
        &released(&row(&d("dep-schema-shape-c"), "SPY", 1, 80)),
        Some(now()),
    )
    .await?;
    // primary key: one row per (deployment, strategy, symbol)
    assert!(
        raw_insert(&pool, &row(&d("dep-schema-control"), "SPY", 2, 40), None)
            .await
            .is_err()
    );
    Ok(())
}
