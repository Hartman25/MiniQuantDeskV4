//! Phase-1 dispatch helpers — submit and cancel paths.
//!
//! These `ExecutionOrchestrator` methods implement the outbox-row dispatch loop
//! called from `tick()` Phase 1.  They live in their own module to keep
//! orchestrator.rs focused on the tick-sequence skeleton.

use anyhow::anyhow;
use mqk_db::{ClaimedOutboxRow, RetryDispatchOutcome, TimeSource};
use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder};
use mqk_execution::{
    BrokerAdapter, BrokerError, BrokerSubmitRequest, GateRefusal, IntegrityGate, ReconcileGate,
    RiskGate, RiskRequestContext, Side, SubmitError,
};

use super::cancel::{
    classify_cancel_gateway_error, revert_local_cancel_request, CancelBrokerClass,
    CancelGatewayError,
};
use super::persist_halt_and_disarm;
use super::ExecutionOrchestrator;

impl<B, IG, RG, RecG, TS> ExecutionOrchestrator<B, IG, RG, RecG, TS>
where
    B: BrokerAdapter,
    IG: IntegrityGate,
    RG: RiskGate,
    RecG: ReconcileGate,
    TS: TimeSource,
{
    /// Submit-path dispatcher for one claimed outbox row.
    pub(super) async fn dispatch_submit_claimed_outbox_row(
        &mut self,
        claimed_row: ClaimedOutboxRow,
        req: BrokerSubmitRequest,
    ) -> anyhow::Result<()> {
        let order_id = claimed_row.row.idempotency_key.clone();
        let outbox_id = claimed_row.row.outbox_id;
        let claim_owner = claimed_row.row.claimed_by.clone();
        let claim = claimed_row.token;
        let symbol = req.symbol.clone();
        let qty = req.quantity;
        let side = req.side;

        // Step 3a: RT-5 - write DISPATCHING before calling gateway.submit().
        //
        // Closes crash window W4: if the process crashes between here and
        // outbox_mark_sent, the row stays DISPATCHING on restart.
        // outbox_reset_stale_claims only resets CLAIMED rows, so the order
        // is NOT silently requeued - preventing double-submit.
        //
        // PAPER-SOAK-STALE-CLAIM-RECOVERY-03 Layer C: the CAS below proves
        // this exact dispatcher still owns the exact durable claim (row
        // identity + claimed_by), and its boolean result is a hard pre-
        // broker fence -- `false` means NO gateway.submit call, no matter
        // how this claimed_row was obtained.
        let claim_owner = claim_owner.ok_or_else(|| {
            anyhow!(
                "STALE_OUTBOX_CLAIM: outbox row {} (order_id={}) has no claim owner \
                 recorded -- refusing to advance to DISPATCHING without proof of ownership",
                outbox_id,
                order_id
            )
        })?;
        let dispatching_ok = mqk_db::outbox_mark_dispatching(
            &self.pool,
            outbox_id,
            &order_id,
            &claim_owner,
            &self.dispatcher_id,
            self.time_source.now_utc(),
        )
        .await?;
        if !dispatching_ok {
            return Err(anyhow!(
                "DISPATCH_FENCE_LOST: outbox row {} (order_id={}, claim_owner={}) failed the \
                 CLAIMED->DISPATCHING ownership CAS -- refusing broker submit (zero calls)",
                outbox_id,
                order_id,
                claim_owner
            ));
        }
        tracing::info!(
            run_id = %self.run_id,
            order_id = %order_id,
            symbol = %symbol,
            qty = %qty,
            "exec_submit_dispatching"
        );

        // RISK-FLATTEN-ON-HALT-01: compute whether this submit is a verified
        // risk-reducing close against the live portfolio position. This is
        // derived solely from current portfolio state + the request's own
        // side/quantity - never from order_json.signal_source or any other
        // caller-supplied flag.
        let current_qty = self
            .portfolio
            .positions
            .get(&symbol)
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        let is_risk_reducing = is_submit_risk_reducing(current_qty, side, qty);
        let risk_ctx = RiskRequestContext { is_risk_reducing };

        // Step 3b: submit via BrokerGateway - the ONLY submit path.
        //
        // A3: gateway.submit returns Result<_, SubmitError>.
        // SubmitError is Send+Sync (all inner fields are String/u64),
        // so no anyhow conversion is needed before the async dispatch.
        let submit_result = self.gateway.submit_with_context(&claim, req, risk_ctx);
        let resp = match submit_result {
            Ok(r) => r,
            Err(e) => {
                // A3: per-class outbox row disposition.
                match &e {
                    SubmitError::Gate(GateRefusal::RiskBlocked(denial)) => {
                        self.capture_risk_denial(&symbol, denial).await?;
                        let _ = mqk_db::outbox_mark_failed(&self.pool, &order_id).await;
                    }
                    SubmitError::Gate(_) => {
                        let _ = mqk_db::outbox_mark_failed(&self.pool, &order_id).await;
                    }
                    SubmitError::Broker(be) if be.requires_halt() => {
                        let now = self.time_source.now_utc();
                        // Outbox state write is attempted first but must not block the
                        // mandatory halt: if the mark fails, Phase-0b quarantine will
                        // enforce on restart via the DISPATCHING row it leaves behind.
                        // persist_halt_and_disarm is mandatory (propagates with ?) so
                        // the caller receives HALT_PERSISTENCE_FAILURE rather than
                        // silently continuing with runs.status still RUNNING.
                        if matches!(be, BrokerError::AmbiguousSubmit { .. }) {
                            if let Err(mark_err) =
                                mqk_db::outbox_mark_ambiguous(&self.pool, &order_id).await
                            {
                                tracing::warn!(
                                    order_id = %order_id,
                                    error = %mark_err,
                                    "dispatch_halt: outbox_mark_ambiguous failed (AmbiguousSubmit); \
                                     Phase-0b quarantine will enforce on restart"
                                );
                            }
                            persist_halt_and_disarm(
                                &self.pool,
                                self.run_id,
                                now,
                                "AmbiguousSubmit",
                            )
                            .await?;
                        } else {
                            if let Err(mark_err) =
                                mqk_db::outbox_mark_failed(&self.pool, &order_id).await
                            {
                                tracing::warn!(
                                    order_id = %order_id,
                                    error = %mark_err,
                                    "dispatch_halt: outbox_mark_failed failed (AuthSession); \
                                     Phase-0b quarantine will enforce on restart"
                                );
                            }
                            persist_halt_and_disarm(&self.pool, self.run_id, now, "AuthSession")
                                .await?;
                        }
                    }
                    SubmitError::Broker(be) if be.is_safe_pre_send_retry() => {
                        let now = self.time_source.now_utc();
                        match mqk_db::outbox_record_retry(
                            &self.pool,
                            &order_id,
                            &format!("{e}"),
                            now,
                        )
                        .await
                        {
                            Ok(RetryDispatchOutcome::WillRetry { attempt }) => {
                                tracing::warn!(
                                    order_id = %order_id,
                                    attempt,
                                    error = %e,
                                    "broker_submit_retryable_will_retry"
                                );
                            }
                            Ok(RetryDispatchOutcome::ExhaustedFailed { attempts }) => {
                                tracing::warn!(
                                    order_id = %order_id,
                                    attempts,
                                    error = %e,
                                    "broker_submit_retryable_exhausted_failed"
                                );
                            }
                            Err(retry_err) => {
                                tracing::warn!(
                                    order_id = %order_id,
                                    error = %retry_err,
                                    "broker_submit_retryable_record_retry_failed"
                                );
                            }
                        }
                    }
                    SubmitError::Broker(be) if be.is_ambiguous_send_outcome() => {
                        let now = self.time_source.now_utc();
                        // Same halt-path pattern: outbox mark is best-effort, halt is mandatory.
                        if let Err(mark_err) =
                            mqk_db::outbox_mark_ambiguous(&self.pool, &order_id).await
                        {
                            tracing::warn!(
                                order_id = %order_id,
                                error = %mark_err,
                                "dispatch_halt: outbox_mark_ambiguous failed (ambiguous-outcome); \
                                 Phase-0b quarantine will enforce on restart"
                            );
                        }
                        persist_halt_and_disarm(&self.pool, self.run_id, now, "AmbiguousSubmit")
                            .await?;
                    }
                    SubmitError::Broker(_) => {
                        let _ = mqk_db::outbox_mark_failed(&self.pool, &order_id).await;
                        tracing::warn!(
                            order_id = %order_id,
                            error = %e,
                            "broker_submit_non_retryable"
                        );
                    }
                }
                return Err(anyhow!("{e}"));
            }
        };

        let sent = mqk_db::outbox_mark_sent_with_broker_map(
            &self.pool,
            &order_id,
            &resp.broker_order_id,
            self.time_source.now_utc(),
        )
        .await?;
        if !sent {
            return Err(anyhow!(
                "broker submit succeeded but outbox row {} could not transition to SENT with broker map persistence",
                order_id
            ));
        }

        self.order_map.register(&order_id, &resp.broker_order_id);
        tracing::info!(
            run_id = %self.run_id,
            order_id = %order_id,
            broker_order_id = %resp.broker_order_id,
            symbol = %symbol,
            qty = %qty,
            "exec_submit_sent"
        );
        self.oms_orders
            .insert(order_id.clone(), OmsOrder::new(&order_id, &symbol, qty));
        // DISCORD-TRADE-LIFECYCLE-ALERTS-01: best-effort alert after durable SENT.
        self.fire_alert(crate::TradeLifecycleEvent::OrderSubmitted {
            run_id: self.run_id,
            order_id: order_id.clone(),
            symbol: symbol.clone(),
            qty,
        });
        Ok(())
    }

    pub(super) async fn dispatch_cancel_claimed_outbox_row(
        &mut self,
        claimed_row: ClaimedOutboxRow,
        target_order_id: String,
    ) -> anyhow::Result<()> {
        let request_id = claimed_row.row.idempotency_key.clone();
        let outbox_id = claimed_row.row.outbox_id;
        let claim_owner = claimed_row.row.claimed_by.clone();

        // PAPER-SOAK-STALE-CLAIM-RECOVERY-03 Layer C: same hard pre-broker
        // fence as the submit path -- see rationale there. `false` means NO
        // gateway.cancel call.
        let claim_owner = claim_owner.ok_or_else(|| {
            anyhow!(
                "STALE_OUTBOX_CLAIM: outbox row {} (request_id={}) has no claim owner \
                 recorded -- refusing to advance to DISPATCHING without proof of ownership",
                outbox_id,
                request_id
            )
        })?;
        let dispatching_ok = mqk_db::outbox_mark_dispatching(
            &self.pool,
            outbox_id,
            &request_id,
            &claim_owner,
            &self.dispatcher_id,
            self.time_source.now_utc(),
        )
        .await?;
        if !dispatching_ok {
            return Err(anyhow!(
                "DISPATCH_FENCE_LOST: outbox row {} (request_id={}, claim_owner={}) failed the \
                 CLAIMED->DISPATCHING ownership CAS -- refusing broker cancel (zero calls)",
                outbox_id,
                request_id,
                claim_owner
            ));
        }

        // EXEC-CANCEL-01: if the cancel target is absent from the live OMS map,
        // handle it immediately and durably.  The row is already DISPATCHING;
        // the broker was never called, but we cannot know whether the broker
        // still holds an open order with that ID (the OMS context was lost).
        //
        // Policy (fail-closed):
        //   1. Mark the row AMBIGUOUS (best-effort) — the cancel was never sent
        //      so the broker-side outcome is unknown; AMBIGUOUS signals operator
        //      intervention is required and prevents Phase-0b silent re-dispatch.
        //   2. persist_halt_and_disarm("CancelTargetMissing") — mandatory.
        //      Continuing to dispatch new orders when cancel-target provenance is
        //      broken is unsafe.  The Phase-0 HALT_GUARD blocks all future ticks.
        //   3. Return Err with "CANCEL_TARGET_MISSING" as the operator-visible label.
        //
        // Do not silently drop the cancel.
        // Do not synthesize a successful cancel.
        // Do not keep trading as if lifecycle state is safe.
        let Some(order) = self.oms_orders.get_mut(&target_order_id) else {
            let now = self.time_source.now_utc();
            if let Err(mark_err) = mqk_db::outbox_mark_ambiguous(&self.pool, &request_id).await {
                tracing::warn!(
                    request_id = %request_id,
                    target_order_id = %target_order_id,
                    error = %mark_err,
                    "dispatch_cancel: outbox_mark_ambiguous failed (CANCEL_TARGET_MISSING); \
                     Phase-0b quarantine will enforce on restart via DISPATCHING row"
                );
            }
            persist_halt_and_disarm(&self.pool, self.run_id, now, "CancelTargetMissing").await?;
            return Err(anyhow!(
                "CANCEL_TARGET_MISSING: cancel request '{}' refused: target order '{}' is not \
                 present in live OMS state; run {} halted and disarmed",
                request_id,
                target_order_id,
                self.run_id
            ));
        };
        order
            .apply(&OmsEvent::CancelRequest, Some(&request_id))
            .map_err(|err| {
                anyhow!(
                    "cancel request {} refused: target order '{}' could not transition to CancelPending: {}",
                    request_id,
                    target_order_id,
                    err
                )
            })?;

        match self.gateway.cancel(&target_order_id, &self.order_map) {
            Ok(_resp) => {
                let acked = mqk_db::outbox_mark_acked(&self.pool, &request_id).await?;
                if !acked {
                    return Err(anyhow!(
                        "broker cancel request succeeded but outbox row {} could not transition to ACKED",
                        request_id
                    ));
                }
                Ok(())
            }
            Err(err) => {
                let CancelGatewayError {
                    revert_local,
                    err_text,
                    class,
                } = classify_cancel_gateway_error(err);

                if revert_local {
                    revert_local_cancel_request(
                        &mut self.oms_orders,
                        &target_order_id,
                        &request_id,
                    );
                }

                match class {
                    CancelBrokerClass::HaltAmbiguous => {
                        let now = self.time_source.now_utc();
                        // Outbox mark is best-effort; persist_halt_and_disarm is mandatory.
                        // See submit path for rationale on this ordering.
                        if let Err(mark_err) =
                            mqk_db::outbox_mark_ambiguous(&self.pool, &request_id).await
                        {
                            tracing::warn!(
                                request_id = %request_id,
                                error = %mark_err,
                                "dispatch_halt: outbox_mark_ambiguous failed (cancel HaltAmbiguous); \
                                 Phase-0b quarantine will enforce on restart"
                            );
                        }
                        persist_halt_and_disarm(&self.pool, self.run_id, now, "AmbiguousSubmit")
                            .await?;
                    }
                    CancelBrokerClass::HaltAuth => {
                        let now = self.time_source.now_utc();
                        if let Err(mark_err) =
                            mqk_db::outbox_mark_failed(&self.pool, &request_id).await
                        {
                            tracing::warn!(
                                request_id = %request_id,
                                error = %mark_err,
                                "dispatch_halt: outbox_mark_failed failed (cancel HaltAuth); \
                                 Phase-0b quarantine will enforce on restart"
                            );
                        }
                        persist_halt_and_disarm(&self.pool, self.run_id, now, "AuthSession")
                            .await?;
                    }
                    CancelBrokerClass::Retryable => {
                        let now = self.time_source.now_utc();
                        match mqk_db::outbox_record_retry(&self.pool, &request_id, &err_text, now)
                            .await
                        {
                            Ok(RetryDispatchOutcome::WillRetry { attempt }) => {
                                tracing::warn!(
                                    request_id = %request_id,
                                    target_order_id = %target_order_id,
                                    attempt,
                                    error = %err_text,
                                    "broker_cancel_retryable_will_retry"
                                );
                            }
                            Ok(RetryDispatchOutcome::ExhaustedFailed { attempts }) => {
                                tracing::warn!(
                                    request_id = %request_id,
                                    target_order_id = %target_order_id,
                                    attempts,
                                    error = %err_text,
                                    "broker_cancel_retryable_exhausted_failed"
                                );
                            }
                            Err(retry_err) => {
                                tracing::warn!(
                                    request_id = %request_id,
                                    target_order_id = %target_order_id,
                                    error = %retry_err,
                                    "broker_cancel_retryable_record_retry_failed"
                                );
                            }
                        }
                    }
                    CancelBrokerClass::Ambiguous => {
                        let now = self.time_source.now_utc();
                        if let Err(mark_err) =
                            mqk_db::outbox_mark_ambiguous(&self.pool, &request_id).await
                        {
                            tracing::warn!(
                                request_id = %request_id,
                                error = %mark_err,
                                "dispatch_halt: outbox_mark_ambiguous failed (cancel Ambiguous); \
                                 Phase-0b quarantine will enforce on restart"
                            );
                        }
                        persist_halt_and_disarm(&self.pool, self.run_id, now, "AmbiguousSubmit")
                            .await?;
                    }
                    CancelBrokerClass::NonRetryable => {
                        let _ = mqk_db::outbox_mark_failed(&self.pool, &request_id).await;
                        tracing::warn!(
                            request_id = %request_id,
                            target_order_id = %target_order_id,
                            error = %err_text,
                            "broker_cancel_non_retryable"
                        );
                    }
                    CancelBrokerClass::Unknown => {
                        let _ = mqk_db::outbox_mark_failed(&self.pool, &request_id).await;
                    }
                }

                Err(anyhow!(err_text))
            }
        }
    }
}

/// RISK-FLATTEN-ON-HALT-01: determine whether a submit request is a verified
/// risk-reducing close against the current portfolio position.
///
/// Derived solely from `current_qty` (signed live position), `side`, and
/// `quantity` (always positive). A request is risk-reducing only when it is
/// an exact-or-smaller close against an existing opposite-direction position:
/// - `Buy` is risk-reducing only when `current_qty < 0` (short) and
///   `quantity <= |current_qty|`.
/// - `Sell` is risk-reducing only when `current_qty > 0` (long) and
///   `quantity <= current_qty`.
pub(super) fn is_submit_risk_reducing(current_qty: i64, side: Side, quantity: i64) -> bool {
    if quantity <= 0 {
        return false;
    }
    match side {
        Side::Buy => current_qty < 0 && quantity <= current_qty.abs(),
        Side::Sell => current_qty > 0 && quantity <= current_qty,
    }
}

#[cfg(test)]
mod is_submit_risk_reducing_tests {
    use super::*;

    #[test]
    fn long_position_sell_within_size_is_risk_reducing() {
        assert!(is_submit_risk_reducing(10, Side::Sell, 10));
        assert!(is_submit_risk_reducing(10, Side::Sell, 5));
    }

    #[test]
    fn long_position_buy_is_not_risk_reducing() {
        assert!(!is_submit_risk_reducing(10, Side::Buy, 5));
    }

    #[test]
    fn short_position_buy_within_size_is_risk_reducing() {
        assert!(is_submit_risk_reducing(-10, Side::Buy, 10));
        assert!(is_submit_risk_reducing(-10, Side::Buy, 5));
    }

    #[test]
    fn short_position_sell_is_not_risk_reducing() {
        assert!(!is_submit_risk_reducing(-10, Side::Sell, 5));
    }

    #[test]
    fn quantity_exceeding_position_is_not_risk_reducing() {
        assert!(!is_submit_risk_reducing(10, Side::Sell, 15));
        assert!(!is_submit_risk_reducing(-10, Side::Buy, 15));
    }

    #[test]
    fn flat_position_is_never_risk_reducing() {
        assert!(!is_submit_risk_reducing(0, Side::Buy, 1));
        assert!(!is_submit_risk_reducing(0, Side::Sell, 1));
    }

    #[test]
    fn non_positive_quantity_is_never_risk_reducing() {
        assert!(!is_submit_risk_reducing(10, Side::Sell, 0));
        assert!(!is_submit_risk_reducing(10, Side::Sell, -5));
    }
}
