use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn crash_recovery_does_not_double_submit_when_broker_already_has_order() -> anyhow::Result<()>
{
    // Skip if no DB configured.
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return Ok(());
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await?;

    mqk_db::migrate(&pool).await?;

    // Create run
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({"arming": {"require_manual_confirmation": false}}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    // Outbox intent
    let idempotency_key = format!("{run_id}_client_order_001");
    let order_json = json!({"symbol":"SPY","side":"BUY","qty":1});

    let created =
        mqk_db::outbox_enqueue(&pool, run_id, &idempotency_key, order_json.clone()).await?;
    assert!(created, "outbox row must be created");

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &idempotency_key).await?;
    let row = row.expect("freshly enqueued outbox row missing");
    assert_eq!(row.status, "PENDING", "fresh enqueue must start as PENDING");

    // Dispatcher claims rows before broker submit.
    //
    // Use a batch > 1 and then locate our exact row. This makes the test robust
    // against a shared local DB that may contain unrelated pending rows from other
    // runs/tests.
    let dispatcher_id = format!("test-dispatcher-{run_id}");
    let claimed = mqk_db::outbox_claim_batch(&pool, 64, &dispatcher_id, chrono::Utc::now()).await?;

    let claimed_row = claimed
        .into_iter()
        .find(|row| row.row.idempotency_key == idempotency_key)
        .expect("dispatcher must claim the target outbox row");

    assert_eq!(
        claimed_row.row.idempotency_key, idempotency_key,
        "claimed row must match the target idempotency key"
    );

    // Simulate the "submit to broker" step happening…
    // …and then a crash BEFORE we ever mark ACKED (only SENT).
    let mut broker = mqk_testkit::FakeBroker::new();
    broker.submit(&claimed_row.row.idempotency_key, order_json.clone());
    assert_eq!(broker.submit_count(), 1);

    // Record that we attempted to send (but did NOT ack).
    let sent =
        mqk_db::outbox_mark_sent(&pool, &claimed_row.row.idempotency_key, chrono::Utc::now())
            .await?;
    assert!(sent, "outbox_mark_sent must transition claimed row to SENT");

    // "Restart" recovery: should see outbox row as SENT/unacked,
    // compare with broker state, and NOT resubmit.
    let report = mqk_testkit::recover_outbox_against_broker(&pool, run_id, &mut broker).await?;
    assert_eq!(
        report.resubmitted, 0,
        "should not resubmit if broker already has order"
    );
    assert_eq!(
        report.acked, 1,
        "should mark ACKED when broker already has order"
    );
    assert_eq!(broker.submit_count(), 1, "submit must remain exactly once");

    // DB should now show ACKED
    let row =
        mqk_db::outbox_fetch_by_idempotency_key(&pool, &claimed_row.row.idempotency_key).await?;
    let row = row.expect("outbox row missing");
    assert_eq!(row.status, "ACKED");

    Ok(())
}
