use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;
use sqlx::PgPool;

/// Minimal fake broker used ONLY for tests.
/// Enforces idempotency by idempotency_key: repeated submit is treated as a no-op.
#[derive(Default)]
pub struct FakeBroker {
    orders: HashMap<String, Value>,
    submits: usize,
}

impl FakeBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Submit an order. If idempotency_key already exists, this is a no-op.
    pub fn submit(&mut self, idempotency_key: &str, order_json: Value) {
        if self.orders.contains_key(idempotency_key) {
            return;
        }
        self.orders.insert(idempotency_key.to_string(), order_json);
        self.submits += 1;
    }

    pub fn has_order(&self, idempotency_key: &str) -> bool {
        self.orders.contains_key(idempotency_key)
    }

    pub fn submit_count(&self) -> usize {
        self.submits
    }
}

#[derive(Debug, Clone)]
pub struct RecoveryReport {
    pub inspected: usize,
    pub acked: usize,
    pub resubmitted: usize,
}

/// Recovery logic against a broker snapshot/adapter.
/// This is the minimal deterministic behavior required for Patch 19B:
/// - If broker already has the order (by idempotency_key), mark outbox ACKED.
/// - If broker does NOT have it, resubmit exactly once (broker is idempotent) and mark ACKED.
///
/// NOTE: This does not implement retries/backoff or polling loops.
/// It’s a single-shot "restart reconciliation" primitive.
pub async fn recover_outbox_against_broker(
    pool: &PgPool,
    run_id: Uuid,
    broker: &mut FakeBroker,
) -> Result<RecoveryReport> {
    let rows = mqk_db::outbox_list_unacked_for_run(pool, run_id).await?;

    let mut report = RecoveryReport {
        inspected: rows.len(),
        acked: 0,
        resubmitted: 0,
    };

    for r in rows {
        let key = r.idempotency_key.clone();

        if broker.has_order(&key) {
            // Broker already has it => do not resubmit; just ACK it locally.
            let _ = mqk_db::outbox_mark_acked(pool, &key).await?;
            report.acked += 1;
            continue;
        }

        // Broker does not have it => submit missing once, then ACK.
        broker.submit(&key, r.order_json.clone());
        report.resubmitted += 1;

        let _ = mqk_db::outbox_mark_acked(pool, &key).await?;
        report.acked += 1;
    }

    Ok(report)
}
