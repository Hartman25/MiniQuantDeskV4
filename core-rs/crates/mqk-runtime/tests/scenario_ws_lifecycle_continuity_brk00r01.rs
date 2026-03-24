//! BRK-00R-01 — Production-owned Alpaca inbound transport/resume seam.
//!
//! # Purpose
//!
//! Proves the runtime-owned `WsLifecycleContinuity` seam is fail-closed.
//! This is the narrowest slice of BRK-00R: it establishes the explicit
//! production-owned contract that later slices will wire into the orchestrator.
//!
//! # Coverage
//!
//! S1  ColdStartUnproven cursor → WsLifecycleContinuity::ColdStartUnproven → is_ready() == false
//! S2  GapDetected cursor → WsLifecycleContinuity::GapDetected → is_ready() == false
//! S3  Live cursor → WsLifecycleContinuity::Live → is_ready() == true
//! S4  Production seam (not test-helper) blocks runtime on cold-start unproven
//! S5  Production seam blocks runtime on gap; gap detail is preserved for diagnostics
//!
//! All tests are pure: no DB, no network, no async.
use mqk_broker_alpaca::types::AlpacaFetchCursor;
use mqk_runtime::alpaca_inbound::{ws_continuity_from_cursor, WsLifecycleContinuity};
// ---------------------------------------------------------------------------
// S1: ColdStartUnproven → not ready
// ---------------------------------------------------------------------------
#[test]
fn s1_cold_start_cursor_derives_to_not_ready() {
    let cursor = AlpacaFetchCursor::cold_start_unproven(None);
    let continuity = ws_continuity_from_cursor(&cursor);
    assert!(
        matches!(continuity, WsLifecycleContinuity::ColdStartUnproven),
        "cold-start cursor must derive to ColdStartUnproven"
    );
    assert!(
        !continuity.is_ready(),
        "ColdStartUnproven must not be ready for event processing"
    );
}
// ---------------------------------------------------------------------------
// S2: GapDetected → not ready
// ---------------------------------------------------------------------------
#[test]
fn s2_gap_detected_cursor_derives_to_not_ready() {
    let cursor = AlpacaFetchCursor::gap_detected(None, None, None, "ws disconnect test");
    let continuity = ws_continuity_from_cursor(&cursor);
    assert!(
        matches!(continuity, WsLifecycleContinuity::GapDetected { .. }),
        "gap cursor must derive to GapDetected"
    );
    assert!(
        !continuity.is_ready(),
        "GapDetected must not be ready for event processing"
    );
}
// ---------------------------------------------------------------------------
// S3: Live → ready
// ---------------------------------------------------------------------------
#[test]
fn s3_live_cursor_derives_to_ready() {
    let cursor = AlpacaFetchCursor::live(
        None,
        "alpaca:order-id:new:2024-06-15T09:30:00Z",
        "2024-06-15T09:30:00.000000Z",
    );
    let continuity = ws_continuity_from_cursor(&cursor);
    assert!(
        matches!(continuity, WsLifecycleContinuity::Live { .. }),
        "live cursor must derive to Live"
    );
    assert!(
        continuity.is_ready(),
        "Live must be ready for event processing"
    );
}
// ---------------------------------------------------------------------------
// S4: production seam blocks cold-start at the runtime check site
// ---------------------------------------------------------------------------
#[test]
fn s4_production_seam_blocks_cold_start_at_runtime_check_site() {
    // Proves the production function (not a cfg(test) helper) is what blocks runtime.
    // Simulates the runtime check pattern that orchestrator will use:
    //   load cursor → derive continuity → check is_ready() → refuse if false.
    let cold = AlpacaFetchCursor::cold_start_unproven(Some("rest-after-xyz".to_string()));
    let continuity = ws_continuity_from_cursor(&cold);
    // Runtime pattern:
    if continuity.is_ready() {
        panic!("production seam must NOT pass cold-start as ready");
    }
    // Reaching here confirms the seam correctly blocks cold-start.
}
// ---------------------------------------------------------------------------
// S5: production seam blocks gap and preserves detail for diagnostics
// ---------------------------------------------------------------------------
#[test]
fn s5_production_seam_blocks_gap_and_preserves_detail() {
    let gap = AlpacaFetchCursor::gap_detected(
        Some("rest-after-xyz".to_string()),
        Some("alpaca:order-id:fill:2024-06-15T09:30:00Z".to_string()),
        Some("2024-06-15T09:30:00.000000Z".to_string()),
        "disconnect without replay: reconnect at 09:35",
    );
    let continuity = ws_continuity_from_cursor(&gap);
    assert!(
        !continuity.is_ready(),
        "production seam must block gap state"
    );
    match &continuity {
        WsLifecycleContinuity::GapDetected {
            last_message_id,
            last_event_at,
            detail,
        } => {
            assert_eq!(
                detail, "disconnect without replay: reconnect at 09:35",
                "gap detail must be preserved for operator diagnostics"
            );
            assert_eq!(
                last_message_id.as_deref(),
                Some("alpaca:order-id:fill:2024-06-15T09:30:00Z"),
                "last_message_id must be preserved from prior cursor"
            );
            assert_eq!(
                last_event_at.as_deref(),
                Some("2024-06-15T09:30:00.000000Z"),
                "last_event_at must be preserved from prior cursor"
            );
        }
        other => panic!("expected GapDetected, got {other:?}"),
    }
}
