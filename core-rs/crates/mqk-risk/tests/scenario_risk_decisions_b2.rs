//! B2: Structured Risk Decisions — pure in-memory tests.
//!
//! All tests use only in-memory types (no DB, no async, no IO).
//! They prove correctness of the `RiskDecision`, `RiskReason`, `RiskDenial`,
//! and `RiskEvidence` types introduced in B2.

use mqk_execution::{RiskDecision, RiskDenial, RiskEvidence, RiskReason};

// ---------------------------------------------------------------------------
// B2-1: limit breach produces correct RiskReason and reason_code
// ---------------------------------------------------------------------------

#[test]
fn b2_1_position_limit_denial_has_correct_reason_code() {
    let denial = RiskDenial {
        reason: RiskReason::PositionLimitExceeded,
        evidence: RiskEvidence {
            symbol: Some("AAPL".to_string()),
            requested_qty: Some(200),
            current_position: Some(150),
            limit: Some(300),
        },
    };

    assert_eq!(denial.reason_code(), "POSITION_LIMIT_EXCEEDED");

    // Deny variant wrapping this denial must not be is_allowed.
    let decision = RiskDecision::Deny(denial.clone());
    assert!(!decision.is_allowed());

    // All five reason variants map to distinct non-empty codes.
    assert_eq!(
        RiskDenial {
            reason: RiskReason::MaxOrderSizeExceeded,
            evidence: RiskEvidence::default()
        }
        .reason_code(),
        "MAX_ORDER_SIZE_EXCEEDED"
    );
    assert_eq!(
        RiskDenial {
            reason: RiskReason::SymbolNotAllowed,
            evidence: RiskEvidence::default()
        }
        .reason_code(),
        "SYMBOL_NOT_ALLOWED"
    );
    assert_eq!(
        RiskDenial {
            reason: RiskReason::CapitalLimitExceeded,
            evidence: RiskEvidence::default()
        }
        .reason_code(),
        "CAPITAL_LIMIT_EXCEEDED"
    );
    assert_eq!(
        RiskDenial {
            reason: RiskReason::RiskEngineUnavailable,
            evidence: RiskEvidence::default()
        }
        .reason_code(),
        "RISK_ENGINE_UNAVAILABLE"
    );
}

// ---------------------------------------------------------------------------
// B2-2: Allow decision is allowed
// ---------------------------------------------------------------------------

#[test]
fn b2_2_allow_decision_is_allowed() {
    let decision = RiskDecision::Allow;
    assert!(decision.is_allowed());

    // Deny is never allowed, regardless of reason.
    let deny = RiskDecision::Deny(RiskDenial {
        reason: RiskReason::PositionLimitExceeded,
        evidence: RiskEvidence::default(),
    });
    assert!(!deny.is_allowed());
}

// ---------------------------------------------------------------------------
// B2-3: RiskEngineUnavailable fails closed
// ---------------------------------------------------------------------------

#[test]
fn b2_3_risk_engine_unavailable_fails_closed() {
    // The fail-closed sentinel: when the engine cannot be consulted,
    // the decision MUST be Deny with RiskEngineUnavailable.
    let denial = RiskDenial {
        reason: RiskReason::RiskEngineUnavailable,
        evidence: RiskEvidence::default(),
    };
    let decision = RiskDecision::Deny(denial.clone());

    // Must not be allowed.
    assert!(!decision.is_allowed());
    // Reason code must be the canonical fail-closed code.
    assert_eq!(denial.reason_code(), "RISK_ENGINE_UNAVAILABLE");
    // Summary must be non-empty.
    assert!(!denial.reason_summary().is_empty());
}

// ---------------------------------------------------------------------------
// B2-4: RiskEvidence to_kv_pairs produces correct values
// ---------------------------------------------------------------------------

#[test]
fn b2_4_evidence_kv_pairs_correct() {
    let evidence = RiskEvidence {
        symbol: Some("MSFT".to_string()),
        requested_qty: Some(50),
        current_position: Some(10),
        limit: Some(100),
    };

    let pairs = evidence.to_kv_pairs();

    // All four fields populated → four pairs.
    assert_eq!(pairs.len(), 4);

    let get = |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };

    assert_eq!(get("symbol"), Some("MSFT"));
    assert_eq!(get("requested_qty"), Some("50"));
    assert_eq!(get("current_position"), Some("10"));
    assert_eq!(get("limit"), Some("100"));

    // Empty evidence → zero pairs.
    assert!(RiskEvidence::default().to_kv_pairs().is_empty());

    // Partial evidence → only populated fields appear.
    let partial = RiskEvidence {
        symbol: Some("TSLA".to_string()),
        requested_qty: Some(100),
        current_position: None,
        limit: None,
    };
    let partial_pairs = partial.to_kv_pairs();
    assert_eq!(partial_pairs.len(), 2);
    assert!(partial_pairs
        .iter()
        .any(|(k, v)| k == "symbol" && v == "TSLA"));
    assert!(partial_pairs
        .iter()
        .any(|(k, v)| k == "requested_qty" && v == "100"));
}
