// DB-OUTBOX-SCHEMA-VERSION-01
//
// Disposable-DB proof that the schema_version stamped by the real writer
// seams (outbox_enqueue / inbox_insert_deduped) survives a genuine Postgres
// JSONB round trip and is accepted by the real reader validators. Also
// proves a row written the way this table looked BEFORE this patch (no
// schema_version key at all) still reads back as valid -- proven historical
// compatibility, not an assumption.

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

async fn connect() -> anyhow::Result<sqlx::PgPool> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;
    Ok(pool)
}

async fn seed_run(pool: &sqlx::PgPool) -> anyhow::Result<Uuid> {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({"x": 1}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;
    Ok(run_id)
}

// ---------------------------------------------------------------------------
// Outbox: writer -> reader round trip through real Postgres JSONB.
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn wr01_outbox_enqueue_stamps_current_version_and_reader_accepts_it() -> anyhow::Result<()> {
    let pool = connect().await?;
    let run_id = seed_run(&pool).await?;
    let idem = format!("{run_id}_wr01");

    mqk_db::outbox_enqueue(&pool, run_id, &idem, json!({"symbol": "SPY", "qty": 1})).await?;

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &idem)
        .await?
        .expect("row must exist");

    assert_eq!(
        row.order_json.get("schema_version").and_then(|v| v.as_i64()),
        Some(mqk_db::ORDER_JSON_SCHEMA_VERSION),
        "round-tripped order_json must carry the current schema_version"
    );
    mqk_db::validate_order_json_schema_version(&row.order_json)
        .expect("current-version row must validate");

    Ok(())
}

// ---------------------------------------------------------------------------
// Outbox: a row written the way this table looked BEFORE this patch (raw
// INSERT bypassing outbox_enqueue's stamping) must still read back valid.
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn wr02_legacy_unversioned_outbox_row_still_reads_back_valid() -> anyhow::Result<()> {
    let pool = connect().await?;
    let run_id = seed_run(&pool).await?;
    let idem = format!("{run_id}_wr02");

    // Bypass the stamping writer entirely -- this is exactly what a row
    // inserted before this patch looks like on disk.
    sqlx::query(
        r#"
        insert into oms_outbox (run_id, idempotency_key, order_json, status)
        values ($1, $2, $3, 'PENDING')
        "#,
    )
    .bind(run_id)
    .bind(&idem)
    .bind(json!({"symbol": "SPY", "qty": 1}))
    .execute(&pool)
    .await?;

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &idem)
        .await?
        .expect("row must exist");

    assert!(
        row.order_json.get("schema_version").is_none(),
        "sanity: this row really has no schema_version key"
    );
    mqk_db::validate_order_json_schema_version(&row.order_json)
        .expect("legacy unversioned row must still validate (historical compatibility)");

    Ok(())
}

// ---------------------------------------------------------------------------
// Inbox: writer -> reader round trip through real Postgres JSONB, using the
// same inbox_load_unapplied_for_run query the production apply path uses.
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn wr03_inbox_insert_stamps_current_version_and_reader_accepts_it() -> anyhow::Result<()> {
    let pool = connect().await?;
    let run_id = seed_run(&pool).await?;
    let msg_id = format!("BROKER_MSG_{run_id}_wr03");

    mqk_db::inbox_insert_deduped(
        &pool,
        run_id,
        &msg_id,
        json!({"type": "ack", "broker_message_id": msg_id, "internal_order_id": "o1", "broker_order_id": null}),
    )
    .await?;

    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    let row = unapplied
        .iter()
        .find(|r| r.broker_message_id == msg_id)
        .expect("row must be present and unapplied");

    assert_eq!(
        row.message_json.get("schema_version").and_then(|v| v.as_i64()),
        Some(mqk_db::MESSAGE_JSON_SCHEMA_VERSION),
        "round-tripped message_json must carry the current schema_version"
    );
    mqk_db::validate_message_json_schema_version(&row.message_json)
        .expect("current-version row must validate");

    Ok(())
}

// ---------------------------------------------------------------------------
// Outbox: the REAL public writer must refuse a caller-supplied future
// schema_version rather than silently rewriting it down to current, and
// must not create any row when it refuses.
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn wr05_outbox_enqueue_refuses_future_schema_version_and_writes_nothing() -> anyhow::Result<()>
{
    let pool = connect().await?;
    let run_id = seed_run(&pool).await?;
    let idem = format!("{run_id}_wr05");

    let result = mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &idem,
        json!({"symbol": "SPY", "qty": 1, "schema_version": mqk_db::ORDER_JSON_SCHEMA_VERSION + 1}),
    )
    .await;
    assert!(result.is_err(), "future schema_version must be refused");

    assert!(
        mqk_db::outbox_fetch_by_idempotency_key(&pool, &idem)
            .await?
            .is_none(),
        "refused write must not create a row"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Outbox: the REAL public writer must refuse a malformed schema_version.
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn wr06_outbox_enqueue_refuses_malformed_schema_version_and_writes_nothing()
-> anyhow::Result<()> {
    let pool = connect().await?;
    let run_id = seed_run(&pool).await?;
    let idem = format!("{run_id}_wr06");

    let result = mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &idem,
        json!({"symbol": "SPY", "qty": 1, "schema_version": "one"}),
    )
    .await;
    assert!(result.is_err(), "malformed schema_version must be refused");

    assert!(
        mqk_db::outbox_fetch_by_idempotency_key(&pool, &idem)
            .await?
            .is_none(),
        "refused write must not create a row"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Outbox: the REAL public writer must refuse a non-object order_json
// envelope.
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn wr07_outbox_enqueue_refuses_non_object_order_json_and_writes_nothing() -> anyhow::Result<()>
{
    let pool = connect().await?;
    let run_id = seed_run(&pool).await?;
    let idem = format!("{run_id}_wr07");

    let result = mqk_db::outbox_enqueue(&pool, run_id, &idem, json!(["not", "an", "object"])).await;
    assert!(result.is_err(), "non-object order_json must be refused");

    assert!(
        mqk_db::outbox_fetch_by_idempotency_key(&pool, &idem)
            .await?
            .is_none(),
        "refused write must not create a row"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Inbox: the REAL public writer must refuse a caller-supplied future
// schema_version and must not create any row when it refuses.
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn wr08_inbox_insert_refuses_future_schema_version_and_writes_nothing() -> anyhow::Result<()> {
    let pool = connect().await?;
    let run_id = seed_run(&pool).await?;
    let msg_id = format!("BROKER_MSG_{run_id}_wr08");

    let result = mqk_db::inbox_insert_deduped(
        &pool,
        run_id,
        &msg_id,
        json!({
            "type": "ack",
            "broker_message_id": msg_id,
            "internal_order_id": "o1",
            "broker_order_id": null,
            "schema_version": mqk_db::MESSAGE_JSON_SCHEMA_VERSION + 1,
        }),
    )
    .await;
    assert!(result.is_err(), "future schema_version must be refused");

    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert!(
        !unapplied.iter().any(|r| r.broker_message_id == msg_id),
        "refused write must not create a row"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Inbox: a row written the way this table looked BEFORE this patch must
// still read back valid.
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn wr04_legacy_unversioned_inbox_row_still_reads_back_valid() -> anyhow::Result<()> {
    let pool = connect().await?;
    let run_id = seed_run(&pool).await?;
    let msg_id = format!("BROKER_MSG_{run_id}_wr04");

    sqlx::query(
        r#"
        insert into oms_inbox (
            run_id, broker_message_id, broker_fill_id, internal_order_id,
            broker_order_id, event_kind, message_json, event_ts_ms,
            received_at_utc, applied_at_utc
        )
        values ($1, $2, null, $3, $3, 'ack', $4, 0, now(), null)
        "#,
    )
    .bind(run_id)
    .bind(&msg_id)
    .bind("o1")
    .bind(json!({"type": "ack"}))
    .execute(&pool)
    .await?;

    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    let row = unapplied
        .iter()
        .find(|r| r.broker_message_id == msg_id)
        .expect("row must be present and unapplied");

    assert!(
        row.message_json.get("schema_version").is_none(),
        "sanity: this row really has no schema_version key"
    );
    mqk_db::validate_message_json_schema_version(&row.message_json)
        .expect("legacy unversioned row must still validate (historical compatibility)");

    Ok(())
}
