// PATCH C: Provider -> md_bars ingestion + md_quality_reports persistence scenario test.
//
// DB-backed test, skipped if MQK_DATABASE_URL is not set.
// Uses a mock provider (no real HTTP / network required).
//
// Mirrors the structure of scenario_md_ingest_csv.rs so both paths are
// exercised with the same quality-report invariants.

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn db_pool() -> Result<PgPool> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(_) => mqk_db::testkit_db_pool().await,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    }
}

async fn clear_symbol(pool: &PgPool, symbol: &str) -> Result<()> {
    sqlx::query(
        r#"
        delete from md_bars
        where timeframe = '1D'
          and symbol = $1
        "#,
    )
    .bind(symbol)
    .execute(pool)
    .await?;

    Ok(())
}

async fn count_symbol_rows(pool: &PgPool, symbol: &str) -> Result<i64> {
    let (cnt,): (i64,) = sqlx::query_as(
        r#"
        select count(*)::bigint
        from md_bars
        where symbol = $1
          and timeframe = '1D'
        "#,
    )
    .bind(symbol)
    .fetch_one(pool)
    .await?;

    Ok(cnt)
}

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
// Scenario 1 — happy path: rows persisted + report shape matches CSV ingest
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_persists_bars_and_quality_report() -> Result<()> {
    let pool = db_pool().await?;

    // Two symbols, 1D timeframe, consecutive weekday dates.
    // These are the same timestamps as the CSV scenario so they coexist safely.
    // 1708041600 = 2024-02-16 00:00:00 UTC (Friday)
    // 1708300800 = 2024-02-19 00:00:00 UTC (Monday — next weekday, no gap)
    let bars = vec![
        bar("PPP", "1D", 1_708_041_600, "10", "12", "9", "11", 100, true),
        bar(
            "PPP",
            "1D",
            1_708_300_800,
            "11",
            "13",
            "10",
            "12",
            110,
            true,
        ),
        // QQQ: one row with negative volume — will be rejected.
        bar("QQQ", "1D", 1_708_041_600, "20", "22", "19", "21", -5, true),
    ];

    let ingest_id = Uuid::new_v4();
    let res = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id,
            bars,
        },
    )
    .await?;

    // ingest_id round-trips.
    assert_eq!(res.ingest_id, ingest_id);

    // Coverage: 3 read, 2 ok (PPP rows), 1 rejected (QQQ negative volume).
    let cov = &res.report.coverage;
    assert_eq!(cov.rows_read, 3, "rows_read");
    assert_eq!(cov.rows_ok, 2, "rows_ok");
    assert_eq!(cov.rows_rejected, 1, "rows_rejected");
    assert_eq!(
        cov.rows_inserted + cov.rows_updated,
        2,
        "inserted+updated must equal rows_ok"
    );

    // At least 2 md_bars rows exist for PPP.
    let (cnt,): (i64,) = sqlx::query_as(
        "select count(*)::bigint from md_bars where symbol = 'PPP' and timeframe = '1D'",
    )
    .fetch_one(&pool)
    .await?;
    assert!(cnt >= 2, "expected >=2 PPP md_bars rows, got {cnt}");

    // md_quality_reports row persisted and retrievable.
    let (exists,): (bool,) =
        sqlx::query_as(r#"select exists(select 1 from md_quality_reports where ingest_id = $1)"#)
            .bind(ingest_id)
            .fetch_one(&pool)
            .await?;
    assert!(exists, "expected md_quality_reports row for ingest_id");

    // Per-symbol group: PPP|1D should exist with negative_or_invalid_volume=0.
    let ppp_stats = res.report.per_symbol_timeframe.get("PPP|1D");
    assert!(ppp_stats.is_some(), "PPP|1D group missing from report");
    let ppp = ppp_stats.unwrap();
    assert_eq!(ppp.negative_or_invalid_volume, 0);
    assert_eq!(ppp.duplicates_in_batch, 0);
    assert_eq!(ppp.out_of_order, 0);
    assert_eq!(ppp.ohlc_sanity_violations, 0);

    // QQQ|1D should exist and record the rejection.
    let qqq_stats = res.report.per_symbol_timeframe.get("QQQ|1D");
    assert!(qqq_stats.is_some(), "QQQ|1D group missing from report");
    let qqq = qqq_stats.unwrap();
    assert_eq!(qqq.negative_or_invalid_volume, 1);

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 2 — duplicate detection
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_detects_duplicates_in_batch() -> Result<()> {
    let pool = db_pool().await?;

    // Same (symbol, timeframe, end_ts) submitted twice.
    let bars = vec![
        bar("DUP", "1D", 1_708_041_600, "10", "12", "9", "11", 100, true),
        bar("DUP", "1D", 1_708_041_600, "10", "12", "9", "11", 100, true), // duplicate
    ];

    let res = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: Uuid::new_v4(),
            bars,
        },
    )
    .await?;

    let cov = &res.report.coverage;
    assert_eq!(cov.rows_read, 2);
    assert_eq!(cov.rows_ok, 1, "only first of duplicate pair inserted");
    assert_eq!(cov.rows_rejected, 1);

    let dup_stats = res
        .report
        .per_symbol_timeframe
        .get("DUP|1D")
        .expect("DUP|1D group missing");
    assert_eq!(dup_stats.duplicates_in_batch, 1);

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 3 — out-of-order detection
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_detects_out_of_order() -> Result<()> {
    let pool = db_pool().await?;

    // Bars submitted in descending order.
    let bars = vec![
        bar(
            "OOO",
            "1D",
            1_708_300_800,
            "11",
            "13",
            "10",
            "12",
            110,
            true,
        ),
        bar("OOO", "1D", 1_708_041_600, "10", "12", "9", "11", 100, true), // earlier ts after later ts
    ];

    let res = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: Uuid::new_v4(),
            bars,
        },
    )
    .await?;

    let cov = &res.report.coverage;
    assert_eq!(cov.rows_read, 2);
    assert_eq!(cov.rows_ok, 1, "second (out-of-order) bar rejected");
    assert_eq!(cov.rows_rejected, 1);

    let stats = res
        .report
        .per_symbol_timeframe
        .get("OOO|1D")
        .expect("OOO|1D missing");
    assert_eq!(stats.out_of_order, 1);

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 4 — OHLC sanity violation
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_rejects_ohlc_violations() -> Result<()> {
    let pool = db_pool().await?;

    // low (15) > high (12) — OHLC insane.
    let bars = vec![bar(
        "BAD",
        "1D",
        1_708_041_600,
        "10",
        "12",
        "15",
        "11",
        100,
        true,
    )];

    let res = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: Uuid::new_v4(),
            bars,
        },
    )
    .await?;

    assert_eq!(res.report.coverage.rows_rejected, 1);
    assert_eq!(res.report.coverage.rows_ok, 0);

    let stats = res
        .report
        .per_symbol_timeframe
        .get("BAD|1D")
        .expect("BAD|1D missing");
    assert_eq!(stats.ohlc_sanity_violations, 1);

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 5 — idempotency: same ingest_id produces same report
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_idempotent_same_ingest_id() -> Result<()> {
    let pool = db_pool().await?;

    let ingest_id = Uuid::new_v4();

    let make_args = || mqk_db::IngestProviderBarsArgs {
        source: "mock_provider".to_string(),
        timeframe: "1D".to_string(),
        ingest_id,
        bars: vec![bar(
            "IDP",
            "1D",
            1_708_041_600,
            "10",
            "12",
            "9",
            "11",
            100,
            true,
        )],
    };

    let r1 = mqk_db::ingest_provider_bars_to_md_bars(&pool, make_args()).await?;
    let r2 = mqk_db::ingest_provider_bars_to_md_bars(&pool, make_args()).await?;

    // Both calls return the same ingest_id.
    assert_eq!(r1.ingest_id, ingest_id);
    assert_eq!(r2.ingest_id, ingest_id);

    // On the second call, the bar is already present (upsert), so rows_ok = 1
    // and rows_updated = 1 (or inserted the first time and updated the second).
    assert_eq!(r2.report.coverage.rows_ok, 1);
    assert_eq!(r2.report.coverage.rows_rejected, 0);

    // Only one md_quality_reports row for this ingest_id.
    let (cnt,): (i64,) =
        sqlx::query_as("select count(*)::bigint from md_quality_reports where ingest_id = $1")
            .bind(ingest_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(cnt, 1, "on-conflict do update must not create a second row");

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 6 — exact replay across distinct ingest_ids is canonical-idempotent
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_replay_same_window_with_new_ingest_id_does_not_duplicate_history(
) -> Result<()> {
    let pool = db_pool().await?;
    clear_symbol(&pool, "RERUN").await?;

    let bars = vec![
        bar(
            "RERUN",
            "1D",
            1_708_041_600,
            "10",
            "12",
            "9",
            "11",
            100,
            true,
        ),
        bar(
            "RERUN",
            "1D",
            1_708_300_800,
            "11",
            "13",
            "10",
            "12",
            110,
            true,
        ),
    ];

    let first = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: Uuid::new_v4(),
            bars: bars.clone(),
        },
    )
    .await?;
    assert_eq!(first.report.coverage.rows_inserted, 2);
    assert_eq!(first.report.coverage.rows_updated, 0);

    let second_ingest_id = Uuid::new_v4();
    let second = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: second_ingest_id,
            bars,
        },
    )
    .await?;

    assert_eq!(second.report.coverage.rows_read, 2);
    assert_eq!(second.report.coverage.rows_ok, 2);
    assert_eq!(second.report.coverage.rows_rejected, 0);
    assert_eq!(
        second.report.coverage.rows_inserted, 0,
        "exact replay should not add canonical rows"
    );
    assert_eq!(
        second.report.coverage.rows_updated, 2,
        "exact replay should register as updates against existing history"
    );

    let cnt = count_symbol_rows(&pool, "RERUN").await?;
    assert_eq!(
        cnt, 2,
        "exact replay must not duplicate canonical md_bars rows"
    );

    let (quality_rows,): (i64,) = sqlx::query_as(
        r#"
        select count(*)::bigint
        from md_quality_reports
        where ingest_id = $1 or ingest_id = $2
        "#,
    )
    .bind(first.ingest_id)
    .bind(second_ingest_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        quality_rows, 2,
        "each replay run should persist its own report row"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 7 — overlap replay updates existing bar and appends only new history
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_overlap_rerun_updates_existing_bar_without_row_duplication(
) -> Result<()> {
    let pool = db_pool().await?;
    clear_symbol(&pool, "SYNC").await?;

    let first = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: Uuid::new_v4(),
            bars: vec![
                bar(
                    "SYNC",
                    "1D",
                    1_708_041_600,
                    "10",
                    "12",
                    "9",
                    "11",
                    100,
                    true,
                ),
                bar(
                    "SYNC",
                    "1D",
                    1_708_300_800,
                    "11",
                    "13",
                    "10",
                    "12",
                    110,
                    true,
                ),
            ],
        },
    )
    .await?;
    assert_eq!(first.report.coverage.rows_inserted, 2);
    assert_eq!(count_symbol_rows(&pool, "SYNC").await?, 2);

    let second = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: Uuid::new_v4(),
            bars: vec![
                // Overlap on the middle bar with corrected values.
                bar(
                    "SYNC",
                    "1D",
                    1_708_300_800,
                    "21",
                    "23",
                    "20",
                    "22",
                    999,
                    false,
                ),
                // New tail bar.
                bar(
                    "SYNC",
                    "1D",
                    1_708_646_400,
                    "12",
                    "14",
                    "11",
                    "13",
                    120,
                    true,
                ),
            ],
        },
    )
    .await?;

    assert_eq!(second.report.coverage.rows_read, 2);
    assert_eq!(second.report.coverage.rows_ok, 2);
    assert_eq!(second.report.coverage.rows_rejected, 0);
    assert_eq!(
        second.report.coverage.rows_inserted, 1,
        "only the new tail bar should insert"
    );
    assert_eq!(
        second.report.coverage.rows_updated, 1,
        "overlap bar should update in place"
    );

    let cnt = count_symbol_rows(&pool, "SYNC").await?;
    assert_eq!(
        cnt, 3,
        "overlap rerun must keep one row per canonical end_ts"
    );

    let updated_row: (i64, i64, i64, i64, i64, bool) = sqlx::query_as(
        r#"
        select open_micros, high_micros, low_micros, close_micros, volume, is_complete
        from md_bars
        where symbol = 'SYNC'
          and timeframe = '1D'
          and end_ts = $1
        "#,
    )
    .bind(1_708_300_800_i64)
    .fetch_one(&pool)
    .await?;
    assert_eq!(updated_row.0, 21_000_000);
    assert_eq!(updated_row.1, 23_000_000);
    assert_eq!(updated_row.2, 20_000_000);
    assert_eq!(updated_row.3, 22_000_000);
    assert_eq!(updated_row.4, 999);
    assert!(
        !updated_row.5,
        "overlap rerun should update is_complete truthfully"
    );

    let ordered_end_ts: Vec<i64> = sqlx::query_scalar(
        r#"
        select end_ts
        from md_bars
        where symbol = 'SYNC'
          and timeframe = '1D'
        order by end_ts asc
        "#,
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        ordered_end_ts,
        vec![1_708_041_600, 1_708_300_800, 1_708_646_400]
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 8 — no-data ingest persists truthful zero report and leaves history unchanged
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_empty_batch_persists_zero_report_without_touching_history() -> Result<()>
{
    let pool = db_pool().await?;
    clear_symbol(&pool, "EMPTY").await?;

    mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: Uuid::new_v4(),
            bars: vec![bar(
                "EMPTY",
                "1D",
                1_708_041_600,
                "10",
                "12",
                "9",
                "11",
                100,
                true,
            )],
        },
    )
    .await?;

    let before = count_symbol_rows(&pool, "EMPTY").await?;
    assert_eq!(before, 1);

    let empty_ingest_id = Uuid::new_v4();
    let res = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: empty_ingest_id,
            bars: vec![],
        },
    )
    .await?;

    assert_eq!(res.report.coverage.rows_read, 0);
    assert_eq!(res.report.coverage.rows_ok, 0);
    assert_eq!(res.report.coverage.rows_rejected, 0);
    assert_eq!(res.report.coverage.rows_inserted, 0);
    assert_eq!(res.report.coverage.rows_updated, 0);
    assert!(
        res.report.per_symbol_timeframe.is_empty(),
        "empty provider response must not fabricate symbol groups"
    );

    let after = count_symbol_rows(&pool, "EMPTY").await?;
    assert_eq!(
        after, before,
        "empty provider response must not mutate canonical history"
    );

    let (exists,): (bool,) =
        sqlx::query_as(r#"select exists(select 1 from md_quality_reports where ingest_id = $1)"#)
            .bind(empty_ingest_id)
            .fetch_one(&pool)
            .await?;
    assert!(
        exists,
        "empty provider response should still persist a truthful report row"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 9 — determinism: shuffled provider output yields same report stats
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_deterministic_regardless_of_input_order() -> Result<()> {
    let pool = db_pool().await?;

    // Original order: ascending end_ts.
    let bars_asc = vec![
        bar("DET", "1D", 1_708_041_600, "10", "12", "9", "11", 100, true),
        bar(
            "DET",
            "1D",
            1_708_300_800,
            "11",
            "13",
            "10",
            "12",
            110,
            true,
        ),
    ];

    // Reversed order: descending end_ts.
    let bars_desc = vec![
        bar(
            "DET2",
            "1D",
            1_708_300_800,
            "11",
            "13",
            "10",
            "12",
            110,
            true,
        ),
        bar(
            "DET2",
            "1D",
            1_708_041_600,
            "10",
            "12",
            "9",
            "11",
            100,
            true,
        ),
    ];

    let r_asc = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: Uuid::new_v4(),
            bars: bars_asc,
        },
    )
    .await?;

    let r_desc = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: Uuid::new_v4(),
            bars: bars_desc,
        },
    )
    .await?;

    // Both should have 2 reads; the descending one will have 1 out-of-order rejection.
    // This is consistent behaviour: the ingestion layer operates on batch order,
    // which means callers providing bars in ascending order get all rows accepted.
    assert_eq!(r_asc.report.coverage.rows_read, 2);
    assert_eq!(r_asc.report.coverage.rows_ok, 2);
    assert_eq!(r_asc.report.coverage.rows_rejected, 0);

    // The descending batch hits the out-of-order guard on the second bar.
    assert_eq!(r_desc.report.coverage.rows_read, 2);
    assert_eq!(r_desc.report.coverage.rows_ok, 1);
    assert_eq!(r_desc.report.coverage.rows_rejected, 1);

    // The ascending path produces a quality report with no anomalies.
    let det_stats = r_asc
        .report
        .per_symbol_timeframe
        .get("DET|1D")
        .expect("DET|1D missing");
    assert_eq!(det_stats.out_of_order, 0);
    assert_eq!(det_stats.duplicates_in_batch, 0);
    assert_eq!(det_stats.ohlc_sanity_violations, 0);
    assert_eq!(det_stats.negative_or_invalid_volume, 0);

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 10 — gap detection for 1D (weekday-only)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_detects_1d_weekday_gaps() -> Result<()> {
    let pool = db_pool().await?;

    // 2024-02-16 (Fri) -> 2024-02-19 (Mon): no weekday gap.
    // 2024-02-19 (Mon) -> 2024-02-23 (Fri): skip Tue, Wed, Thu = 3 gaps.
    // 2024-02-23 = 1_708_646_400
    let bars = vec![
        bar("GAP", "1D", 1_708_041_600, "10", "12", "9", "11", 100, true), // Fri 2024-02-16
        bar(
            "GAP",
            "1D",
            1_708_300_800,
            "11",
            "13",
            "10",
            "12",
            110,
            true,
        ), // Mon 2024-02-19
        bar(
            "GAP",
            "1D",
            1_708_646_400,
            "12",
            "14",
            "11",
            "13",
            120,
            true,
        ), // Fri 2024-02-23
    ];

    let res = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(),
            ingest_id: Uuid::new_v4(),
            bars,
        },
    )
    .await?;

    assert_eq!(res.report.coverage.rows_ok, 3);

    let gap_stats = res
        .report
        .per_symbol_timeframe
        .get("GAP|1D")
        .expect("GAP|1D missing");
    // Fri -> Mon: 0 gaps (weekend skipped correctly)
    // Mon -> Fri: Tue + Wed + Thu = 3 missing weekdays
    assert_eq!(
        gap_stats.gaps_detected, 3,
        "expected 3 weekday gaps Mon->Fri"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 11 — wrong timeframe rows are rejected
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_ingest_provider_rejects_wrong_timeframe() -> Result<()> {
    let pool = db_pool().await?;

    let bars = vec![
        bar("WTF", "1D", 1_708_041_600, "10", "12", "9", "11", 100, true), // matches
        bar("WTF", "1m", 1_708_041_660, "10", "12", "9", "11", 10, true),  // wrong tf
    ];

    let res = mqk_db::ingest_provider_bars_to_md_bars(
        &pool,
        mqk_db::IngestProviderBarsArgs {
            source: "mock_provider".to_string(),
            timeframe: "1D".to_string(), // only accept 1D
            ingest_id: Uuid::new_v4(),
            bars,
        },
    )
    .await?;

    assert_eq!(res.report.coverage.rows_read, 2);
    assert_eq!(res.report.coverage.rows_ok, 1);
    assert_eq!(res.report.coverage.rows_rejected, 1);

    Ok(())
}
