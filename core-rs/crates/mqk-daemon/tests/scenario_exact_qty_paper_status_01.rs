//! `GET /api/v1/autonomous/paper-status`: a real fractional position in the
//! configured strategy symbol must never collapse to `null` (== "no position").
//!
//! V1 keeps `current_position_qty` whole-unit; `null` means only "no snapshot /
//! no position for the symbol". A fractional (long or short) position is
//! refused with a 409 naming the exact live-weights V2 surface, which reads the
//! same execution snapshot. The route stays read-only.
//!
//! One test function drives every phase in order: `MQK_STRATEGY_SYMBOL` is
//! process-global, so phases must not run concurrently.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::state::{AlpacaWsContinuityState, BrokerKind};
use mqk_daemon::{routes, state};
use mqk_execution::QtyMicros;
use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot, PositionSnapshot};
use tower::ServiceExt;

const PAPER_STATUS: &str = "/api/v1/autonomous/paper-status";
const LIVE_WEIGHTS_V2: &str = "/api/v2/portfolio/live-weights";

async fn call(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = router
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot failed");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json body"))
}

async fn paper_alpaca() -> Arc<state::AppState> {
    let st = Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ));
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "ps-msg".to_string(),
        last_event_at: "2026-03-30T15:00:00Z".to_string(),
    })
    .await;
    st
}

fn snapshot(positions: &[(&str, &str)]) -> ExecutionSnapshot {
    ExecutionSnapshot {
        run_id: None,
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: PortfolioSnapshot {
            cash_micros: 1_000_000_000,
            realized_pnl_micros: 0,
            positions: positions
                .iter()
                .map(|(symbol, qty)| PositionSnapshot {
                    symbol: symbol.to_string(),
                    net_qty: qty.parse::<QtyMicros>().expect("qty"),
                })
                .collect(),
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        has_recent_terminal_fill: false,
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        snapshot_at_utc: chrono::Utc::now(),
    }
}

async fn with_positions(positions: &[(&str, &str)]) -> Arc<state::AppState> {
    let st = paper_alpaca().await;
    st.execution_snapshot
        .write()
        .await
        .replace(snapshot(positions));
    st
}

#[tokio::test]
async fn paper_status_current_position_never_collapses_fractional_into_null() {
    std::env::set_var("MQK_STRATEGY_SYMBOL", "BTC/USD");

    // Whole-unit position: V1 unchanged.
    let st = with_positions(&[("BTC/USD", "10")]).await;
    st.set_bar_tick_state_for_test(1, 12, 5);
    let (status, v) = call(routes::build_router(Arc::clone(&st)), PAPER_STATUS).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["current_position_qty"], 10);
    assert_eq!(v["target_qty"], 12);
    assert_eq!(v["computed_delta_qty"], 2);

    // Short whole-unit position keeps its sign.
    let st = with_positions(&[("BTC/USD", "-3")]).await;
    let (status, v) = call(routes::build_router(Arc::clone(&st)), PAPER_STATUS).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["current_position_qty"], -3);

    // Fractional long and short (incl. micro-dust): refused, not null/zero.
    for qty in ["0.5", "-1.500001", "0.000001"] {
        let st = with_positions(&[("BTC/USD", qty)]).await;
        let router = routes::build_router(Arc::clone(&st));
        let (status, v) = call(router.clone(), PAPER_STATUS).await;
        assert_eq!(status, StatusCode::CONFLICT, "{qty}: {v}");
        assert_eq!(v["error"], "quantity_not_representable_in_v1", "{qty}");
        assert!(
            v.get("current_position_qty").is_none(),
            "no null placeholder"
        );
        assert!(
            v["detail"].as_str().unwrap().contains(LIVE_WEIGHTS_V2),
            "refusal names the exact surface: {v}"
        );

        // The named exact surface really carries that quantity.
        let (status, exact) = call(router, LIVE_WEIGHTS_V2).await;
        assert_eq!(status, StatusCode::OK);
        let want: i64 = qty.parse::<QtyMicros>().unwrap().raw();
        assert_eq!(exact["positions"][0]["signed_qty_micros"], want, "{qty}");
    }

    // A fractional position in a symbol that is NOT the configured strategy
    // symbol does not affect this status.
    let st = with_positions(&[("ETH/USD", "0.5")]).await;
    let (status, v) = call(routes::build_router(Arc::clone(&st)), PAPER_STATUS).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(
        v["current_position_qty"].is_null(),
        "no position in the configured symbol stays null: {v}"
    );

    // No snapshot at all: still null (nothing to report), still 200.
    let st = paper_alpaca().await;
    let (status, v) = call(routes::build_router(st), PAPER_STATUS).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(v["current_position_qty"].is_null());

    // Read-only: serving the refusal leaves the execution snapshot untouched.
    let st = with_positions(&[("BTC/USD", "0.5")]).await;
    let before = st.execution_snapshot.read().await.clone().unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(&st)), PAPER_STATUS).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let after = st.execution_snapshot.read().await.clone().unwrap();
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
}
