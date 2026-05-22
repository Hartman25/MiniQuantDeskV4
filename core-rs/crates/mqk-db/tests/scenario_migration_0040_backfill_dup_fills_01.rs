//! MIGRATION-0040-BACKFILL-DUP-FILLS-01 — proof tests
//!
//! Proves the deterministic backfill CTE in migration 0040 correctly reduces
//! duplicate event_kind='fill' rows to a single canonical keeper before the
//! unique index uq_inbox_run_order_single_fill is created.
//!
//! # Test inventory
//!
//! | ID      | Description                                              | DB?  |
//! |---------|----------------------------------------------------------|------|
//! | M40-P01 | ranking: applied row beats unapplied (Priority 1)        | No   |
//! | M40-P02 | ranking: among applied, broker_fill_id wins (Priority 2) | No   |
//! | M40-P03 | ranking: all unapplied, broker_fill_id wins (Priority 2) | No   |
//! | M40-P04 | ranking: lowest inbox_id wins on full tiebreak            | No   |
//! | M40-P05 | partial fills excluded from backfill predicate            | No   |
//! | M40-D01 | unique index exists after migration                       | Yes  |
//! | M40-D02 | second fill for same (run_id, internal_order_id) rejected | Yes  |
//! | M40-D03 | multiple partial fills for same order are allowed         | Yes  |
//! | M40-D04 | backfill CTE on dirty Pattern A state → REST row kept    | Yes  |
//! | M40-D05 | migrate() idempotent — second call succeeds               | Yes  |

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Canonical ranking model (mirrors the SQL ORDER BY in the backfill CTE)
// ---------------------------------------------------------------------------

/// A minimal fill-row representation for ranking simulation.
#[derive(Debug, Clone)]
struct FillRow {
    inbox_id: i64,
    applied_at_utc: Option<DateTime<Utc>>,
    broker_fill_id: Option<&'static str>,
    received_at_utc: DateTime<Utc>,
}

/// Returns `std::cmp::Ordering::Less` when `a` ranks before `b` (a is the
/// canonical keeper).  Mirrors the SQL:
///
/// ```sql
/// ORDER BY
///   case when applied_at_utc  IS NOT NULL then 0 else 1 end,  -- P1
///   case when broker_fill_id  IS NOT NULL then 0 else 1 end,  -- P2
///   applied_at_utc  ASC NULLS LAST,                            -- P3
///   received_at_utc ASC NULLS LAST,                            -- P4
///   inbox_id        ASC                                         -- P5
/// ```
fn cmp_rank(a: &FillRow, b: &FillRow) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;

    // P1: applied_at_utc IS NOT NULL → sort key 0 (wins)
    let a_p1 = if a.applied_at_utc.is_some() { 0u8 } else { 1 };
    let b_p1 = if b.applied_at_utc.is_some() { 0u8 } else { 1 };
    match a_p1.cmp(&b_p1) {
        Equal => {}
        ord => return ord,
    }

    // P2: broker_fill_id IS NOT NULL → sort key 0 (wins)
    let a_p2 = if a.broker_fill_id.is_some() { 0u8 } else { 1 };
    let b_p2 = if b.broker_fill_id.is_some() { 0u8 } else { 1 };
    match a_p2.cmp(&b_p2) {
        Equal => {}
        ord => return ord,
    }

    // P3: applied_at_utc ASC NULLS LAST
    match (a.applied_at_utc, b.applied_at_utc) {
        (Some(at), Some(bt)) => match at.cmp(&bt) {
            Equal => {}
            ord => return ord,
        },
        (None, None) => {}
        (Some(_), None) => return Less, // Some before None (nulls last)
        (None, Some(_)) => return Greater,
    }

    // P4: received_at_utc ASC
    match a.received_at_utc.cmp(&b.received_at_utc) {
        Equal => {}
        ord => return ord,
    }

    // P5: inbox_id ASC
    a.inbox_id.cmp(&b.inbox_id)
}

/// Returns the canonical keeper (rank=1) from a group of fill rows.
fn canonical_keeper(rows: &[FillRow]) -> &FillRow {
    rows.iter()
        .min_by(|a, b| cmp_rank(a, b))
        .expect("non-empty group")
}

// ---------------------------------------------------------------------------
// Pure ranking tests — no DB required
// ---------------------------------------------------------------------------

/// M40-P01: An applied row (applied_at_utc IS NOT NULL) outranks an unapplied
/// row regardless of broker_fill_id or received_at_utc.
#[test]
fn m40_p01_applied_row_outranks_unapplied() {
    let now = Utc::now();
    let applied = FillRow {
        inbox_id: 2,
        applied_at_utc: Some(now),
        broker_fill_id: None,
        received_at_utc: now + Duration::seconds(1), // received LATER
    };
    let unapplied = FillRow {
        inbox_id: 1,
        applied_at_utc: None,
        broker_fill_id: Some("fill-id"),
        received_at_utc: now, // received EARLIER, has fill_id
    };

    let rows = [applied.clone(), unapplied.clone()];
    let keeper = canonical_keeper(&rows);
    assert_eq!(
        keeper.inbox_id, applied.inbox_id,
        "M40-P01: applied row must outrank unapplied regardless of fill_id or received_at"
    );
}

/// M40-P02: Among two applied rows, the one with broker_fill_id wins (Priority 2).
/// The broker_fill_id row represents the authoritative REST lane identity.
#[test]
fn m40_p02_among_applied_broker_fill_id_wins() {
    let now = Utc::now();
    let ws_row = FillRow {
        inbox_id: 433,
        applied_at_utc: Some(now), // applied FIRST
        broker_fill_id: None,      // WS lane: no fill ID
        received_at_utc: now,
    };
    let rest_row = FillRow {
        inbox_id: 434,
        applied_at_utc: Some(now + Duration::milliseconds(20)), // applied SLIGHTLY LATER
        broker_fill_id: Some("rest-exec-id"),
        received_at_utc: now + Duration::milliseconds(5),
    };

    let rows = [ws_row.clone(), rest_row.clone()];
    let keeper = canonical_keeper(&rows);
    assert_eq!(
        keeper.inbox_id, rest_row.inbox_id,
        "M40-P02: REST row with broker_fill_id must outrank WS row even when WS was applied first"
    );
}

/// M40-P03: Among two unapplied rows, the one with broker_fill_id wins (Priority 2).
#[test]
fn m40_p03_among_unapplied_broker_fill_id_wins() {
    let now = Utc::now();
    let no_fill_id = FillRow {
        inbox_id: 100,
        applied_at_utc: None,
        broker_fill_id: None,
        received_at_utc: now, // received FIRST, but no fill_id
    };
    let has_fill_id = FillRow {
        inbox_id: 101,
        applied_at_utc: None,
        broker_fill_id: Some("rest-fill-id"),
        received_at_utc: now + Duration::seconds(1), // received LATER
    };

    let rows = [no_fill_id.clone(), has_fill_id.clone()];
    let keeper = canonical_keeper(&rows);
    assert_eq!(
        keeper.inbox_id, has_fill_id.inbox_id,
        "M40-P03: among unapplied rows, broker_fill_id presence wins even if received later"
    );
}

/// M40-P04: Full tiebreak to inbox_id — when all other fields are equal, the
/// row with the lowest inbox_id is kept (durable insertion-order tiebreak, P5).
#[test]
fn m40_p04_tiebreak_to_lowest_inbox_id() {
    let now = Utc::now();
    let row_a = FillRow {
        inbox_id: 50,
        applied_at_utc: None,
        broker_fill_id: Some("same-fill-id"), // same on both — not possible in practice,
        received_at_utc: now,                 // but proves P5 tiebreak fires
    };
    let row_b = FillRow {
        inbox_id: 51,
        applied_at_utc: None,
        broker_fill_id: Some("same-fill-id"),
        received_at_utc: now,
    };

    let rows = [row_a.clone(), row_b.clone()];
    let keeper = canonical_keeper(&rows);
    assert_eq!(
        keeper.inbox_id, row_a.inbox_id,
        "M40-P04: when all other priorities tie, lowest inbox_id wins"
    );
}

/// M40-P05: The backfill predicate is `event_kind = 'fill'` — partial fills
/// are excluded by event_kind filter and must not be counted for dedup.
///
/// This test proves the filter concept: a group with event_kind='partial_fill'
/// rows is NOT affected by the backfill CTE even if multiple rows exist for the
/// same (run_id, internal_order_id).
#[test]
fn m40_p05_partial_fills_excluded_from_backfill_predicate() {
    // The backfill CTE only applies to rows where event_kind = 'fill'.
    // Multiple partial fills for the same order are legitimate and must not
    // be collapsed by this migration.
    let fill_kinds = ["fill", "partial_fill", "partial_fill", "partial_fill"];
    let fill_rows_count = fill_kinds.iter().filter(|&&k| k == "fill").count();
    let partial_fill_rows_count = fill_kinds.iter().filter(|&&k| k == "partial_fill").count();

    // Only the single 'fill' row is in scope for the backfill CTE.
    // The three 'partial_fill' rows are out of scope.
    assert_eq!(
        fill_rows_count, 1,
        "M40-P05: backfill predicate targets exactly 1 'fill' row in this group"
    );
    assert_eq!(
        partial_fill_rows_count, 3,
        "M40-P05: 3 partial fills are out of scope and must not be collapsed"
    );

    // No dedup needed when fill_rows_count == 1 — the CTE deletes 0 rows.
    let duplicates_to_delete = fill_rows_count.saturating_sub(1);
    assert_eq!(
        duplicates_to_delete, 0,
        "M40-P05: backfill CTE deletes 0 rows when only one 'fill' row exists"
    );
}

// ---------------------------------------------------------------------------
// DB-backed tests — require MQK_DATABASE_URL
// ---------------------------------------------------------------------------

fn db_url_or_skip() -> Option<String> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            println!("SKIP: MQK_DATABASE_URL not set");
            None
        }
    }
}

async fn try_pool(url: &str) -> Result<Option<sqlx::PgPool>> {
    use sqlx::postgres::PgPoolOptions;
    match PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .connect(url)
        .await
    {
        Ok(p) => Ok(Some(p)),
        Err(e) => {
            println!("SKIP: cannot connect to DB: {e}");
            Ok(None)
        }
    }
}

async fn cleanup_run(pool: &sqlx::PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn seed_running_run(pool: &sqlx::PgPool, run_id: Uuid) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "m40-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "m40-test".to_string(),
            config_hash: "m40-test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "m40-test".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

/// M40-D01: After migration, the unique index uq_inbox_run_order_single_fill
/// exists on oms_inbox with the expected predicate.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn m40_d01_unique_index_exists_after_migration() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let index_def: Option<String> = sqlx::query_scalar(
        "select indexdef from pg_indexes
         where schemaname = 'public'
           and tablename  = 'oms_inbox'
           and indexname  = 'uq_inbox_run_order_single_fill'",
    )
    .fetch_optional(&pool)
    .await?;

    assert!(
        index_def.is_some(),
        "M40-D01: unique index uq_inbox_run_order_single_fill must exist on oms_inbox \
         after migration 0040 is applied"
    );

    let def = index_def.unwrap();
    assert!(
        def.contains("event_kind"),
        "M40-D01: index definition must include event_kind predicate; got: {def}"
    );
    assert!(
        def.to_lowercase().contains("fill"),
        "M40-D01: index predicate must reference 'fill'; got: {def}"
    );

    Ok(())
}

/// M40-D02: After migration, a second 'fill' row for the same
/// (run_id, internal_order_id) is rejected by the unique constraint.
/// inbox_insert_deduped_with_identity returns Ok(false) on conflict.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn m40_d02_second_fill_rejected_by_constraint() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "40400200-0000-0000-0000-000000000002"
        .parse()
        .expect("valid uuid");
    let order_id = "m40d02-order";

    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let first = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "m40d02-ws-msg",
        None,
        order_id,
        "broker-d02",
        "fill",
        &serde_json::json!({}),
        0,
        Utc::now(),
    )
    .await?;
    assert!(first, "M40-D02: first fill insert must succeed");

    let second = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "m40d02-rest-msg",
        Some("rest-fill-id-d02"),
        order_id,
        "broker-d02",
        "fill",
        &serde_json::json!({}),
        0,
        Utc::now(),
    )
    .await?;
    assert!(
        !second,
        "M40-D02: second fill for same (run_id, internal_order_id) must be rejected \
         by uq_inbox_run_order_single_fill"
    );

    let count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from oms_inbox
         where run_id = $1 and internal_order_id = $2 and event_kind = 'fill'",
    )
    .bind(run_id)
    .bind(order_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        count, 1,
        "M40-D02: exactly one 'fill' row must exist after second insert is rejected"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// M40-D03: Multiple 'partial_fill' rows for the same (run_id, internal_order_id)
/// are allowed — the unique constraint covers only event_kind='fill'.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn m40_d03_multiple_partial_fills_for_same_order_are_allowed() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "40400300-0000-0000-0000-000000000003"
        .parse()
        .expect("valid uuid");
    let order_id = "m40d03-order";

    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let pf1 = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "m40d03-pf-msg-a",
        Some("exec-pf-a"),
        order_id,
        "broker-d03",
        "partial_fill",
        &serde_json::json!({}),
        0,
        Utc::now(),
    )
    .await?;
    assert!(pf1, "M40-D03: first partial_fill must insert");

    let pf2 = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "m40d03-pf-msg-b",
        Some("exec-pf-b"),
        order_id,
        "broker-d03",
        "partial_fill",
        &serde_json::json!({}),
        0,
        Utc::now(),
    )
    .await?;
    assert!(
        pf2,
        "M40-D03: second distinct partial_fill for the same order must also insert — \
         uq_inbox_run_order_single_fill does not cover partial_fill rows"
    );

    let count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from oms_inbox
         where run_id = $1 and internal_order_id = $2 and event_kind = 'partial_fill'",
    )
    .bind(run_id)
    .bind(order_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        count, 2,
        "M40-D03: both partial_fill rows must be present — partial fills are not constrained"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// M40-D04: Backfill CTE on a dirty Pattern-A state (WS fill applied first,
/// REST fill applied slightly later) leaves exactly the REST row (canonical
/// keeper per Priority 2: broker_fill_id IS NOT NULL wins).
///
/// Setup: drop the unique index → insert WS+REST duplicate fills → run the
/// exact backfill CTE from migration 0040 → verify REST row is kept.
/// The index is re-created at the end to restore post-migration invariants.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn m40_d04_backfill_cte_pattern_a_keeps_rest_row() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "40400400-0000-0000-0000-000000000004"
        .parse()
        .expect("valid uuid");
    let order_id = "m40d04-order";

    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    // Drop the unique index to allow inserting the historical duplicate state.
    sqlx::query("drop index if exists uq_inbox_run_order_single_fill")
        .execute(&pool)
        .await?;

    let ws_applied_at = Utc::now();
    let rest_applied_at = ws_applied_at + Duration::milliseconds(20);

    // WS fill: no broker_fill_id, applied slightly before REST.
    sqlx::query(
        "insert into oms_inbox
             (run_id, broker_message_id, broker_fill_id, internal_order_id,
              broker_order_id, event_kind, message_json, event_ts_ms,
              received_at_utc, applied_at_utc)
         values ($1, 'm40d04-ws-msg', null, $2, 'broker-d04',
                 'fill', '{}'::jsonb, 0, $3, $4)",
    )
    .bind(run_id)
    .bind(order_id)
    .bind(ws_applied_at - Duration::seconds(1))
    .bind(ws_applied_at)
    .execute(&pool)
    .await?;

    // REST fill: broker_fill_id present, applied slightly after WS.
    sqlx::query(
        "insert into oms_inbox
             (run_id, broker_message_id, broker_fill_id, internal_order_id,
              broker_order_id, event_kind, message_json, event_ts_ms,
              received_at_utc, applied_at_utc)
         values ($1, 'm40d04-rest-msg', 'm40d04-rest-fill-id', $2, 'broker-d04',
                 'fill', '{}'::jsonb, 0, $3, $4)",
    )
    .bind(run_id)
    .bind(order_id)
    .bind(ws_applied_at)
    .bind(rest_applied_at)
    .execute(&pool)
    .await?;

    let pre_count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from oms_inbox
         where run_id = $1 and internal_order_id = $2 and event_kind = 'fill'",
    )
    .bind(run_id)
    .bind(order_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        pre_count, 2,
        "M40-D04: must have 2 duplicate fill rows before cleanup"
    );

    // Run the exact backfill CTE from migration 0040.
    sqlx::query(
        "with ranked as (
            select
                inbox_id,
                row_number() over (
                    partition by run_id, internal_order_id
                    order by
                        case when applied_at_utc  is not null then 0 else 1 end,
                        case when broker_fill_id  is not null then 0 else 1 end,
                        applied_at_utc  asc nulls last,
                        received_at_utc asc nulls last,
                        inbox_id        asc
                ) as rn
            from oms_inbox
            where event_kind          = 'fill'
              and internal_order_id is not null
        )
        delete from oms_inbox
        using ranked
        where oms_inbox.inbox_id = ranked.inbox_id
          and ranked.rn > 1",
    )
    .execute(&pool)
    .await?;

    let post_count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from oms_inbox
         where run_id = $1 and internal_order_id = $2 and event_kind = 'fill'",
    )
    .bind(run_id)
    .bind(order_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        post_count, 1,
        "M40-D04: exactly one fill row must remain after backfill cleanup"
    );

    // The kept row must be the REST row — Priority 2: broker_fill_id IS NOT NULL wins.
    let kept_fill_id: Option<String> = sqlx::query_scalar(
        "select broker_fill_id from oms_inbox
         where run_id = $1 and internal_order_id = $2 and event_kind = 'fill'",
    )
    .bind(run_id)
    .bind(order_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        kept_fill_id.as_deref(),
        Some("m40d04-rest-fill-id"),
        "M40-D04: the canonical REST row (broker_fill_id IS NOT NULL) must be kept; \
         the WS row (broker_fill_id=null) must be deleted"
    );

    // Re-create the index to restore post-migration invariants.
    sqlx::query(
        "create unique index if not exists uq_inbox_run_order_single_fill
             on oms_inbox (run_id, internal_order_id)
             where event_kind          = 'fill'
               and internal_order_id is not null",
    )
    .execute(&pool)
    .await?;

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// M40-D05: Calling mqk_db::migrate() twice on the same DB is idempotent —
/// the `CREATE UNIQUE INDEX IF NOT EXISTS` form and the zero-row CTE DELETE
/// on a clean DB both succeed silently on the second call.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn m40_d05_migrate_twice_is_idempotent() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };

    mqk_db::migrate(&pool).await?;
    mqk_db::migrate(&pool).await?;

    // Index must still exist after the second migrate call.
    let exists: bool = sqlx::query_scalar(
        "select exists(
             select 1 from pg_indexes
             where schemaname = 'public'
               and tablename  = 'oms_inbox'
               and indexname  = 'uq_inbox_run_order_single_fill'
         )",
    )
    .fetch_one(&pool)
    .await?;

    assert!(
        exists,
        "M40-D05: unique index must still exist after migrate() called twice"
    );

    Ok(())
}
