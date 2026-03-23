//! Cancel-request gateway error classification and local OMS revert helpers.
//!
//! These are pure functions: they do not touch the DB or the broker directly.
//! DB writes are the caller's responsibility.
//!
//! # Exports
//!
//! - `CancelBrokerClass` — disposition enum for a failed cancel attempt.
//! - `CancelGatewayError` — structured error record returned by the gateway.
//! - `classify_cancel_gateway_error` — map a raw gateway error to its class.
//! - `revert_local_cancel_request` — roll back an OMS CancelPending state on
//!   safe-to-revert cancel failures.

use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder};
use mqk_execution::BrokerError;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// CancelBrokerClass + CancelGatewayError
// ---------------------------------------------------------------------------

pub(super) enum CancelBrokerClass {
    HaltAmbiguous,
    HaltAuth,
    Retryable,
    Ambiguous,
    NonRetryable,
    Unknown,
}

pub(super) struct CancelGatewayError {
    pub(super) revert_local: bool,
    pub(super) err_text: String,
    pub(super) class: CancelBrokerClass,
}

pub(super) fn classify_cancel_gateway_error(err: Box<dyn std::error::Error>) -> CancelGatewayError {
    let revert_local = should_revert_local_cancel_request(err.as_ref());
    let err_text = err.to_string();
    let class = match err.downcast_ref::<BrokerError>() {
        Some(be) if be.requires_halt() => {
            if matches!(be, BrokerError::AmbiguousSubmit { .. }) {
                CancelBrokerClass::HaltAmbiguous
            } else {
                CancelBrokerClass::HaltAuth
            }
        }
        Some(be) if be.is_safe_pre_send_retry() => CancelBrokerClass::Retryable,
        Some(be) if be.is_ambiguous_send_outcome() => CancelBrokerClass::Ambiguous,
        Some(_) => CancelBrokerClass::NonRetryable,
        None => CancelBrokerClass::Unknown,
    };
    CancelGatewayError {
        revert_local,
        err_text,
        class,
    }
}

fn should_revert_local_cancel_request(err: &(dyn std::error::Error + 'static)) -> bool {
    if err.downcast_ref::<mqk_execution::GateRefusal>().is_some() {
        return true;
    }
    if err.downcast_ref::<mqk_execution::UnknownOrder>().is_some() {
        return true;
    }
    if let Some(be) = err.downcast_ref::<BrokerError>() {
        return be.is_safe_pre_send_retry()
            || matches!(
                be,
                BrokerError::Reject { .. } | BrokerError::AuthSession { .. }
            );
    }
    false
}

pub(super) fn revert_local_cancel_request(
    oms_orders: &mut BTreeMap<String, OmsOrder>,
    target_order_id: &str,
    request_id: &str,
) {
    let Some(order) = oms_orders.get_mut(target_order_id) else {
        tracing::warn!(
            "cancel_request_local_revert_missing_order request_id={} target_order_id={}",
            request_id,
            target_order_id
        );
        return;
    };

    let revert_event_id = format!("{}:local-revert", request_id);
    if let Err(err) = order.apply(&OmsEvent::CancelReject, Some(&revert_event_id)) {
        tracing::warn!(
            "cancel_request_local_revert_failed request_id={} target_order_id={} err={}",
            request_id,
            target_order_id,
            err
        );
    }
}
