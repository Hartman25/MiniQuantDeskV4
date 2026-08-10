use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
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

/// Load the internal order IDs of every outbox row for a run that may have
/// reached the broker but has not yet resolved to a terminal broker outcome
/// (PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01).
///
/// "May have reached the broker": `status` in `('DISPATCHING', 'SENT',
/// 'ACKED', 'AMBIGUOUS')` -- `DISPATCHING`/`AMBIGUOUS` are included
/// deliberately, fail-closed, for the same reason
/// [`outbox_load_restart_ambiguous_for_run`] treats them as unsafe to ignore:
/// submission may have been attempted before the outcome was durably known.
/// `PENDING`/`CLAIMED` are excluded -- those orders have never been sent to
/// the broker, so there is no possibility of a late broker event for them;
/// they simply never get dispatched once drainage suppresses Phase 1, which
/// is the intended effect, not a gap.
///
/// "Not yet resolved to a terminal outcome": no `oms_inbox` row exists for
/// `(run_id, internal_order_id)` with `event_kind` in `('fill', 'cancel_ack',
/// 'reject')` -- the three broker-driven events that end an order's
/// lifecycle. `partial_fill`/`ack`/`replace_ack`/`replace_reject`/
/// `cancel_reject` are all non-terminal by design (an order can receive any
/// of them and still have further lifecycle ahead of it).
///
/// Returns an empty vec when every broker-reachable order for the run has
/// drained -- the signal `stop_execution_runtime` uses to allow the run to
/// actually stop. Read-only; does not modify any state.
pub async fn outbox_unresolved_broker_reachable_orders(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        select o.idempotency_key
          from oms_outbox o
         where o.run_id = $1
           and o.status in ('DISPATCHING', 'SENT', 'ACKED', 'AMBIGUOUS')
           and not exists (
               select 1
                 from oms_inbox i
                where i.run_id = o.run_id
                  and i.internal_order_id = o.idempotency_key
                  and i.event_kind in ('fill', 'cancel_ack', 'reject')
           )
         order by o.outbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("outbox_unresolved_broker_reachable_orders failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(row.try_get::<String, _>("idempotency_key")?);
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
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedOutboxRow {
    /// The claimed outbox row (status = `CLAIMED`).
    pub row: OutboxRow,
    /// Unforgeable proof of the DB claim. Pass to `BrokerGateway::submit`.
    pub token: OutboxClaimToken,
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
                  and (next_dispatch_after_utc is null or next_dispatch_after_utc <= $4)
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
                  and (next_dispatch_after_utc is null or next_dispatch_after_utc <= $3)
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

/// Outcome of [`outbox_claim_batch_for_run_with_lease_authority`].
#[cfg(any(feature = "runtime-claim", feature = "testkit"))]
#[derive(Debug, Clone, PartialEq)]
pub enum FencedClaimOutcome {
    /// Run/lease authority was proven; the returned rows (possibly empty if
    /// no `PENDING` rows were available) were claimed.
    Claimed(Vec<ClaimedOutboxRow>),
    /// Refused: the run is not durably `RUNNING`.
    RunNotRunning { actual_status: String },
    /// Refused: no unexpired `runtime_leader_lease` row matches the exact
    /// `(holder_id, epoch)` the caller presented.
    LeaseInvalid,
}

/// PAPER-SOAK-STALE-CLAIM-RECOVERY-03 — production-fenced outbox claim.
///
/// # Why this exists
///
/// `outbox_claim_batch_for_run` proves only that a `PENDING` row exists for
/// `run_id` — it has no notion of run status or runtime lease authority at
/// all. Production dispatch (`ExecutionOrchestrator`) called
/// `refresh_or_acquire_runtime_leadership()` immediately before it as a
/// separate step, which is a check-then-act gap: nothing stops the run from
/// being halted-and-cleared in the window between the lease check completing
/// and this claim call executing. This function closes that gap by
/// re-verifying run status AND lease identity atomically, in the same
/// transaction as the claim mutation itself — not by trusting an earlier,
/// separate check.
///
/// # Authority proof
///
/// Inside one transaction, this function:
/// 1. Locks the run row (`SELECT ... FOR UPDATE`) — the same serialization
///    boundary `clear_halted_run_and_reset_stale_claims` and
///    `acquire_or_refresh_lease_for_running_run` use, so a concurrent
///    halt-clear cannot interleave with this claim.
/// 2. Requires `runs.status = 'RUNNING'`.
/// 3. Requires the current `runtime_leader_lease` row to exactly match
///    `(holder_id, epoch)` and be unexpired at `claimed_at`.
/// 4. Only then performs the ordinary `PENDING -> CLAIMED` claim (identical
///    `FOR UPDATE SKIP LOCKED` semantics to `outbox_claim_batch_for_run`).
///
/// Any failure of 2/3 rolls back with zero mutation and returns a typed
/// refusal instead of an empty claim — callers must not conflate "refused
/// because unauthorized" with "no work available".
///
/// # Availability — RT-1 single-dispatcher gate
///
/// Same gate as `outbox_claim_batch_for_run`: `runtime-claim` (production,
/// `mqk-runtime` only) or `testkit` (test infrastructure).
#[cfg(any(feature = "runtime-claim", feature = "testkit"))]
pub async fn outbox_claim_batch_for_run_with_lease_authority(
    pool: &PgPool,
    run_id: Uuid,
    holder_id: &str,
    epoch: i64,
    batch_size: i64,
    dispatcher_id: &str,
    claimed_at: DateTime<Utc>,
) -> Result<FencedClaimOutcome> {
    let mut tx = pool
        .begin()
        .await
        .context("outbox_claim_batch_for_run_with_lease_authority: begin tx failed")?;

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM runs WHERE run_id = $1 FOR UPDATE")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .context("outbox_claim_batch_for_run_with_lease_authority: run lock failed")?;

    let Some(status) = status else {
        tx.rollback().await.ok();
        return Err(anyhow!(
            "outbox_claim_batch_for_run_with_lease_authority: run {run_id} not found"
        ));
    };

    if status != "RUNNING" {
        tx.rollback().await.context(
            "outbox_claim_batch_for_run_with_lease_authority: rollback (not running) failed",
        )?;
        return Ok(FencedClaimOutcome::RunNotRunning {
            actual_status: status,
        });
    }

    let lease_valid: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT holder_id
          FROM runtime_leader_lease
         WHERE id = 1
           AND holder_id = $1
           AND epoch = $2
           AND lease_expires_at > $3
        "#,
    )
    .bind(holder_id)
    .bind(epoch)
    .bind(claimed_at)
    .fetch_optional(&mut *tx)
    .await
    .context("outbox_claim_batch_for_run_with_lease_authority: lease check failed")?;

    if lease_valid.is_none() {
        tx.rollback().await.context(
            "outbox_claim_batch_for_run_with_lease_authority: rollback (lease invalid) failed",
        )?;
        return Ok(FencedClaimOutcome::LeaseInvalid);
    }

    let rows = sqlx::query(
        r#"
        with to_claim as (
            select outbox_id
            from oms_outbox
            where run_id = $2
              and status = 'PENDING'
              and (next_dispatch_after_utc is null or next_dispatch_after_utc <= $4)
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
    .fetch_all(&mut *tx)
    .await
    .context("outbox_claim_batch_for_run_with_lease_authority: claim failed")?;

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

    tx.commit()
        .await
        .context("outbox_claim_batch_for_run_with_lease_authority: commit failed")?;

    Ok(FencedClaimOutcome::Claimed(out))
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
/// # Claim-owner CAS (PAPER-SOAK-STALE-CLAIM-RECOVERY-03)
///
/// The `WHERE` clause now also requires `claimed_by = claim_owner` — not
/// just `status = 'CLAIMED'`. Without this, an ABA sequence is possible: A
/// claims a row, the row is legitimately reset by recovery (e.g. A's run was
/// halted and cleared), B later claims the *same* row, and A — still holding
/// a stale in-memory `ClaimedOutboxRow`/token from before the reset — could
/// transition B's current claim straight to `DISPATCHING` and reach the
/// broker for an order it no longer owns. Binding the transition to the
/// exact `outbox_id` + `idempotency_key` + `claimed_by` recorded at claim
/// time makes that transition provably impossible: A's stale `claim_owner`
/// value can never match B's current `claimed_by`.
///
/// `dispatching_at` is caller-supplied (no SQL `now()` — FC-7 policy).
/// `dispatch_attempt_id` identifies which dispatcher instance was in-flight;
/// used for crash-recovery audit. `claim_owner` should be the exact
/// `claimed_by` value recorded on the row at claim time (`ClaimedOutboxRow::
/// row::claimed_by`), not merely the caller's current identity string.
///
/// Returns `true` if the row transitioned `CLAIMED → DISPATCHING` for
/// exactly this claim owner; `false` if not found, not in `CLAIMED` state,
/// or currently claimed by someone else. Callers MUST treat `false` as a
/// hard pre-broker stop — see `dispatch.rs` submit/cancel paths.
pub async fn outbox_mark_dispatching(
    pool: &PgPool,
    outbox_id: i64,
    idempotency_key: &str,
    claim_owner: &str,
    dispatch_attempt_id: &str,
    dispatching_at: DateTime<Utc>,
) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status              = 'DISPATCHING',
               dispatching_at_utc  = $5,
               dispatch_attempt_id = $4
         where outbox_id       = $1
           and idempotency_key = $2
           and status          = 'CLAIMED'
           and claimed_by      = $3
        returning outbox_id
        "#,
    )
    .bind(outbox_id)
    .bind(idempotency_key)
    .bind(claim_owner)
    .bind(dispatch_attempt_id)
    .bind(dispatching_at)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_dispatching failed")?;

    Ok(row.is_some())
}

/// Reset stale CLAIMED rows back to PENDING — the crash-recovery reaper (FC-6).
///
/// # Status: not currently wired into any production caller
///
/// PAPER-SOAK-STALE-CLAIM-RECOVERY-01 wired this into
/// `build_execution_orchestrator` (mqk-daemon), unconditionally, on every
/// run start. PAPER-SOAK-STALE-CLAIM-RECOVERY-02 removed that call: it ran
/// before any runtime leadership lease existed for the orchestrator being
/// constructed, and the crash-recovery scenario it targeted could never
/// actually reach it anyway (a crashed `RUNNING` run stays durably `RUNNING`
/// in the DB, and the normal start path refuses to start when a durable
/// active run exists without local ownership — so `build_execution_
/// orchestrator` is never called for that run_id at all). Stale-claim
/// recovery now happens atomically inside
/// [`crate::clear_halted_run_and_reset_stale_claims`], gated on the run's
/// durable `HALTED` status as the ownership proof, rather than on this
/// threshold-based standalone primitive. This function itself is unchanged
/// and still correct for its own documented contract; it is retained as a
/// tested primitive with no current caller.
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
    run_id: Uuid,
    stale_threshold: DateTime<Utc>,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        update oms_outbox
           set status         = 'PENDING',
               claimed_at_utc = null,
               claimed_by     = null
         where run_id         = $2
           and status         = 'CLAIMED'
           and claimed_at_utc < $1
        "#,
    )
    .bind(stale_threshold)
    .bind(run_id)
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

// ---------------------------------------------------------------------------
// EXEC-RETRY-01: bounded, backoff-gated retry
// ---------------------------------------------------------------------------

/// Hard ceiling on automatic dispatch retries per outbox row.
///
/// On the `MAX_DISPATCH_ATTEMPTS`-th failure the row transitions to `FAILED`
/// rather than `PENDING`, preventing unbounded retry loops.
pub const MAX_DISPATCH_ATTEMPTS: i32 = 3;

/// Backoff window written to `next_dispatch_after_utc` on each retry.
///
/// `outbox_claim_batch` will not re-claim the row until this many seconds
/// after the retry is recorded.  Fixed (not exponential) to keep behaviour
/// deterministic and auditable.
const RETRY_BACKOFF_SECS: i64 = 30;

/// Outcome returned by [`outbox_record_retry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryDispatchOutcome {
    /// Row reset to `PENDING`; caller-visible attempt number (1-based).
    WillRetry { attempt: i32 },
    /// Attempt ceiling reached; row transitioned to `FAILED`.
    ExhaustedFailed { attempts: i32 },
}

/// Record a retryable dispatch failure and re-queue with backoff.
///
/// Transitions a `DISPATCHING` row to either `PENDING` (with backoff) or
/// `FAILED` (when `dispatch_attempt_count + 1 >= MAX_DISPATCH_ATTEMPTS`).
///
/// Increments `dispatch_attempt_count`, writes `last_dispatch_error`, and
/// sets `next_dispatch_after_utc = now + RETRY_BACKOFF_SECS` on the
/// `WillRetry` path so `outbox_claim_batch` ignores the row until the window
/// expires.  On the `ExhaustedFailed` path the row is permanently quarantined
/// as `FAILED`.
///
/// Returns `Err` only on DB failure or if the row is not found / not
/// `DISPATCHING`.
pub async fn outbox_record_retry(
    pool: &PgPool,
    idempotency_key: &str,
    error_detail: &str,
    now_utc: DateTime<Utc>,
) -> Result<RetryDispatchOutcome> {
    let retry_after = now_utc + chrono::Duration::seconds(RETRY_BACKOFF_SECS);

    let row: Option<(i32, String)> = sqlx::query_as(
        r#"
        update oms_outbox
           set dispatch_attempt_count  = dispatch_attempt_count + 1,
               last_dispatch_error     = $2,
               status = case
                   when dispatch_attempt_count + 1 >= $3 then 'FAILED'
                   else 'PENDING'
               end,
               next_dispatch_after_utc = case
                   when dispatch_attempt_count + 1 >= $3 then next_dispatch_after_utc
                   else $4
               end,
               claimed_by          = null,
               claimed_at_utc      = null,
               dispatching_at_utc  = null,
               dispatch_attempt_id = null
         where idempotency_key = $1
           and status = 'DISPATCHING'
        returning dispatch_attempt_count, status
        "#,
    )
    .bind(idempotency_key)
    .bind(error_detail)
    .bind(MAX_DISPATCH_ATTEMPTS)
    .bind(retry_after)
    .fetch_optional(pool)
    .await
    .context("outbox_record_retry failed")?;

    match row {
        None => Err(anyhow!(
            "outbox_record_retry: row '{}' not found or not DISPATCHING",
            idempotency_key
        )),
        Some((count, ref status)) if status == "FAILED" => {
            Ok(RetryDispatchOutcome::ExhaustedFailed { attempts: count })
        }
        Some((count, _)) => Ok(RetryDispatchOutcome::WillRetry { attempt: count }),
    }
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

/// Load SENT outbox rows that have a confirmed broker_order_map entry for this run.
///
/// Used by Phase 0c's pending-fill-propagation guard
/// (RECONCILE-DRIFT-AFTER-FAST-PAPER-FILL-01): a SENT row with a broker_order_map
/// entry proves the broker received the order.  When the WS fill event has not
/// yet arrived in oms_inbox, the local OMS still shows the order as open while
/// the broker REST snapshot already reflects the fill.  Phase 0c uses this query
/// to detect whether the reconcile drift is in the ack→fill propagation window
/// before deciding to halt.
///
/// Returns only SENT rows (not ACKED or others): ACKED rows have already been
/// confirmed terminal and their broker_order_map entries have been removed.
pub async fn outbox_load_sent_with_broker_map_for_run(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Vec<OutboxRow>> {
    let rows = sqlx::query(
        r#"
        select o.outbox_id, o.run_id, o.idempotency_key, o.order_json, o.status,
               o.created_at_utc, o.sent_at_utc, o.claimed_at_utc, o.claimed_by,
               o.dispatching_at_utc, o.dispatch_attempt_id
        from oms_outbox o
        inner join broker_order_map m on m.internal_id = o.idempotency_key
        where o.run_id = $1
          and o.status = 'SENT'
        order by o.outbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("outbox_load_sent_with_broker_map_for_run failed")?;

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

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-
/// FINALIZATION-CAS: load every `oms_outbox` row for one run, any status,
/// unbounded, ordered by `outbox_id asc`. Read-only. Distinct from every
/// existing run-scoped outbox reader above (each of which filters to a
/// specific status subset for a specific operational purpose): the
/// finalization classifier's no-trade/activity evidence needs "did any
/// outbox row ever exist for this run" and "what is its exact status",
/// across every status the CHECK constraint allows, not a pre-filtered
/// subset.
pub async fn outbox_load_all_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<OutboxRow>> {
    let rows = sqlx::query(
        r#"
        select outbox_id, run_id, idempotency_key, order_json, status,
               created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
               dispatching_at_utc, dispatch_attempt_id
          from oms_outbox
         where run_id = $1
         order by outbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("outbox_load_all_for_run failed")?;

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

// ---------------------------------------------------------------------------
// OPS-08 / EXEC-06: supervisor-grade outbox read for paper execution timeline
// ---------------------------------------------------------------------------

/// Supervisor-visible outbox row for `GET /api/v1/execution/outbox`.
///
/// Read-only.  No feature gate — supervision never modifies lifecycle state.
#[derive(Debug, Clone)]
pub struct OutboxSupervisorRow {
    pub outbox_id: i64,
    pub run_id: Uuid,
    pub idempotency_key: String,
    pub order_json: Value,
    pub status: String,
    pub created_at_utc: DateTime<Utc>,
    pub claimed_at_utc: Option<DateTime<Utc>>,
    pub dispatching_at_utc: Option<DateTime<Utc>>,
    pub sent_at_utc: Option<DateTime<Utc>>,
}

/// Fetch outbox rows for a run for operator supervision.
///
/// Returns at most 200 rows, ordered newest-first (by `outbox_id DESC`).
/// Read-only — no lifecycle state is modified.  No feature gate.
///
/// Called by `GET /api/v1/execution/outbox` to surface the authoritative
/// paper execution timeline for the current run.
pub async fn outbox_fetch_for_supervisor(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Vec<OutboxSupervisorRow>> {
    let rows = sqlx::query(
        r#"
        select outbox_id, run_id, idempotency_key, order_json, status,
               created_at_utc, claimed_at_utc, dispatching_at_utc, sent_at_utc
        from oms_outbox
        where run_id = $1
        order by outbox_id desc
        limit 200
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("outbox_fetch_for_supervisor failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(OutboxSupervisorRow {
            outbox_id: row.try_get("outbox_id")?,
            run_id: row.try_get("run_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
            order_json: row.try_get("order_json")?,
            status: row.try_get("status")?,
            created_at_utc: row.try_get("created_at_utc")?,
            claimed_at_utc: row.try_get("claimed_at_utc")?,
            dispatching_at_utc: row.try_get("dispatching_at_utc")?,
            sent_at_utc: row.try_get("sent_at_utc")?,
        });
    }
    Ok(out)
}
