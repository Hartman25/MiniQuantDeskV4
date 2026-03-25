//! CC-01A / CC-01B: Strategy registry and active-fleet durable-truth scenarios.
//!
//! CC-01A proves:
//! - canonical strategy identities are durable and queryable after upsert
//! - duplicate/conflicting identities are handled canonically (upsert updates
//!   mutable fields; `registered_at_utc` is preserved from first insert)
//! - missing registry state returns `None`, not a fake/synthesized row
//! - empty `strategy_id` is rejected before any DB contact
//! - `fetch_strategy_registry` returns results ordered by `strategy_id`
//!
//! CC-01B proves:
//! - `enabled` field is the authoritative active-fleet flag; fetch returns both
//!   enabled and disabled strategies, allowing callers to distinguish them
//! - disabling a strategy does not remove it from the registry; it remains
//!   queryable as registered + inactive
//! - active fleet (enabled strategies) can be derived correctly by filtering on
//!   `enabled = true`; this is the canonical fleet activation seam
//! - unregistered strategy IDs produce no registry row (not a fake active entry)
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-db --test scenario_strategy_registry -- --include-ignored

use chrono::{Duration, Utc};
use mqk_db::{
    fetch_strategy_registry, fetch_strategy_registry_entry, upsert_strategy_registry_entry,
    UpsertStrategyRegistryArgs, ENV_DB_URL,
};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    let url = match std::env::var(ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-db --test scenario_strategy_registry -- --include-ignored"
        ),
    };
    let pool = PgPoolOptions::new().max_connections(2).connect(&url).await?;
    mqk_db::migrate(&pool).await?;
    Ok(pool)
}

fn make_args(
    strategy_id: &str,
    display_name: &str,
    enabled: bool,
    kind: &str,
    note: &str,
    registered_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
) -> UpsertStrategyRegistryArgs {
    UpsertStrategyRegistryArgs {
        strategy_id: strategy_id.to_string(),
        display_name: display_name.to_string(),
        enabled,
        kind: kind.to_string(),
        registered_at_utc: registered_at,
        updated_at_utc: updated_at,
        note: note.to_string(),
    }
}

/// Generate a unique strategy_id for test isolation.
fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// CC-01A: canonical strategy identity is durable and queryable after upsert.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn registry_insert_is_durable_and_queryable() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let ts = Utc::now();
    let id = unique_id("cc01a_durable");

    upsert_strategy_registry_entry(
        &pool,
        &make_args(&id, "Durable Test Strategy", true, "external_signal", "", ts, ts),
    )
    .await?;

    let entry = fetch_strategy_registry_entry(&pool, &id)
        .await?
        .expect("entry must be present immediately after upsert");

    assert_eq!(entry.strategy_id, id);
    assert_eq!(entry.display_name, "Durable Test Strategy");
    assert!(entry.enabled);
    assert_eq!(entry.kind, "external_signal");
    assert_eq!(entry.note, "");
    // Timestamps are caller-injected and must round-trip faithfully.
    assert_eq!(entry.registered_at_utc.timestamp(), ts.timestamp());
    assert_eq!(entry.updated_at_utc.timestamp(), ts.timestamp());

    Ok(())
}

/// CC-01A: duplicate/conflicting identities are handled canonically.
///
/// A second upsert with the same `strategy_id` must:
/// - update mutable fields (`display_name`, `enabled`, `kind`, `updated_at_utc`, `note`)
/// - preserve `registered_at_utc` from the first insert (not overwrite with new value)
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn registry_upsert_updates_mutable_fields_preserves_registered_at() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let t1 = Utc::now();
    let t2 = t1 + Duration::seconds(90);
    let id = unique_id("cc01a_upsert");

    // First insert.
    upsert_strategy_registry_entry(
        &pool,
        &make_args(&id, "Original Name", true, "bar_driven", "", t1, t1),
    )
    .await?;

    // Second upsert: mutate every field, supply a different registered_at.
    upsert_strategy_registry_entry(
        &pool,
        &make_args(&id, "Updated Name", false, "external_signal", "operator note", t2, t2),
    )
    .await?;

    let entry = fetch_strategy_registry_entry(&pool, &id)
        .await?
        .expect("entry must exist after second upsert");

    // Mutable fields must reflect the second upsert.
    assert_eq!(entry.display_name, "Updated Name");
    assert!(!entry.enabled);
    assert_eq!(entry.kind, "external_signal");
    assert_eq!(entry.note, "operator note");
    assert_eq!(entry.updated_at_utc.timestamp(), t2.timestamp());

    // registered_at_utc must be preserved from the first insert.
    assert_eq!(
        entry.registered_at_utc.timestamp(),
        t1.timestamp(),
        "registered_at_utc must not be overwritten on conflict"
    );

    Ok(())
}

/// CC-01A: missing registry state does not produce fake strategy truth.
///
/// `fetch_strategy_registry_entry` must return `None` for an unknown ID,
/// never a synthesized/default row.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn registry_unknown_id_returns_none_not_fake_row() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let nonexistent = unique_id("cc01a_ghost");

    let result = fetch_strategy_registry_entry(&pool, &nonexistent).await?;
    assert!(
        result.is_none(),
        "unknown strategy_id must return None, not a synthesized row"
    );

    Ok(())
}

/// CC-01A: empty `strategy_id` is rejected before any DB contact.
///
/// `upsert_strategy_registry_entry` must return `Err` immediately when
/// `strategy_id` is empty or whitespace-only.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn registry_empty_strategy_id_rejected_before_db() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let ts = Utc::now();

    for bad_id in &["", "   ", "\t"] {
        let result = upsert_strategy_registry_entry(
            &pool,
            &make_args(bad_id, "Should Not Insert", true, "", "", ts, ts),
        )
        .await;

        assert!(
            result.is_err(),
            "empty/blank strategy_id '{bad_id:?}' must be rejected"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("strategy_id must not be empty"),
            "error must name the violated constraint; got: {msg}"
        );
    }

    Ok(())
}

/// CC-01A: `fetch_strategy_registry` returns all registered entries ordered by
/// `strategy_id` (deterministic, not insertion-order).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn registry_fetch_all_ordered_by_strategy_id() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let ts = Utc::now();

    // Use a stable prefix so we can isolate our rows from any pre-existing data.
    let u = Uuid::new_v4().to_string().replace('-', "");
    let prefix = format!("cc01a_ord_{}", &u[..8]);

    let id_c = format!("{prefix}_c");
    let id_a = format!("{prefix}_a");
    let id_b = format!("{prefix}_b");

    // Insert deliberately out of lexicographic order.
    upsert_strategy_registry_entry(&pool, &make_args(&id_c, "C", true, "", "", ts, ts)).await?;
    upsert_strategy_registry_entry(&pool, &make_args(&id_a, "A", true, "", "", ts, ts)).await?;
    upsert_strategy_registry_entry(&pool, &make_args(&id_b, "B", true, "", "", ts, ts)).await?;

    let all = fetch_strategy_registry(&pool).await?;
    let ours: Vec<_> = all
        .iter()
        .filter(|r| r.strategy_id.starts_with(&prefix))
        .collect();

    assert_eq!(ours.len(), 3, "must find exactly the 3 inserted rows");
    assert_eq!(ours[0].strategy_id, id_a, "must be ordered a < b < c");
    assert_eq!(ours[1].strategy_id, id_b);
    assert_eq!(ours[2].strategy_id, id_c);

    Ok(())
}

/// CC-01A: `fetch_strategy_registry` on an empty (or zero-matching) registry
/// returns an empty Vec — not an error, not fake rows.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn registry_fetch_all_empty_result_is_authoritative() -> anyhow::Result<()> {
    let pool = test_pool().await?;

    // Using a prefix guaranteed never to be inserted in any other test.
    let u = Uuid::new_v4().to_string().replace('-', "");
    let prefix = format!("cc01a_empty_{}", &u[..16]);

    let all = fetch_strategy_registry(&pool).await?;
    let ours: Vec<_> = all
        .iter()
        .filter(|r| r.strategy_id.starts_with(&prefix))
        .collect();

    // Must be empty — no fake/synthesized rows.
    assert!(
        ours.is_empty(),
        "fetch_strategy_registry must return empty Vec for unknown prefix, not fake rows"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// CC-01B: Active-fleet wiring scenarios
// ---------------------------------------------------------------------------

/// CC-01B: `enabled` is the authoritative active-fleet flag.
///
/// fetch_strategy_registry returns both enabled and disabled strategies.
/// Callers derive the active fleet by filtering on `enabled = true`.
/// Disabled strategies remain registered + queryable; they are not removed.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn active_fleet_enabled_flag_distinguishes_active_from_inactive() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let ts = Utc::now();
    let u = Uuid::new_v4().to_string().replace('-', "");
    let prefix = format!("cc01b_fleet_{}", &u[..8]);

    let id_active = format!("{prefix}_active");
    let id_inactive = format!("{prefix}_inactive");

    // Register one active (enabled) and one inactive (disabled) strategy.
    upsert_strategy_registry_entry(
        &pool,
        &make_args(&id_active, "Active Strategy", true, "external_signal", "", ts, ts),
    )
    .await?;
    upsert_strategy_registry_entry(
        &pool,
        &make_args(&id_inactive, "Inactive Strategy", false, "bar_driven", "", ts, ts),
    )
    .await?;

    let all = fetch_strategy_registry(&pool).await?;
    let ours: Vec<_> = all
        .iter()
        .filter(|r| r.strategy_id.starts_with(&prefix))
        .collect();

    // Both strategies must appear in the registry (enabled and disabled).
    assert_eq!(ours.len(), 2, "both active and inactive strategies must be in registry");

    // Derive the active fleet by filtering on enabled = true.
    let active: Vec<_> = ours.iter().filter(|r| r.enabled).collect();
    let inactive: Vec<_> = ours.iter().filter(|r| !r.enabled).collect();

    assert_eq!(active.len(), 1);
    assert_eq!(active[0].strategy_id, id_active);

    assert_eq!(inactive.len(), 1);
    assert_eq!(inactive[0].strategy_id, id_inactive);

    Ok(())
}

/// CC-01B: disabling a registered strategy marks it inactive but does not remove it.
///
/// A strategy that transitions from enabled to disabled must remain in the registry
/// with enabled = false.  The active fleet shrinks by one; the known-strategy set
/// does not shrink.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn active_fleet_disable_keeps_strategy_in_registry() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let ts = Utc::now();
    let id = unique_id("cc01b_disable");

    // Register as enabled.
    upsert_strategy_registry_entry(
        &pool,
        &make_args(&id, "Will Be Disabled", true, "external_signal", "", ts, ts),
    )
    .await?;

    let before = fetch_strategy_registry_entry(&pool, &id)
        .await?
        .expect("must exist");
    assert!(before.enabled, "must start enabled");

    // Disable via upsert.
    let ts2 = ts + chrono::Duration::seconds(10);
    upsert_strategy_registry_entry(
        &pool,
        &make_args(&id, "Will Be Disabled", false, "external_signal", "", ts, ts2),
    )
    .await?;

    let after = fetch_strategy_registry_entry(&pool, &id)
        .await?
        .expect("must still exist after disable");

    // Strategy is still registered — it is not removed from the registry.
    assert_eq!(after.strategy_id, id);
    // But it is now inactive (not in active fleet).
    assert!(!after.enabled, "disabled strategy must have enabled = false");

    Ok(())
}

/// CC-01B: unregistered strategy IDs produce no registry row.
///
/// An ID that was never registered must not appear in fetch_strategy_registry
/// or fetch_strategy_registry_entry.  There is no implicit "active by default"
/// entry for unregistered IDs.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn active_fleet_unregistered_id_produces_no_active_entry() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let unregistered = unique_id("cc01b_unreg");

    // Must not appear in the full registry listing.
    let all = fetch_strategy_registry(&pool).await?;
    assert!(
        !all.iter().any(|r| r.strategy_id == unregistered),
        "unregistered strategy must not appear in fetch_strategy_registry"
    );

    // Must not appear as a single-entry fetch.
    let entry = fetch_strategy_registry_entry(&pool, &unregistered).await?;
    assert!(
        entry.is_none(),
        "unregistered strategy must return None from fetch_strategy_registry_entry"
    );

    Ok(())
}
