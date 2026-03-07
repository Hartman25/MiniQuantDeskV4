use anyhow::Result;
use mqk_db::md::load_md_bars_for_backtest;
use sqlx::PgPool;

async fn maybe_pool() -> Option<PgPool> {
    match mqk_db::connect_from_env().await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("SKIP: requires working MQK_DATABASE_URL ({e})");
            None
        }
    }
}

/// DB-backed loader test.
///
/// This test is ignored by default because it requires a Postgres instance
/// reachable via MQK_DATABASE_URL.
///
/// Run:
///   MQK_DATABASE_URL=... cargo test -p mqk-db --test md_load_backtest -- --ignored
#[tokio::test]
#[ignore]
async fn load_md_bars_for_backtest_is_deterministically_ordered() -> Result<()> {
    let Some(pool) = maybe_pool().await else {
        return Ok(());
    };

    mqk_db::migrate(&pool).await?;

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
    symbols.reverse();

    let rows = load_md_bars_for_backtest(&pool, "1m", 0, 999_999, &symbols).await?;

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].end_ts, 60);
    assert_eq!(rows[0].symbol, "A");
    assert_eq!(rows[1].end_ts, 120);
    assert_eq!(rows[1].symbol, "A");
    assert_eq!(rows[2].end_ts, 120);
    assert_eq!(rows[2].symbol, "B");

    Ok(())
}
