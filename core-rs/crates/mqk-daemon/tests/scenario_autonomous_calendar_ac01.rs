//! # AUTON-CALENDAR-01 — Autonomous session calendar truth
//!
//! ## Problem closed
//!
//! Before this patch, `AppState.calendar_spec` for Paper+Alpaca was `AlwaysOn`.
//! The `GET /api/v1/system/session` route read from that field and returned
//! `market_session="regular"` and `calendar_spec_id="always_on"` at ALL times —
//! including weekends, holidays, and premarket.  Meanwhile the autonomous session
//! controller was (correctly) using `CalendarSpec::NyseWeekdays` hardcoded, so
//! the controller would block starts outside NYSE regular session while the session
//! display claimed the session was "regular".
//!
//! ## What this file proves
//!
//! | Test   | Claim                                                                                         |
//! |--------|-----------------------------------------------------------------------------------------------|
//! | AC-01  | Paper+Alpaca `calendar_spec()` == NyseWeekdays (fixed by AUTON-CALENDAR-01)                  |
//! | AC-02  | Paper+Paper `calendar_spec()` == AlwaysOn (unchanged — synthetic, not exchange-backed)        |
//! | AC-03  | `system_session` for Paper+Alpaca with Saturday clock → `calendar_spec_id="nyse_weekdays"`,  |
//! |        |   `market_session="closed"`, `exchange_calendar_state="closed"` (no longer lies)             |
//! | AC-04  | `system_session` for Paper+Alpaca with NYSE regular-session clock → `market_session="regular"`|
//! | AC-05  | `system_session` for Paper+Paper with Saturday clock → `market_session="regular"` (AlwaysOn) |
//! | AC-06  | `calendar_spec_id` in `system_session` matches `AppState.calendar_spec().spec_id()` exactly  |
//! | AC-07  | Preflight `session_in_window` and `system_session` `market_session` agree on the same seam:  |
//! |        |   Saturday clock → preflight `session_in_window=false` + session `market_session="closed"`   |

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use state::{AlpacaWsContinuityState, BrokerKind};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Timestamps used across tests
// ---------------------------------------------------------------------------

/// Saturday 2026-03-28 15:00:00 UTC — outside NYSE session.
fn saturday_ts() -> i64 {
    Utc.with_ymd_and_hms(2026, 3, 28, 15, 0, 0)
        .unwrap()
        .timestamp()
}

/// Monday 2026-03-30 14:00:00 UTC = 10:00:00 ET — NYSE regular session.
fn regular_session_ts() -> i64 {
    Utc.with_ymd_and_hms(2026, 3, 30, 14, 0, 0)
        .unwrap()
        .timestamp()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    (status, body)
}

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

fn make_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

fn make_paper_paper() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Paper,
    ))
}

// ---------------------------------------------------------------------------
// AC-01 — Paper+Alpaca calendar_spec() == NyseWeekdays
// ---------------------------------------------------------------------------

#[test]
fn ac01_paper_alpaca_calendar_spec_is_nyse_weekdays() {
    let st = make_paper_alpaca();
    assert_eq!(
        st.calendar_spec(),
        mqk_integrity::CalendarSpec::NyseWeekdays,
        "AUTON-CALENDAR-01: Paper+Alpaca must use NyseWeekdays calendar; \
         the autonomous controller enforces NYSE session boundaries via Alpaca, \
         so the display surface must agree"
    );
}

// ---------------------------------------------------------------------------
// AC-02 — Paper+Paper calendar_spec() == AlwaysOn (unchanged)
// ---------------------------------------------------------------------------

#[test]
fn ac02_paper_paper_calendar_spec_is_always_on() {
    let st = make_paper_paper();
    assert_eq!(
        st.calendar_spec(),
        mqk_integrity::CalendarSpec::AlwaysOn,
        "Paper+Paper uses the in-process fill engine (synthetic time); AlwaysOn is correct"
    );
}

// ---------------------------------------------------------------------------
// AC-03 — system_session for Paper+Alpaca with Saturday clock → "closed"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac03_paper_alpaca_session_shows_closed_on_saturday() {
    let st = make_paper_alpaca();
    // Inject Saturday wall-clock so session_now_ts() returns Saturday.
    st.set_session_clock_ts_for_test(saturday_ts()).await;

    let (status, body) = call(
        routes::build_router(st),
        Request::builder()
            .method("GET")
            .uri("/api/v1/system/session")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["calendar_spec_id"], "nyse_weekdays",
        "Paper+Alpaca must surface nyse_weekdays calendar spec (not always_on)"
    );
    assert_eq!(
        v["market_session"], "closed",
        "Saturday must show market_session=closed for Paper+Alpaca (NyseWeekdays)"
    );
    assert_eq!(
        v["exchange_calendar_state"], "closed",
        "Saturday must show exchange_calendar_state=closed"
    );
    let notes = v["notes"].as_array().expect("notes must be array");
    assert!(
        !notes.is_empty(),
        "notes must carry session truth provenance"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().unwrap_or("").contains("nyse_weekdays")
                || n.as_str().unwrap_or("").contains("heuristic")),
        "notes must describe the NyseWeekdays authority basis: {notes:?}"
    );
}

// ---------------------------------------------------------------------------
// AC-04 — system_session for Paper+Alpaca with regular-session clock → "regular"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac04_paper_alpaca_session_shows_regular_during_nyse_hours() {
    let st = make_paper_alpaca();
    // Inject NYSE regular-session clock.
    st.set_session_clock_ts_for_test(regular_session_ts()).await;

    let (status, body) = call(
        routes::build_router(st),
        Request::builder()
            .method("GET")
            .uri("/api/v1/system/session")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["calendar_spec_id"], "nyse_weekdays",
        "Paper+Alpaca must use nyse_weekdays spec"
    );
    assert_eq!(
        v["market_session"], "regular",
        "NYSE regular-session time must produce market_session=regular"
    );
    assert_eq!(
        v["exchange_calendar_state"], "open",
        "Regular trading day must show exchange_calendar_state=open"
    );
}

// ---------------------------------------------------------------------------
// AC-05 — system_session for Paper+Paper with Saturday clock → "regular"
//         (AlwaysOn is synthetic; it is never closed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05_paper_paper_always_on_shows_regular_even_on_saturday() {
    let st = make_paper_paper();
    // Even with a Saturday timestamp injected, AlwaysOn always returns "regular".
    st.set_session_clock_ts_for_test(saturday_ts()).await;

    let (status, body) = call(
        routes::build_router(st),
        Request::builder()
            .method("GET")
            .uri("/api/v1/system/session")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["calendar_spec_id"], "always_on",
        "Paper+Paper must keep always_on calendar (synthetic in-process fill engine)"
    );
    assert_eq!(
        v["market_session"], "regular",
        "AlwaysOn always reports regular — correct for in-process paper simulation"
    );
}

// ---------------------------------------------------------------------------
// AC-06 — calendar_spec_id in system_session matches AppState.calendar_spec().spec_id()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac06_session_route_calendar_spec_id_matches_state_spec_id() {
    for (st, expected_spec_id) in [
        (
            make_paper_alpaca(),
            mqk_integrity::CalendarSpec::NyseWeekdays.spec_id(),
        ),
        (
            make_paper_paper(),
            mqk_integrity::CalendarSpec::AlwaysOn.spec_id(),
        ),
    ] {
        let spec_from_state = st.calendar_spec().spec_id();
        assert_eq!(
            spec_from_state, expected_spec_id,
            "state.calendar_spec().spec_id() must match expected for this mode/broker"
        );

        // Also confirm the route reflects the same value.
        st.set_session_clock_ts_for_test(saturday_ts()).await;
        let (_, body) = call(
            routes::build_router(Arc::clone(&st)),
            Request::builder()
                .method("GET")
                .uri("/api/v1/system/session")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        let v = parse_json(body);
        assert_eq!(
            v["calendar_spec_id"].as_str().unwrap_or(""),
            spec_from_state,
            "system_session calendar_spec_id must match state.calendar_spec().spec_id()"
        );
    }
}

// ---------------------------------------------------------------------------
// AC-07 — Preflight session_in_window and system_session market_session agree
//         on Saturday: both are blocked/closed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac07_preflight_session_in_window_and_session_route_agree_on_saturday() {
    let st = make_paper_alpaca();
    // Advance WS to Live and arm integrity so the only variable is the session window.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-03-28T15:00:00Z".to_string(),
    })
    .await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    // Inject Saturday clock.
    st.set_session_clock_ts_for_test(saturday_ts()).await;

    // --- preflight ---
    let (_, preflight_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("GET")
            .uri("/api/v1/system/preflight")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    let preflight = parse_json(preflight_body);
    assert_eq!(
        preflight["session_in_window"], false,
        "preflight session_in_window must be false on Saturday for Paper+Alpaca"
    );

    // --- system/session ---
    let (_, session_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("GET")
            .uri("/api/v1/system/session")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    let session = parse_json(session_body);
    assert_eq!(
        session["market_session"], "closed",
        "system_session market_session must be closed on Saturday for Paper+Alpaca"
    );

    // Both surfaces are derived from the same NyseWeekdays seam:
    // preflight session_in_window=false ↔ market_session="closed"
    let market_session = session["market_session"].as_str().unwrap_or("");
    let session_in_window = preflight["session_in_window"].as_bool().unwrap_or(true);
    assert!(
        !session_in_window,
        "session_in_window=false confirms controller will not start"
    );
    assert!(
        market_session != "regular",
        "market_session must not say 'regular' when session_in_window is false: got {market_session}"
    );
}
