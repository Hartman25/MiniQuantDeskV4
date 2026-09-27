//! Exact-quantity V2 surfaces for the broker-snapshot portfolio routes
//! (`positions`, `orders/open`, `fills`).
//!
//! Invariant: a real fractional broker quantity is never reported as `0`
//! (flat / no order / no fill). V1 (whole-unit) refuses it with a 409 that
//! names the V2 route; V2 carries the exact raw `QtyMicros`; a quantity that
//! is not an exact decimal is refused by both, never substituted.
//!
//! | Test  | Claim                                                                    |
//! |-------|--------------------------------------------------------------------------|
//! | BS01  | positions: whole book V1 unchanged (incl. `5.000`), V2 exact + tag       |
//! | BS02  | positions: fractional long/short V1 409, V2 exact, never `flat`          |
//! | BS03  | positions: unparseable qty refused on V1 and V2, never `0`               |
//! | BS04  | open orders: whole V1 unchanged; fractional V1 409, V2 exact             |
//! | BS05  | fills: whole V1 unchanged; fractional V1 409, V2 exact                   |
//! | BS06  | open orders / fills: unparseable qty refused on V1 and V2                |
//! | BS07  | V2 envelopes carry the version tag with no broker snapshot               |
//!
//! All tests are fully in-process (no DB, no network, no broker adapter).

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::DateTime;
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_schemas::{BrokerAccount, BrokerFill, BrokerOrder, BrokerPosition, BrokerSnapshot};
use tower::ServiceExt;

const V2_VERSION: &str = "qty_micros_v1";

async fn call(router: axum::Router, path: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&body).expect("JSON body"))
}

fn ts() -> DateTime<chrono::Utc> {
    DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
}

fn order(symbol: &str, qty: &str) -> BrokerOrder {
    BrokerOrder {
        broker_order_id: format!("bo-{symbol}"),
        client_order_id: format!("co-{symbol}"),
        symbol: symbol.to_string(),
        side: "buy".to_string(),
        r#type: "limit".to_string(),
        status: "new".to_string(),
        qty: qty.to_string(),
        filled_qty: "0".to_string(),
        limit_price: Some("100".to_string()),
        stop_price: None,
        created_at_utc: ts(),
    }
}

fn fill(symbol: &str, qty: &str) -> BrokerFill {
    BrokerFill {
        broker_fill_id: format!("bf-{symbol}"),
        broker_order_id: format!("bo-{symbol}"),
        client_order_id: format!("co-{symbol}"),
        symbol: symbol.to_string(),
        side: "buy".to_string(),
        qty: qty.to_string(),
        price: "100".to_string(),
        fee: "0".to_string(),
        ts_utc: ts(),
    }
}

fn position(symbol: &str, qty: &str) -> BrokerPosition {
    BrokerPosition {
        symbol: symbol.to_string(),
        qty: qty.to_string(),
        avg_price: "100".to_string(),
    }
}

async fn router_with(
    positions: Vec<BrokerPosition>,
    orders: Vec<BrokerOrder>,
    fills: Vec<BrokerFill>,
) -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    st.broker_snapshot.write().await.replace(BrokerSnapshot {
        captured_at_utc: ts(),
        account: BrokerAccount {
            equity: "1000000.00".to_string(),
            cash: "999000.00".to_string(),
            currency: "USD".to_string(),
            buying_power: None,
            daytrading_buying_power: None,
        },
        orders,
        fills,
        positions,
    });
    routes::build_router(st)
}

fn row<'a>(v: &'a serde_json::Value, symbol: &str) -> &'a serde_json::Value {
    v["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .find(|r| r["symbol"] == symbol)
        .unwrap_or_else(|| panic!("no row {symbol} in {v}"))
}

fn assert_v1_refusal(v: &serde_json::Value, v2_route: &str) {
    assert_eq!(v["error"], "quantity_not_representable_in_v1", "{v}");
    assert!(v["detail"].as_str().unwrap().contains(v2_route), "{v}");
    assert!(v.get("rows").is_none(), "no partial V1 body: {v}");
}

fn assert_unparseable_refusal(v: &serde_json::Value, symbol: &str) {
    assert_eq!(v["error"], "broker_quantity_unparseable", "{v}");
    assert!(v["detail"].as_str().unwrap().contains(symbol), "{v}");
    assert!(v.get("rows").is_none(), "no partial body: {v}");
}

#[tokio::test]
async fn bs01_positions_whole_book_v1_unchanged_v2_exact() {
    let router = router_with(
        vec![
            position("AAPL", "10"),
            position("TSLA", "-3"),
            position("MSFT", "5.000"),
            position("IBM", "0"),
        ],
        vec![],
        vec![],
    )
    .await;

    let (status, v1) = call(router.clone(), "/api/v1/portfolio/positions").await;
    assert_eq!(status, StatusCode::OK, "{v1}");
    assert_eq!(row(&v1, "AAPL")["qty"], 10);
    assert_eq!(row(&v1, "TSLA")["qty"], -3);
    assert_eq!(row(&v1, "MSFT")["qty"], 5);
    assert_eq!(row(&v1, "IBM")["pnl_truth_state"], "flat");
    assert_eq!(row(&v1, "AAPL")["broker_qty"], 10);
    assert!(v1.get("quantity_schema_version").is_none(), "{v1}");

    let (status, v2) = call(router, "/api/v2/portfolio/positions").await;
    assert_eq!(status, StatusCode::OK, "{v2}");
    assert_eq!(v2["quantity_schema_version"], V2_VERSION);
    assert_eq!(row(&v2, "AAPL")["qty_micros"], 10_000_000);
    assert_eq!(row(&v2, "TSLA")["qty_micros"], -3_000_000);
    assert_eq!(row(&v2, "MSFT")["broker_qty_micros"], 5_000_000);
    assert_eq!(row(&v2, "IBM")["qty_micros"], 0);
    assert!(row(&v2, "AAPL").get("qty").is_none());
}

#[tokio::test]
async fn bs02_positions_fractional_long_and_short_v1_409_v2_exact_never_flat() {
    let router = router_with(
        vec![position("BTC/USD", "0.5"), position("ETH/USD", "-1.500001")],
        vec![],
        vec![],
    )
    .await;

    let (status, v1) = call(router.clone(), "/api/v1/portfolio/positions").await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "V1 must refuse, not report 0: {v1}"
    );
    assert_v1_refusal(&v1, "/api/v2/portfolio/positions");

    let (status, v2) = call(router, "/api/v2/portfolio/positions").await;
    assert_eq!(status, StatusCode::OK, "{v2}");
    assert_eq!(row(&v2, "BTC/USD")["qty_micros"], 500_000);
    assert_eq!(row(&v2, "BTC/USD")["broker_qty_micros"], 500_000);
    assert_eq!(row(&v2, "ETH/USD")["qty_micros"], -1_500_001);
    assert_ne!(
        row(&v2, "BTC/USD")["pnl_truth_state"],
        "flat",
        "a real fractional position is never flat"
    );
}

#[tokio::test]
async fn bs03_positions_unparseable_qty_is_refused_never_zero() {
    for bad in ["abc", "1e3", "", "0.1234567", "NaN"] {
        let router = router_with(
            vec![position("AAPL", "10"), position("ZZZ", bad)],
            vec![],
            vec![],
        )
        .await;
        for path in ["/api/v1/portfolio/positions", "/api/v2/portfolio/positions"] {
            let (status, v) = call(router.clone(), path).await;
            assert_eq!(status, StatusCode::CONFLICT, "{path} {bad:?}: {v}");
            assert_unparseable_refusal(&v, "ZZZ");
        }
    }
}

#[tokio::test]
async fn bs04_open_orders_whole_v1_unchanged_fractional_v1_409_v2_exact() {
    let whole = router_with(vec![], vec![order("AAPL", "10")], vec![]).await;
    let (status, v1) = call(whole.clone(), "/api/v1/portfolio/orders/open").await;
    assert_eq!(status, StatusCode::OK, "{v1}");
    assert_eq!(row(&v1, "AAPL")["requested_qty"], 10);
    assert!(v1.get("quantity_schema_version").is_none());
    let (_, v2) = call(whole, "/api/v2/portfolio/orders/open").await;
    assert_eq!(v2["quantity_schema_version"], V2_VERSION);
    assert_eq!(row(&v2, "AAPL")["requested_qty_micros"], 10_000_000);

    let frac = router_with(
        vec![],
        vec![order("AAPL", "10"), order("BTC/USD", "0.25")],
        vec![],
    )
    .await;
    let (status, v1) = call(frac.clone(), "/api/v1/portfolio/orders/open").await;
    assert_eq!(status, StatusCode::CONFLICT, "{v1}");
    assert_v1_refusal(&v1, "/api/v2/portfolio/orders/open");
    let (status, v2) = call(frac, "/api/v2/portfolio/orders/open").await;
    assert_eq!(status, StatusCode::OK, "{v2}");
    assert_eq!(row(&v2, "BTC/USD")["requested_qty_micros"], 250_000);
    assert_eq!(row(&v2, "AAPL")["requested_qty_micros"], 10_000_000);
}

#[tokio::test]
async fn bs05_fills_whole_v1_unchanged_fractional_v1_409_v2_exact() {
    let whole = router_with(vec![], vec![], vec![fill("AAPL", "10")]).await;
    let (status, v1) = call(whole.clone(), "/api/v1/portfolio/fills").await;
    assert_eq!(status, StatusCode::OK, "{v1}");
    assert_eq!(row(&v1, "AAPL")["qty"], 10);
    let (_, v2) = call(whole, "/api/v2/portfolio/fills").await;
    assert_eq!(v2["quantity_schema_version"], V2_VERSION);
    assert_eq!(row(&v2, "AAPL")["qty_micros"], 10_000_000);

    let frac = router_with(
        vec![],
        vec![],
        vec![fill("AAPL", "10"), fill("BTC/USD", "0.000001")],
    )
    .await;
    let (status, v1) = call(frac.clone(), "/api/v1/portfolio/fills").await;
    assert_eq!(status, StatusCode::CONFLICT, "{v1}");
    assert_v1_refusal(&v1, "/api/v2/portfolio/fills");
    let (status, v2) = call(frac, "/api/v2/portfolio/fills").await;
    assert_eq!(status, StatusCode::OK, "{v2}");
    assert_eq!(row(&v2, "BTC/USD")["qty_micros"], 1);
}

#[tokio::test]
async fn bs06_open_orders_and_fills_unparseable_qty_is_refused_never_zero() {
    let router = router_with(vec![], vec![order("ZZZ", "lots")], vec![fill("ZZZ", "n/a")]).await;
    for path in [
        "/api/v1/portfolio/orders/open",
        "/api/v2/portfolio/orders/open",
        "/api/v1/portfolio/fills",
        "/api/v2/portfolio/fills",
    ] {
        let (status, v) = call(router.clone(), path).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {v}");
        assert_unparseable_refusal(&v, "ZZZ");
    }
}

#[tokio::test]
async fn bs07_v2_envelopes_carry_the_version_tag_with_no_broker_snapshot() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);
    for path in [
        "/api/v2/portfolio/positions",
        "/api/v2/portfolio/orders/open",
        "/api/v2/portfolio/fills",
    ] {
        let (status, v) = call(router.clone(), path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {v}");
        assert_eq!(v["snapshot_state"], "no_snapshot", "{path}");
        assert_eq!(v["quantity_schema_version"], V2_VERSION, "{path}");
    }
}
