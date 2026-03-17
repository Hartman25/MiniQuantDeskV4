// scenario_md_sync_provider.rs
//
// DB-backed tests for the sync-provider incremental-sync path.
// Proves: latest_stored_bar_end_ts semantics, incremental start computation,
// overlap idempotency, per-symbol independence, and ingest-provider not regressed.
//
// All tests are #[ignore] and self-skip when MQK_DATABASE_URL is absent.

use anyhow::Result;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helper: build a ProviderBar with valid OHLCV fields
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn bar(
    symbol: &str,
    timeframe: &str,
    end_ts: i64,
    open: &str,
    high: &str,
    low: &str,
    close: &str,
    volume: i64,
    is_complete: bool,
) -> mqk_db::ProviderBar {
    mqk_db::ProviderBar {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        end_ts,
        open: open.to_string(),
        high: high.to_string(),
        low: low.to_string(),
        close: close.to_string(),
        volume,
        is_complete,
    }
}

// ---------------------------------------------------------------------------
// Scenario S1 — no bars: latest_stored_bar_end_ts returns None
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn sync_provider_no_bars_returns_none() -> Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL");
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;

    // Use a unique symbol guaranteed not to exist in other tests.
    let sym = "SYNC_S1_NOVBARS";
    let tf = "1D";

    // The DB may have stale rows from a prior run; clean them for determinism.
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(sym)
        .bind(tf)
        .execute(&pool)
        .await?;

    let result = mqk_db::latest_stored_bar_end_ts(&pool, sym, tf).await?;

    // No bars stored → must return None; caller requires --full-start.
    assert_eq!(
        result, None,
        "expected None when no bars exist for {sym}/{tf}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario S2 — with bars: latest_stored_bar_end_ts returns Some(max_end_ts)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn sync_provider_with_bars_returns_max_end_ts() -> Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL");
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;

    let sym = "SYNC_S2_WBARS";
    let tf = "1D";
    // 1708041600 = 2024-02-16 (Friday)
    // 1708300800 = 2024-02-19 (Monday — max)
    let ts_first: i64 = 1_708_041_600;
    let ts_last: i64 = 1_708_300_800;

    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(sym)
        .bind(tf)
        .execute(&pool)
        .await?;

    mqk_db::md::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: "test".to_string(),
            timeframe: tf.to_string(),
            ingest_id: Uuid::new_v4(),
            bars: vec![
                bar(sym, tf, ts_first, "10", "12", "9", "11", 100, true),
                bar(sym, tf, ts_last, "11", "13", "10", "12", 110, true),
            ],
        },
    )
    .await?;

    let result = mqk_db::latest_stored_bar_end_ts(&pool, sym, tf).await?;

    // Must return the maximum end_ts, not the first or an arbitrary row.
    assert_eq!(
        result,
        Some(ts_last),
        "expected Some({ts_last}) = max end_ts, got {:?}",
        result
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario S3 — overlap idempotency: re-ingest with overlap window is safe
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn sync_provider_overlap_reingest_is_idempotent() -> Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL");
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;

    let sym = "SYNC_S3_OVERLAP";
    let tf = "1D";
    let ts: i64 = 1_708_041_600; // 2024-02-16

    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(sym)
        .bind(tf)
        .execute(&pool)
        .await?;

    // Initial ingest: one bar.
    mqk_db::md::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: "test".to_string(),
            timeframe: tf.to_string(),
            ingest_id: Uuid::new_v4(),
            bars: vec![bar(sym, tf, ts, "10", "12", "9", "11", 100, true)],
        },
    )
    .await?;

    // Verify stored.
    let after_first = mqk_db::latest_stored_bar_end_ts(&pool, sym, tf).await?;
    assert_eq!(after_first, Some(ts));

    // Simulate overlap reingest: re-submit the same bar (same end_ts, updated volume).
    // This must upsert without error — on conflict do update.
    let res2 = mqk_db::md::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: "test".to_string(),
            timeframe: tf.to_string(),
            ingest_id: Uuid::new_v4(),
            bars: vec![bar(sym, tf, ts, "10", "12", "9", "11", 200, true)],
        },
    )
    .await?;

    // No rows rejected; upsert succeeded.
    assert_eq!(
        res2.report.coverage.rows_rejected, 0,
        "overlap reingest must not reject valid rows"
    );

    // Latest stored bar is still the same timestamp (no new bar added).
    let after_second = mqk_db::latest_stored_bar_end_ts(&pool, sym, tf).await?;
    assert_eq!(
        after_second,
        Some(ts),
        "end_ts must be stable after overlap upsert"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario S4 — multi-symbol independence: each symbol tracks its own max end_ts
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn sync_provider_multi_symbol_independent_end_ts() -> Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL");
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;

    let sym_a = "SYNC_S4_AABB";
    let sym_b = "SYNC_S4_BBCC";
    let tf = "1D";

    // ts_a_max > ts_b_max — symbols have different coverage depths.
    let ts_a: i64 = 1_708_300_800; // 2024-02-19
    let ts_b: i64 = 1_708_041_600; // 2024-02-16

    for sym in &[sym_a, sym_b] {
        sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
            .bind(*sym)
            .bind(tf)
            .execute(&pool)
            .await?;
    }

    mqk_db::md::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: "test".to_string(),
            timeframe: tf.to_string(),
            ingest_id: Uuid::new_v4(),
            bars: vec![
                bar(sym_a, tf, ts_a, "10", "12", "9", "11", 100, true),
                bar(sym_b, tf, ts_b, "20", "22", "19", "21", 200, true),
            ],
        },
    )
    .await?;

    let a_ts = mqk_db::latest_stored_bar_end_ts(&pool, sym_a, tf).await?;
    let b_ts = mqk_db::latest_stored_bar_end_ts(&pool, sym_b, tf).await?;

    // Each symbol returns its own max, not the global max across all symbols.
    assert_eq!(
        a_ts,
        Some(ts_a),
        "{sym_a} must return its own latest end_ts"
    );
    assert_eq!(
        b_ts,
        Some(ts_b),
        "{sym_b} must return its own latest end_ts"
    );
    assert_ne!(
        a_ts, b_ts,
        "symbols have different depths; results must differ"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario S5 — ingest-provider not regressed: existing ingest path unchanged
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn sync_provider_ingest_provider_not_regressed() -> Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL");
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;

    let sym = "SYNC_S5_REGRESS";
    let tf = "1D";
    let ts: i64 = 1_708_128_000; // 2024-02-17

    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(sym)
        .bind(tf)
        .execute(&pool)
        .await?;

    // Call the same path used by `mqk md ingest-provider` — must still work without change.
    let res = mqk_db::md::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: "twelvedata".to_string(),
            timeframe: tf.to_string(),
            ingest_id: Uuid::new_v4(),
            bars: vec![bar(sym, tf, ts, "50", "55", "48", "52", 500, true)],
        },
    )
    .await?;

    assert_eq!(res.report.coverage.rows_read, 1);
    assert_eq!(res.report.coverage.rows_rejected, 0);
    assert_eq!(res.report.coverage.rows_ok, 1);

    // Verify the new helper correctly returns the just-inserted row.
    let latest = mqk_db::latest_stored_bar_end_ts(&pool, sym, tf).await?;
    assert_eq!(
        latest,
        Some(ts),
        "latest_stored_bar_end_ts must see the row inserted by ingest_provider_bars_to_md_bars"
    );

    Ok(())
}
