use mqk_db::md::load_md_bars_for_backtest;

/// DB-backed loader test.
///
/// This test is ignored by default because it requires a Postgres instance
/// reachable via MQK_DATABASE_URL.
///
/// Run:
///   MQK_DATABASE_URL=... cargo test -p mqk-db --test md_load_backtest -- --ignored
#[tokio::test]
#[ignore]
async fn load_md_bars_for_backtest_is_deterministically_ordered() {
    let pool = mqk_db::testkit_db_pool().await.expect("db pool");

    // Clean slate.
    sqlx::query("delete from md_bars")
        .execute(&pool)
        .await
        .expect("clear md_bars");

    // Insert intentionally shuffled rows (order of insertion must not affect load ordering).
    // timeframe: 1m, end_ts: 120 then 60, and two symbols at same end_ts.
    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete
        ) values
          ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        "#,
    )
    .bind("B")
    .bind("1m")
    .bind(120_i64)
    .bind(2_000_000_i64)
    .bind(2_010_000_i64)
    .bind(1_990_000_i64)
    .bind(2_005_000_i64)
    .bind(100_i64)
    .bind(true)
    .execute(&pool)
    .await
    .expect("insert B@120");

    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete
        ) values
          ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        "#,
    )
    .bind("A")
    .bind("1m")
    .bind(60_i64)
    .bind(1_000_000_i64)
    .bind(1_010_000_i64)
    .bind(990_000_i64)
    .bind(1_005_000_i64)
    .bind(200_i64)
    .bind(true)
    .execute(&pool)
    .await
    .expect("insert A@60");

    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete
        ) values
          ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        "#,
    )
    .bind("A")
    .bind("1m")
    .bind(120_i64)
    .bind(1_500_000_i64)
    .bind(1_510_000_i64)
    .bind(1_490_000_i64)
    .bind(1_505_000_i64)
    .bind(300_i64)
    .bind(true)
    .execute(&pool)
    .await
    .expect("insert A@120");

    let mut symbols = vec!["B".to_string(), "A".to_string()];
    // Intentionally unsorted input must not matter.
    symbols.reverse();

    let rows = load_md_bars_for_backtest(&pool, "1m", 0, 999_999, &symbols)
        .await
        .expect("load");

    // Deterministic order: (end_ts ASC, symbol ASC)
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].end_ts, 60);
    assert_eq!(rows[0].symbol, "A");
    assert_eq!(rows[1].end_ts, 120);
    assert_eq!(rows[1].symbol, "A");
    assert_eq!(rows[2].end_ts, 120);
    assert_eq!(rows[2].symbol, "B");
}
