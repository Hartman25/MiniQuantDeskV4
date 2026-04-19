//! Phase-1 dispatch helpers — submit and cancel paths.
//!
//! These `ExecutionOrchestrator` methods implement the outbox-row dispatch loop
//! called from `tick()` Phase 1.  They live in their own module to keep
//! orchestrator.rs focused on the tick-sequence skeleton.

use anyhow::anyhow;
use mqk_db::{ClaimedOutboxRow, TimeSource};
use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder};
use mqk_execution::{
    BrokerAdapter, BrokerError, BrokerSubmitRequest, GateRefusal, IntegrityGate, ReconcileGate,
    RiskGate, SubmitError,
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
        let claim = claimed_row.token;
        let symbol = req.symbol.clone();
        let qty = req.quantity;

        // Step 3a: RT-5 - write DISPATCHING before calling gateway.submit().
        //
        // Closes crash window W4: if the process crashes between here and
        // outbox_mark_sent, the row stays DISPATCHING on restart.
        // outbox_reset_stale_claims only resets CLAIMED rows, so the order
        // is NOT silently requeued - preventing double-submit.
        mqk_db::outbox_mark_dispatching(
            &self.pool,
            &order_id,
            &self.dispatcher_id,
            self.time_source.now_utc(),
        )
        .await?;
        tracing::info!(
            run_id = %self.run_id,
            order_id = %order_id,
            symbol = %symbol,
            qty = %qty,
            "exec_submit_dispatching"
        );

        // Step 3b: submit via BrokerGateway - the ONLY submit path.
        //
        // A3: gateway.submit returns Result<_, SubmitError>.
        // SubmitError is Send+Sync (all inner fields are String/u64),
        // so no anyhow conversion is needed before the async dispatch.
        let submit_result = self.gateway.submit(&claim, req);
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
                        let _ = mqk_db::outbox_reset_dispatching_to_pending(&self.pool, &order_id)
                            .await;
                        tracing::warn!(
                            order_id = %order_id,
                            error = %e,
                            "broker_submit_retryable"
                        );
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
        Ok(())
    }

    pub(super) async fn dispatch_cancel_claimed_outbox_row(
        &mut self,
        claimed_row: ClaimedOutboxRow,
        target_order_id: String,
    ) -> anyhow::Result<()> {
        let request_id = claimed_row.row.idempotency_key.clone();

        mqk_db::outbox_mark_dispatching(
            &self.pool,
            &request_id,
            &self.dispatcher_id,
            self.time_source.now_utc(),
        )
        .await?;

        let order = self.oms_orders.get_mut(&target_order_id).ok_or_else(|| {
            anyhow!(
                "cancel request {} refused: target order '{}' is not present in live OMS state",
                request_id,
                target_order_id
            )
        })?;
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
                        let _ =
                            mqk_db::outbox_reset_dispatching_to_pending(&self.pool, &request_id)
                                .await;
                        tracing::warn!(
                            request_id = %request_id,
                            target_order_id = %target_order_id,
                            error = %err_text,
                            "broker_cancel_retryable"
                        );
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
