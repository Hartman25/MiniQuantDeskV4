use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub outbox_id: i64,
    pub run_id: Uuid,
    pub idempotency_key: String,
    pub order_json: Value,
    pub status: String, // PENDING | CLAIMED | DISPATCHING | SENT | ACKED | FAILED
    pub created_at_utc: DateTime<Utc>,
    pub sent_at_utc: Option<DateTime<Utc>>,
    pub claimed_at_utc: Option<DateTime<Utc>>,
    pub claimed_by: Option<String>,
    /// RT-5: timestamp written before gateway.submit(); null until DISPATCHING.
    pub dispatching_at_utc: Option<DateTime<Utc>>,
    /// RT-5: dispatcher identity written before gateway.submit(); null until DISPATCHING.
    pub dispatch_attempt_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousOutboxRow {
    pub idempotency_key: String,
    pub status: String, // AMBIGUOUS | DISPATCHING | SENT (without broker map)
    pub broker_order_id: Option<String>,
}

/// Load restart-ambiguous outbox rows for a run.
///
/// Policy (A4):
/// - `AMBIGUOUS` is always quarantined: `BrokerError::AmbiguousSubmit` was
///   returned, meaning the broker may or may not have accepted the order.
///   These rows can only exit quarantine via `outbox_reset_ambiguous_to_pending`
///   (explicit operator/reconcile-proof release).
/// - `DISPATCHING` is always ambiguous on restart: broker submit may have
///   been attempted, but the process died before closure.
/// - `SENT` is ambiguous only when the broker-order map is still missing.
///   A normal healthy `SENT` row with a broker map entry must NOT be
///   quarantined every tick, otherwise the system would halt during
///   ordinary pre-ACK operation.
///
/// This helper therefore returns only rows that are unsafe to continue past
/// restart without operator intervention.
pub async fn outbox_load_restart_ambiguous_for_run(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Vec<AmbiguousOutboxRow>> {
    let rows = sqlx::query(
        r#"
        select
            o.idempotency_key,
            o.status,
            m.broker_id as broker_order_id
        from oms_outbox o
        left join broker_order_map m
          on m.internal_id = o.idempotency_key
        where o.run_id = $1
          and (
                o.status = 'AMBIGUOUS'
                or o.status = 'DISPATCHING'
                or (
                    o.status = 'SENT'
                    and m.broker_id is null
                )
          )
        order by o.outbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("outbox_load_restart_ambiguous_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(AmbiguousOutboxRow {
            idempotency_key: row.try_get("idempotency_key")?,
            status: row.try_get("status")?,
            broker_order_id: row.try_get("broker_order_id")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// OutboxClaimToken (FC-2)
// ---------------------------------------------------------------------------

/// Unforgeable proof that an outbox row has been claimed via
/// [`outbox_claim_batch`].
///
/// # Forgeability
///
/// The `_priv` field is `pub(crate)`, preventing struct-literal construction
/// outside this crate. The only `pub(crate)` constructor (`OutboxClaimToken::new`)
/// is called exclusively inside `outbox_claim_batch`, which atomically performs
/// `FOR UPDATE SKIP LOCKED` — the DB lock IS the proof.
///
/// External code may name this type (needed to implement `BrokerAdapter` and
/// call `BrokerGateway::submit`) but cannot construct it. In production, the
/// only way to obtain a token is through `outbox_claim_batch`. In tests,
/// [`OutboxClaimToken::for_test`] is available as an explicit escape hatch.
///
/// ```text
/// ✅  let claimed = outbox_claim_batch(&pool, …).await?;   // production path
///     let token = &claimed[0].token;
/// ✅  OutboxClaimToken::for_test(id, key)                  // tests only
/// ❌  OutboxClaimToken { _priv: (), … }                    // ERROR: private field
/// ```
#[allow(clippy::manual_non_exhaustive)]
#[derive(Debug, Clone)]
pub struct OutboxClaimToken {
    /// The DB row ID of the claimed outbox entry.
    pub outbox_id: i64,
    /// The idempotency key (`client_order_id`) of the claimed outbox entry.
    pub idempotency_key: String,
    /// Prevents struct-literal construction outside this crate (FC-2).
    pub(crate) _priv: (),
}

impl OutboxClaimToken {
    /// Construct a claim token from a successfully claimed outbox row.
    ///
    /// `pub(crate)` — only callable inside `mqk-db`. Callers outside this
    /// crate must obtain tokens via [`outbox_claim_batch`].
    ///
    /// # Compile-time gate
    ///
    /// Compiled only when at least one of the following is active:
    /// - `test` — for the `for_test` escape hatch used in unit tests
    /// - `feature = "runtime-claim"` — for `outbox_claim_batch` (production path)
    /// - `feature = "testkit"` — for integration test infrastructure
    ///
    /// In a plain `cargo build` / `cargo clippy` without any of these, this
    /// function is not present and cannot be called — enforcing the RT-1 gate.
    #[cfg(any(test, feature = "runtime-claim", feature = "testkit"))]
    pub(crate) fn new(outbox_id: i64, idempotency_key: impl Into<String>) -> Self {
        Self {
            outbox_id,
            idempotency_key: idempotency_key.into(),
            _priv: (),
        }
    }

    /// Test-only escape hatch. Do NOT call from production code.
    ///
    /// # Compile-time gate
    ///
    /// This function is compiled only when:
    /// - `#[cfg(test)]` is active (i.e., the **owning crate** is being tested
    ///   via `cargo test -p mqk-db`), OR
    /// - the `testkit` Cargo feature is explicitly enabled.
    ///
    /// The `testkit` feature MUST NOT be listed in any production crate's
    /// `[dependencies]` — only in `[dev-dependencies]` of test/testkit crates.
    ///
    /// In production, tokens are returned exclusively by [`outbox_claim_batch`],
    /// coupling each token to a real DB-level `FOR UPDATE SKIP LOCKED` row
    /// lock. This function bypasses that guarantee and exists solely for unit
    /// and integration test setup.
    #[doc(hidden)]
    #[cfg(any(test, feature = "testkit"))]
    pub fn for_test(outbox_id: i64, idempotency_key: impl Into<String>) -> Self {
        Self::new(outbox_id, idempotency_key)
    }
}

/// Return type of [`outbox_claim_batch`].
///
/// Bundles the claimed [`OutboxRow`] with its [`OutboxClaimToken`], ensuring
/// the token is always paired with the row that generated it.
///
/// # Availability
///
/// Gated behind `feature = "runtime-claim"` (production) or `feature = "testkit"`
/// (tests). See RT-1.
// RT-1: single-dispatcher boundary. Only mqk-runtime (runtime-claim feature) and
// test infrastructure (testkit feature) may use this type. Daemon and CLI must
// not depend on mqk-db with either feature active.
#[cfg(any(feature = "runtime-claim", feature = "testkit"))]
#[derive(Debug, Clone)]
pub struct ClaimedOutboxRow {
    /// The claimed outbox row (status = `CLAIMED`).
    pub row: OutboxRow,
    /// Unforgeable proof of the DB claim. Pass to `BrokerGateway::submit`.
    pub token: OutboxClaimToken,
}

#[derive(Debug, Clone)]
pub struct InboxRow {
    pub inbox_id: i64,
    pub run_id: Uuid,
    pub broker_message_id: String,
    pub broker_fill_id: Option<String>,
    pub broker_sequence_id: Option<String>,
    pub broker_timestamp: Option<String>,
    pub message_json: Value,
    pub received_at_utc: DateTime<Utc>,
    /// NULL until inbox_mark_applied() is called after a successful portfolio
    /// apply.  Rows with applied_at_utc IS NULL are returned by
    /// inbox_load_unapplied_for_run() for crash-recovery replay (Patch D2).
    pub applied_at_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct BrokerEventIdentity {
    pub broker_message_id: String,
    pub broker_fill_id: Option<String>,
    pub broker_sequence_id: Option<String>,
    pub broker_timestamp: Option<String>,
}

/// Enqueue an order intent into oms_outbox.
///
/// Idempotent behavior:
/// - If idempotency_key already exists, returns Ok(false) and does NOT create a second row.
/// - If inserted, returns Ok(true).
///
/// This matches the allocator-grade requirement: restarts cannot double-submit.
pub async fn outbox_enqueue(
    pool: &PgPool,
    run_id: Uuid,
    idempotency_key: &str,
    order_json: Value,
) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        insert into oms_outbox (run_id, idempotency_key, order_json, status)
        values ($1, $2, $3, 'PENDING')
        on conflict (idempotency_key) do nothing
        returning outbox_id
        "#,
    )
    .bind(run_id)
    .bind(idempotency_key)
    .bind(order_json)
    .fetch_optional(pool)
    .await
    .context("outbox_enqueue failed")?;

    Ok(row.is_some())
}

/// Atomically claim up to `batch_size` PENDING outbox rows for exclusive dispatch.
///
/// Uses `FOR UPDATE SKIP LOCKED` so concurrent dispatchers never claim the same row.
/// Returns [`ClaimedOutboxRow`]s, each containing the claimed [`OutboxRow`] **and**
/// an [`OutboxClaimToken`] constructed from the DB row — coupling the token to the
/// actual lock (FC-2). Returns an empty `Vec` if no `PENDING` rows are available.
///
/// The caller MUST:
/// - call `outbox_mark_dispatching` immediately before `gateway.submit()`, THEN
/// - call `outbox_mark_sent` after a successful submit (DISPATCHING → SENT), OR
/// - call `outbox_mark_failed` on submit failure (row quarantined as FAILED).
///
/// `outbox_release_claim` (CLAIMED → PENDING) is only valid while the row is
/// still CLAIMED — i.e. before `outbox_mark_dispatching` is called.
///
/// # Availability — RT-1 single-dispatcher gate
///
/// This function is only compiled when `feature = "runtime-claim"` (enabled
/// exclusively by `mqk-runtime`) or `feature = "testkit"` (test infrastructure)
/// is active. Daemon and CLI crates must NOT enable either feature; any attempt
/// to call this function from those crates produces `error[E0425]` at compile time.
// RT-1: gate enforced here. Do not remove without updating the prover.
#[cfg(any(feature = "runtime-claim", feature = "testkit"))]
async fn outbox_claim_batch_inner(
    pool: &PgPool,
    run_id: Option<Uuid>,
    batch_size: i64,
    dispatcher_id: &str,
    claimed_at: DateTime<Utc>,
) -> Result<Vec<ClaimedOutboxRow>> {
    let rows = if let Some(run_id) = run_id {
        sqlx::query(
            r#"
            with to_claim as (
                select outbox_id
                from oms_outbox
                where run_id = $2
                  and status = 'PENDING'
                order by outbox_id asc
                limit $1
                for update skip locked
            )
            update oms_outbox
               set status         = 'CLAIMED',
                   claimed_at_utc = $4,
                   claimed_by     = $3
             where outbox_id in (select outbox_id from to_claim)
            returning outbox_id, run_id, idempotency_key, order_json, status,
                      created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
                      dispatching_at_utc, dispatch_attempt_id
            "#,
        )
        .bind(batch_size)
        .bind(run_id)
        .bind(dispatcher_id)
        .bind(claimed_at)
        .fetch_all(pool)
        .await
        .context("outbox_claim_batch_for_run failed")?
    } else {
        sqlx::query(
            r#"
            with to_claim as (
                select outbox_id
                from oms_outbox
                where status = 'PENDING'
                order by outbox_id asc
                limit $1
                for update skip locked
            )
            update oms_outbox
               set status         = 'CLAIMED',
                   claimed_at_utc = $3,
                   claimed_by     = $2
             where outbox_id in (select outbox_id from to_claim)
            returning outbox_id, run_id, idempotency_key, order_json, status,
                      created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
                      dispatching_at_utc, dispatch_attempt_id
            "#,
        )
        .bind(batch_size)
        .bind(dispatcher_id)
        .bind(claimed_at)
        .fetch_all(pool)
        .await
        .context("outbox_claim_batch failed")?
    };

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let outbox_row = OutboxRow {
            outbox_id: row.try_get("outbox_id")?,
            run_id: row.try_get("run_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
            order_json: row.try_get("order_json")?,
            status: row.try_get("status")?,
            created_at_utc: row.try_get("created_at_utc")?,
            sent_at_utc: row.try_get("sent_at_utc")?,
            claimed_at_utc: row.try_get("claimed_at_utc")?,
            claimed_by: row.try_get("claimed_by")?,
            dispatching_at_utc: row.try_get("dispatching_at_utc")?,
            dispatch_attempt_id: row.try_get("dispatch_attempt_id")?,
        };
        let token = OutboxClaimToken::new(outbox_row.outbox_id, &outbox_row.idempotency_key);
        out.push(ClaimedOutboxRow {
            row: outbox_row,
            token,
        });
    }
    Ok(out)
}

#[cfg(any(feature = "runtime-claim", feature = "testkit"))]
pub async fn outbox_claim_batch(
    pool: &PgPool,
    batch_size: i64,
    dispatcher_id: &str,
    claimed_at: DateTime<Utc>,
) -> Result<Vec<ClaimedOutboxRow>> {
    outbox_claim_batch_inner(pool, None, batch_size, dispatcher_id, claimed_at).await
}

#[cfg(any(feature = "runtime-claim", feature = "testkit"))]
pub async fn outbox_claim_batch_for_run(
    pool: &PgPool,
    run_id: Uuid,
    batch_size: i64,
    dispatcher_id: &str,
    claimed_at: DateTime<Utc>,
) -> Result<Vec<ClaimedOutboxRow>> {
    outbox_claim_batch_inner(pool, Some(run_id), batch_size, dispatcher_id, claimed_at).await
}

/// Release a CLAIMED row back to PENDING.
///
/// Called when a dispatcher fails before broker submit and wants to relinquish
/// its claim so another dispatcher (or a future retry) can pick it up.
/// Returns true if the row was CLAIMED and is now PENDING; false otherwise.
pub async fn outbox_release_claim(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status         = 'PENDING',
               claimed_at_utc = null,
               claimed_by     = null
         where idempotency_key = $1
           and status = 'CLAIMED'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_release_claim failed")?;

    Ok(row.is_some())
}

/// RT-5: Advance a CLAIMED outbox row to DISPATCHING immediately before calling
/// `gateway.submit()`.
///
/// Writing DISPATCHING before the broker call closes the W4 crash window:
/// `outbox_reset_stale_claims` only resets `CLAIMED` rows — a crash between
/// `outbox_mark_dispatching` and `outbox_mark_sent` leaves the row in
/// `DISPATCHING`, preventing silent requeue and double-submit on restart.
///
/// `dispatching_at` is caller-supplied (no SQL `now()` — FC-7 policy).
/// `dispatch_attempt_id` identifies which dispatcher instance was in-flight;
/// used for crash-recovery audit.
///
/// Returns `true` if the row transitioned `CLAIMED → DISPATCHING`; `false` if
/// not found or not in `CLAIMED` state.
pub async fn outbox_mark_dispatching(
    pool: &PgPool,
    idempotency_key: &str,
    dispatch_attempt_id: &str,
    dispatching_at: DateTime<Utc>,
) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status              = 'DISPATCHING',
               dispatching_at_utc  = $3,
               dispatch_attempt_id = $2
         where idempotency_key = $1
           and status = 'CLAIMED'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .bind(dispatch_attempt_id)
    .bind(dispatching_at)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_dispatching failed")?;

    Ok(row.is_some())
}

/// Reset stale CLAIMED rows back to PENDING — the crash-recovery reaper (FC-6).
///
/// Called on orchestrator startup (and optionally on a periodic sweep) to
/// recover rows left in CLAIMED state by a crashed or stuck dispatcher.
///
/// A row is considered stale when its `claimed_at_utc` is strictly earlier
/// than `stale_threshold`.  The threshold is caller-supplied — no wall-clock
/// inside this function (FC-5 policy).  In production, pass
/// `time_source.now_utc() - stale_duration`; in tests, pass an explicit
/// timestamp.
///
/// Returns the number of rows reset.  Only `CLAIMED` rows are affected.
/// Terminal states (`SENT`, `ACKED`, `FAILED`) and `PENDING` rows are never
/// modified.
pub async fn outbox_reset_stale_claims(
    pool: &PgPool,
    stale_threshold: DateTime<Utc>,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        update oms_outbox
           set status         = 'PENDING',
               claimed_at_utc = null,
               claimed_by     = null
         where status         = 'CLAIMED'
           and claimed_at_utc < $1
        "#,
    )
    .bind(stale_threshold)
    .execute(pool)
    .await
    .context("outbox_reset_stale_claims failed")?;

    Ok(result.rows_affected())
}

/// Fetch a single outbox row by idempotency_key.
pub async fn outbox_fetch_by_idempotency_key(
    pool: &PgPool,
    idempotency_key: &str,
) -> Result<Option<OutboxRow>> {
    let row = sqlx::query(
        r#"
        select outbox_id, run_id, idempotency_key, order_json, status,
               created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
               dispatching_at_utc, dispatch_attempt_id
        from oms_outbox
        where idempotency_key = $1
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_fetch_by_idempotency_key failed")?;

    let Some(row) = row else { return Ok(None) };

    Ok(Some(OutboxRow {
        outbox_id: row.try_get("outbox_id")?,
        run_id: row.try_get("run_id")?,
        idempotency_key: row.try_get("idempotency_key")?,
        order_json: row.try_get("order_json")?,
        status: row.try_get("status")?,
        created_at_utc: row.try_get("created_at_utc")?,
        sent_at_utc: row.try_get("sent_at_utc")?,
        claimed_at_utc: row.try_get("claimed_at_utc")?,
        claimed_by: row.try_get("claimed_by")?,
        dispatching_at_utc: row.try_get("dispatching_at_utc")?,
        dispatch_attempt_id: row.try_get("dispatch_attempt_id")?,
    }))
}

/// Atomically persist `internal_id → broker_id` and transition the outbox row
/// to `SENT`.
///
/// This closes the Patch 3A durability gap:
/// the system must not durably acknowledge dispatch (`SENT`) without also
/// durably persisting the broker order ID mapping needed for restart recovery.
///
/// Transaction semantics:
/// - upsert `(internal_id, broker_id)` into `broker_order_map`
/// - transition `oms_outbox` row to `SENT`
/// - commit only if both steps succeed
///
/// Returns `true` if the outbox row transitioned to `SENT`; `false` if not
/// found or not in an acceptable pre-SENT state. If the outbox transition does
/// not occur, the transaction is not committed, so the broker map upsert is
/// rolled back as well.
///
/// Accepts both `CLAIMED` and `DISPATCHING`:
/// - Production path (RT-5): `DISPATCHING → SENT`
/// - Legacy test path: `CLAIMED → SENT`
pub async fn outbox_mark_sent_with_broker_map(
    pool: &PgPool,
    internal_id: &str,
    broker_id: &str,
    sent_at: DateTime<Utc>,
) -> Result<bool> {
    let mut tx = pool
        .begin()
        .await
        .context("outbox_mark_sent_with_broker_map begin failed")?;

    sqlx::query(
        r#"
        insert into broker_order_map (internal_id, broker_id)
        values ($1, $2)
        on conflict (internal_id) do update
            set broker_id = excluded.broker_id
        "#,
    )
    .bind(internal_id)
    .bind(broker_id)
    .execute(&mut *tx)
    .await
    .context("outbox_mark_sent_with_broker_map broker_map_upsert failed")?;

    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status      = 'SENT',
               sent_at_utc = coalesce(sent_at_utc, $2)
         where idempotency_key = $1
           and status in ('CLAIMED', 'DISPATCHING')
        returning outbox_id
        "#,
    )
    .bind(internal_id)
    .bind(sent_at)
    .fetch_optional(&mut *tx)
    .await
    .context("outbox_mark_sent_with_broker_map outbox_mark_sent failed")?;

    let Some((_outbox_id,)) = row else {
        return Ok(false);
    };

    tx.commit()
        .await
        .context("outbox_mark_sent_with_broker_map commit failed")?;

    Ok(true)
}

/// Mark an outbox row as ACKED.
/// Returns true if transitioned, false if not found.
pub async fn outbox_mark_acked(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    // ACK closure is valid for both:
    // - SENT → ACKED        (submit lifecycle after broker map persistence)
    // - DISPATCHING → ACKED (non-submit actions like cancel that do not create
    //                        a SENT/broker-map phase of their own)
    // Any other predecessor is an explicit protocol violation and must return
    // Err, not a silent Ok(false).
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status = 'ACKED'
         where idempotency_key = $1
           and status in ('SENT', 'DISPATCHING')
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_acked failed")?;

    if row.is_some() {
        return Ok(true);
    }

    // Row was not updated.  Distinguish "already ACKED" (idempotent ok) from
    // "wrong predecessor state" (protocol violation → Err).
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT status FROM oms_outbox WHERE idempotency_key = $1")
            .bind(idempotency_key)
            .fetch_optional(pool)
            .await
            .context("outbox_mark_acked status check failed")?;

    match existing {
        Some((status,)) if status == "ACKED" => Ok(false), // already acked; idempotent
        Some((status,)) => Err(anyhow!(
            "outbox_mark_acked: invalid transition from {status} to ACKED \
             (only SENT or DISPATCHING → ACKED is valid)"
        )),
        None => Ok(false), // row not found; caller can treat as no-op
    }
}

/// Mark a CLAIMED or DISPATCHING outbox row as FAILED.
///
/// Returns true if a row transitioned to FAILED; false otherwise.
/// Accepts both `CLAIMED` and `DISPATCHING` — use `outbox_claim_batch` first.
/// After RT-5, the production submit-failure path calls this with a DISPATCHING row.
pub async fn outbox_mark_failed(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status = 'FAILED'
         where idempotency_key = $1
           and status in ('CLAIMED', 'DISPATCHING')
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_failed failed")?;

    Ok(row.is_some())
}

/// Reset a `DISPATCHING` row back to `PENDING` for safe retry.
///
/// Used by the orchestrator when the broker adapter returns a retryable error
/// (`Transport` or `RateLimit`) — i.e., the request provably never reached the
/// broker.  Clears the claim fields so `outbox_claim_batch` can re-claim the
/// row on the next tick.
///
/// Returns `true` if the row was reset; `false` if not found or not
/// `DISPATCHING`.
pub async fn outbox_reset_dispatching_to_pending(
    pool: &PgPool,
    idempotency_key: &str,
) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status                 = 'PENDING',
               claimed_by             = null,
               claimed_at_utc         = null,
               dispatching_at_utc     = null,
               dispatch_attempt_id    = null
         where idempotency_key = $1
           and status = 'DISPATCHING'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_reset_dispatching_to_pending failed")?;

    Ok(row.is_some())
}

/// A4: Transition a DISPATCHING outbox row to AMBIGUOUS explicit quarantine.
///
/// Called when `BrokerError::AmbiguousSubmit` is returned by the broker
/// adapter: the submit reached the broker transport layer but the outcome
/// is definitively unknown (timeout after send, partial ACK, connection drop
/// between send and receive).
///
/// Unlike `DISPATCHING` (which is also written for rows that crashed mid-
/// dispatch), `AMBIGUOUS` explicitly encodes "broker confirmed: outcome
/// unknown". It is structurally prevented from re-entering normal dispatch:
/// - `outbox_claim_batch` only claims `PENDING` rows — `AMBIGUOUS` is skipped.
/// - `outbox_load_restart_ambiguous_for_run` always returns `AMBIGUOUS` rows.
/// - The only exit is `outbox_reset_ambiguous_to_pending`.
///
/// Returns `true` if the row transitioned `DISPATCHING → AMBIGUOUS`; `false`
/// if not found or not in `DISPATCHING` state.
pub async fn outbox_mark_ambiguous(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status = 'AMBIGUOUS'
         where idempotency_key = $1
           and status = 'DISPATCHING'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_ambiguous failed")?;

    Ok(row.is_some())
}

/// A4: Release an AMBIGUOUS outbox row back to PENDING.
///
/// This is the ONLY safe path to re-enable dispatch for an order that was
/// quarantined by `outbox_mark_ambiguous`. It MUST only be called after:
/// - reconcile proof confirms the order was NOT accepted by the broker, OR
/// - an operator has verified the broker state and confirmed no live order
///   for this `idempotency_key` exists at the broker.
///
/// Clears all claim/dispatch metadata so `outbox_claim_batch` can re-claim
/// the row on the next tick after the run is re-armed.
///
/// Returns `true` if the row was released; `false` if not found or not in
/// `AMBIGUOUS` state (safe: calling this on a non-AMBIGUOUS row is a no-op).
pub async fn outbox_reset_ambiguous_to_pending(
    pool: &PgPool,
    idempotency_key: &str,
) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status              = 'PENDING',
               claimed_by          = null,
               claimed_at_utc      = null,
               dispatching_at_utc  = null,
               dispatch_attempt_id = null
         where idempotency_key = $1
           and status = 'AMBIGUOUS'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_reset_ambiguous_to_pending failed")?;

    Ok(row.is_some())
}

/// Recovery query: list outbox rows that are not terminal (not ACKED).
///
/// Includes PENDING, CLAIMED, DISPATCHING, SENT, FAILED, and AMBIGUOUS rows —
/// all statuses that indicate the order has not yet been confirmed by the broker.
///
/// NOTE: This does NOT talk to broker yet.
/// It provides the minimal deterministic input required for a future reconcile step.
pub async fn outbox_list_unacked_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<OutboxRow>> {
    let rows = sqlx::query(
        r#"
        select outbox_id, run_id, idempotency_key, order_json, status,
               created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
               dispatching_at_utc, dispatch_attempt_id
        from oms_outbox
        where run_id = $1
          and status in ('PENDING','CLAIMED','DISPATCHING','SENT','FAILED','AMBIGUOUS')
        order by outbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("outbox_list_unacked_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(OutboxRow {
            outbox_id: row.try_get("outbox_id")?,
            run_id: row.try_get("run_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
            order_json: row.try_get("order_json")?,
            status: row.try_get("status")?,
            created_at_utc: row.try_get("created_at_utc")?,
            sent_at_utc: row.try_get("sent_at_utc")?,
            claimed_at_utc: row.try_get("claimed_at_utc")?,
            claimed_by: row.try_get("claimed_by")?,
            dispatching_at_utc: row.try_get("dispatching_at_utc")?,
            dispatch_attempt_id: row.try_get("dispatch_attempt_id")?,
        });
    }
    Ok(out)
}

/// Insert a broker message/fill into oms_inbox with dedupe on (run_id, broker_message_id).
///
/// Idempotent behavior:
/// - If (run_id, broker_message_id) already exists, returns Ok(false) and does NOT create a
///   second row.
/// - If inserted, returns Ok(true).
///
/// RT-3: dedupe is scoped to the run — the same broker_message_id can appear in different
/// runs without collision (broker IDs are only unique within a session).
///
/// Patch D2 caller contract:
/// ```text
/// let inserted = inbox_insert_deduped(pool, run_id, msg_id, json).await?;
/// if inserted {
///     apply_fill_to_portfolio(json);                   // idempotent apply
///     inbox_mark_applied(pool, run_id, msg_id).await?; // journal completion
/// }
/// ```
/// On crash between insert and mark_applied: the row surfaces in
/// `inbox_load_unapplied_for_run` for recovery replay.
pub async fn inbox_insert_deduped(
    pool: &PgPool,
    run_id: Uuid,
    broker_message_id: &str,
    message_json: serde_json::Value,
) -> Result<bool> {
    // Legacy compatibility shim:
    // older callers only provide (run_id, broker_message_id, message_json).
    // Derive the richer identity fields best-effort from the payload, then
    // delegate to the canonical insert path.

    let broker_fill_id = message_json.get("broker_fill_id").and_then(|v| v.as_str());

    let internal_order_id = message_json
        .get("internal_order_id")
        .or_else(|| message_json.get("order_id"))
        .or_else(|| message_json.get("client_order_id"))
        .and_then(|v| v.as_str())
        .unwrap_or(broker_message_id);

    let broker_order_id = message_json
        .get("broker_order_id")
        .or_else(|| message_json.get("order_id"))
        .or_else(|| message_json.get("client_order_id"))
        .and_then(|v| v.as_str())
        .unwrap_or(internal_order_id);

    let event_kind = message_json
        .get("event_kind")
        .or_else(|| message_json.get("kind"))
        .or_else(|| message_json.get("event_type"))
        .or_else(|| message_json.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN");

    let event_ts_ms = message_json
        .get("event_ts_ms")
        .or_else(|| message_json.get("ts_ms"))
        .or_else(|| message_json.get("timestamp_ms"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let received_at = message_json
        .get("received_at_utc")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| DateTime::<Utc>::from_timestamp_millis(event_ts_ms)) // allow: ops-metadata — parsing stored event millis, not a wall-clock read
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);

    inbox_insert_deduped_with_identity(
        pool,
        run_id,
        broker_message_id,
        broker_fill_id,
        internal_order_id,
        broker_order_id,
        event_kind,
        &message_json,
        event_ts_ms,
        received_at,
    )
    .await
}

/// Insert a broker message/fill into oms_inbox with explicit identity fields.
///
/// Dedupe rule is transport-only and explicit:
/// - conflict key: `(run_id, broker_message_id)`
/// - `broker_fill_id` is optional economic identity metadata and does NOT
///   participate in inbox insertion dedupe.
#[allow(clippy::too_many_arguments)]
pub async fn inbox_insert_deduped_with_identity(
    pool: &PgPool,
    run_id: Uuid,
    broker_message_id: &str,
    broker_fill_id: Option<&str>,
    internal_order_id: &str,
    broker_order_id: &str,
    event_kind: &str,
    event_json: &serde_json::Value,
    event_ts_ms: i64,
    received_at: DateTime<Utc>,
) -> Result<bool> {
    let insert_result = sqlx::query(
        r#"
        insert into oms_inbox (
            run_id,
            broker_message_id,
            broker_fill_id,
            internal_order_id,
            broker_order_id,
            event_kind,
            message_json,
            event_ts_ms,
            received_at_utc,
            applied_at_utc
        )
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, null)
        "#,
    )
    .bind(run_id)
    .bind(broker_message_id)
    .bind(broker_fill_id)
    .bind(internal_order_id)
    .bind(broker_order_id)
    .bind(event_kind)
    .bind(event_json)
    .bind(event_ts_ms)
    .bind(received_at)
    .execute(pool)
    .await;

    match insert_result {
        Ok(done) => Ok(done.rows_affected() == 1),

        Err(sqlx::Error::Database(db_err))
            if db_err.code().as_deref() == Some("23505")
                && matches!(
                    db_err.constraint(),
                    Some("uq_inbox_run_broker_message_id")
                        | Some("uq_inbox_run_message")
                        | Some("uq_inbox_run_broker_fill_id")
                ) =>
        {
            Ok(false)
        }

        Err(e) => Err(e).context("inbox_insert_deduped_with_identity failed"),
    }
}

/// Stamp `applied_at_utc` on an inbox row after its fill has been
/// successfully applied to in-process portfolio state.
///
/// Part of the Patch D2 crash-recovery contract:
/// - Call this immediately after the portfolio apply completes.
/// - Rows where `applied_at_utc IS NULL` appear in
///   `inbox_load_unapplied_for_run` and must be replayed at startup.
///
/// RT-3: `run_id` is now required — dedupe is scoped to (run_id, broker_message_id).
///
/// `applied_at` is caller-supplied — no SQL `now()` in this function (FC-8
/// policy: wall-clock excluded from the fill-apply path).  In production,
/// pass `time_source.now_utc()`; in tests, pass an explicit timestamp.
///
/// Idempotent: silently succeeds if (run_id, broker_message_id) is not present
/// or has already been stamped.
pub async fn inbox_mark_applied(
    pool: &PgPool,
    run_id: Uuid,
    broker_message_id: &str,
    applied_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        update oms_inbox
           set applied_at_utc = $3
         where run_id = $1
           and broker_message_id = $2
           and applied_at_utc is null
        "#,
    )
    .bind(run_id)
    .bind(broker_message_id)
    .bind(applied_at)
    .execute(pool)
    .await
    .context("inbox_mark_applied failed")?;
    Ok(())
}

/// Load inbox rows for a run that were received but not yet applied
/// (`applied_at_utc IS NULL`).
///
/// Call this at startup/recovery to identify fills whose apply step did not
/// complete before a crash. Replay these events in canonical durable ingest
/// order (`inbox_id ASC`), independent of `broker_message_id`; each apply must
/// be idempotent so re-applying a partially-applied fill is safe. After
/// successfully applying each row, call `inbox_mark_applied`.
///
/// Uses the partial index `idx_inbox_run_unapplied` for efficiency.
pub async fn inbox_load_unapplied_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<InboxRow>> {
    let rows = sqlx::query(
        r#"
        select inbox_id, run_id, broker_message_id, broker_fill_id,
               broker_sequence_id, broker_timestamp, message_json,
               received_at_utc, applied_at_utc
          from oms_inbox
         where run_id = $1
           and applied_at_utc is null
         order by inbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("inbox_load_unapplied_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(InboxRow {
            inbox_id: row.try_get("inbox_id")?,
            run_id: row.try_get("run_id")?,
            broker_message_id: row.try_get("broker_message_id")?,
            broker_fill_id: row.try_get("broker_fill_id")?,
            broker_sequence_id: row.try_get("broker_sequence_id")?,
            broker_timestamp: row.try_get("broker_timestamp")?,
            message_json: row.try_get("message_json")?,
            received_at_utc: row.try_get("received_at_utc")?,
            applied_at_utc: row.try_get("applied_at_utc")?,
        });
    }
    Ok(out)
}

/// Load outbox rows with status SENT or ACKED (submitted to broker), ordered
/// by outbox_id asc.  Used at cold-start to reconstruct the in-flight OMS
/// order map without querying the broker.
pub async fn outbox_load_submitted_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<OutboxRow>> {
    let rows = sqlx::query(
        r#"
        select outbox_id, run_id, idempotency_key, order_json, status,
               created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
               dispatching_at_utc, dispatch_attempt_id
          from oms_outbox
         where run_id = $1
           and status in ('SENT', 'ACKED')
         order by outbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("outbox_load_submitted_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(OutboxRow {
            outbox_id: row.try_get("outbox_id")?,
            run_id: row.try_get("run_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
            order_json: row.try_get("order_json")?,
            status: row.try_get("status")?,
            created_at_utc: row.try_get("created_at_utc")?,
            sent_at_utc: row.try_get("sent_at_utc")?,
            claimed_at_utc: row.try_get("claimed_at_utc")?,
            claimed_by: row.try_get("claimed_by")?,
            dispatching_at_utc: row.try_get("dispatching_at_utc")?,
            dispatch_attempt_id: row.try_get("dispatch_attempt_id")?,
        });
    }
    Ok(out)
}

/// Load all applied inbox rows (`applied_at_utc IS NOT NULL`), ordered by
/// inbox_id asc.  Used at cold-start to replay fills into the portfolio and
/// advance OMS order state.  Disjoint from the unapplied set processed by
/// Phase 3, so no double-apply risk.
pub async fn inbox_load_all_applied_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<InboxRow>> {
    let rows = sqlx::query(
        r#"
        select inbox_id, run_id, broker_message_id, broker_fill_id,
               broker_sequence_id, broker_timestamp, message_json,
               received_at_utc, applied_at_utc
          from oms_inbox
         where run_id = $1
           and applied_at_utc is not null
         order by inbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("inbox_load_all_applied_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(InboxRow {
            inbox_id: row.try_get("inbox_id")?,
            run_id: row.try_get("run_id")?,
            broker_message_id: row.try_get("broker_message_id")?,
            broker_fill_id: row.try_get("broker_fill_id")?,
            broker_sequence_id: row.try_get("broker_sequence_id")?,
            broker_timestamp: row.try_get("broker_timestamp")?,
            message_json: row.try_get("message_json")?,
            received_at_utc: row.try_get("received_at_utc")?,
            applied_at_utc: row.try_get("applied_at_utc")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Broker order ID map persistence — Patch A4
// ---------------------------------------------------------------------------

/// Persist (or update) an `internal_id → broker_id` mapping after a successful
/// broker submit.
///
/// Uses `ON CONFLICT … DO UPDATE` so idempotent retries (e.g. after a crash
/// between submit and `outbox_mark_sent`) safely overwrite rather than fail.
///
/// Call this immediately after a confirmed broker submit, before returning from
/// the dispatch loop.
pub async fn broker_map_upsert(pool: &PgPool, internal_id: &str, broker_id: &str) -> Result<()> {
    sqlx::query(
        r#"
        insert into broker_order_map (internal_id, broker_id)
        values ($1, $2)
        on conflict (internal_id) do update
            set broker_id = excluded.broker_id
        "#,
    )
    .bind(internal_id)
    .bind(broker_id)
    .execute(pool)
    .await
    .context("broker_map_upsert failed")?;
    Ok(())
}

/// Remove an `internal_id → broker_id` mapping when an order reaches a terminal
/// state (filled, cancel-ack, rejected).
///
/// Silently succeeds if `internal_id` is not present (idempotent cleanup).
pub async fn broker_map_remove(pool: &PgPool, internal_id: &str) -> Result<()> {
    sqlx::query(
        r#"
        delete from broker_order_map
        where internal_id = $1
        "#,
    )
    .bind(internal_id)
    .execute(pool)
    .await
    .context("broker_map_remove failed")?;
    Ok(())
}

/// Load all live `internal_id → broker_id` pairs from DB.
///
/// Called at daemon startup to repopulate the in-memory `BrokerOrderMap`
/// (see `mqk-execution/id_map.rs`) so cancel/replace operations can target the
/// correct broker order ID after a crash or planned restart.
///
/// Returns pairs ordered by `registered_at_utc` ascending (insertion order).
pub async fn broker_map_load(pool: &PgPool) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query(
        r#"
        select internal_id, broker_id
        from broker_order_map
        order by registered_at_utc asc
        "#,
    )
    .fetch_all(pool)
    .await
    .context("broker_map_load failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push((
            row.try_get::<String, _>("internal_id")?,
            row.try_get::<String, _>("broker_id")?,
        ));
    }
    Ok(out)
}
