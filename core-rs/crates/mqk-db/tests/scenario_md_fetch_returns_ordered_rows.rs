// PATCH BT2: Deterministic md_bars READ API ordering + filtering scenario test.
//
// DB-backed test, skipped if MQK_DATABASE_URL is not set.

use anyhow::Result;

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn md_fetch_returns_ordered_rows_and_applies_filters() -> Result<()> {
    let pool = mqk_db::testkit_db_pool().await?;

    // Clean target rows.
    sqlx::query(
        r#"
        delete from md_bars
        where timeframe = '1D'
          and symbol in ('AAA1','ZZZ1')
        "#,
    )
    .execute(&pool)
    .await?;

    // Insert out-of-order rows for ZZZ1 and single row for AAA1.
    // Note: ingested_at is defaulted in schema; do not assert it.
    for (symbol, end_ts, is_complete) in [
        ("ZZZ1", 300_i64, true),
        ("ZZZ1", 100_i64, true),
        ("AAA1", 200_i64, true),
    ] {
        sqlx::query(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts,
              open_micros, high_micros, low_micros, close_micros,
              volume, is_complete
            ) values ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            on conflict (symbol, timeframe, end_ts) do update set
              open_micros = excluded.open_micros,
              high_micros = excluded.high_micros,
              low_micros = excluded.low_micros,
              close_micros = excluded.close_micros,
              volume = excluded.volume,
              is_complete = excluded.is_complete
            "#,
        )
        .bind(symbol)
        .bind("1D")
        .bind(end_ts)
        .bind(10_i64)
        .bind(11_i64)
        .bind(9_i64)
        .bind(10_i64)
        .bind(100_i64)
        .bind(is_complete)
        .execute(&pool)
        .await?;
    }

    // Unsorted input symbols must still produce deterministic ordering.
    let got = mqk_db::fetch_md_bars(
        &pool,
        mqk_db::FetchMdBarsArgs {
            timeframe: "1D".to_string(),
            symbols: vec!["ZZZ1".to_string(), "AAA1".to_string()],
            start_end_ts: None,
            end_end_ts: None,
            require_complete: false,
        },
    )
    .await?;

    let expect = vec![
        mqk_db::MdBarRow {
            symbol: "AAA1".to_string(),
            timeframe: "1D".to_string(),
            end_ts: 200,
            open_micros: 10,
            high_micros: 11,
            low_micros: 9,
            close_micros: 10,
            volume: 100,
            is_complete: true,
        },
        mqk_db::MdBarRow {
            symbol: "ZZZ1".to_string(),
            timeframe: "1D".to_string(),
            end_ts: 100,
            open_micros: 10,
            high_micros: 11,
            low_micros: 9,
            close_micros: 10,
            volume: 100,
            is_complete: true,
        },
        mqk_db::MdBarRow {
            symbol: "ZZZ1".to_string(),
            timeframe: "1D".to_string(),
            end_ts: 300,
            open_micros: 10,
            high_micros: 11,
            low_micros: 9,
            close_micros: 10,
            volume: 100,
            is_complete: true,
        },
    ];

    assert_eq!(got, expect);

    // start_end_ts filter should exclude ZZZ1@100.
    let got_filtered = mqk_db::fetch_md_bars(
        &pool,
        mqk_db::FetchMdBarsArgs {
            timeframe: "1D".to_string(),
            symbols: vec!["ZZZ1".to_string(), "AAA1".to_string()],
            start_end_ts: Some(150),
            end_end_ts: None,
            require_complete: false,
        },
    )
    .await?;

    let expect_filtered = vec![expect[0].clone(), expect[2].clone()];
    assert_eq!(got_filtered, expect_filtered);

    // require_complete=true should exclude incomplete rows.
    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts,
          open_micros, high_micros, low_micros, close_micros,
          volume, is_complete
        ) values ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        on conflict (symbol, timeframe, end_ts) do update set
          is_complete = excluded.is_complete
        "#,
    )
    .bind("ZZZ1")
    .bind("1D")
    .bind(400_i64)
    .bind(10_i64)
    .bind(11_i64)
    .bind(9_i64)
    .bind(10_i64)
    .bind(100_i64)
    .bind(false)
    .execute(&pool)
    .await?;

    let got_complete_only = mqk_db::fetch_md_bars(
        &pool,
        mqk_db::FetchMdBarsArgs {
            timeframe: "1D".to_string(),
            symbols: vec!["ZZZ1".to_string(), "AAA1".to_string()],
            start_end_ts: None,
            end_end_ts: None,
            require_complete: true,
        },
    )
    .await?;

    assert_eq!(got_complete_only, expect);

    Ok(())
}
