//! PER-SYMBOL-TARGET-STATE-01: in-memory target-state proof tests.
//!
//! These tests prove the AppState storage seam directly. The loop wiring is
//! covered by tests/script_guards/test_per_symbol_target_state.ps1 so these
//! tests do not require a DB, broker, or running execution loop.

use std::sync::Arc;

use mqk_daemon::state::{self, AppState, PerSymbolTargetState};

async fn bare_state() -> Arc<AppState> {
    Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ))
}

fn target_state(
    symbol: &str,
    strategy_id: &str,
    current_qty: i64,
    target_qty: i64,
    no_order_reason: &str,
) -> PerSymbolTargetState {
    PerSymbolTargetState {
        symbol: symbol.to_string(),
        strategy_id: strategy_id.to_string(),
        current_qty,
        target_qty,
        delta: target_qty - current_qty,
        no_order_reason: no_order_reason.to_string(),
        last_decision_id: None,
        last_decision_disposition: None,
        updated_at_utc: "2026-01-01T00:00:00Z".to_string(),
    }
}

#[tokio::test]
async fn t01_default_appstate_has_empty_per_symbol_target_states() {
    let st = bare_state().await;

    assert!(
        st.per_symbol_target_states().await.is_empty(),
        "T01: default AppState must start with no per-symbol target rows"
    );
}

#[tokio::test]
async fn t02_recording_one_state_returns_it_through_per_symbol_target_states() {
    let st = bare_state().await;
    let row = target_state("AAPL", "swing_momentum", 0, 10, "order_will_be_submitted");

    st.record_per_symbol_target_state(row.clone()).await;

    assert_eq!(
        st.per_symbol_target_states().await,
        vec![row],
        "T02: recorded target state must round-trip through per_symbol_target_states"
    );
}

#[tokio::test]
async fn t03_same_symbol_with_different_casing_whitespace_overwrites_normalized_row() {
    let st = bare_state().await;

    st.record_per_symbol_target_state(target_state(
        " aapl ",
        "swing_momentum",
        0,
        10,
        "order_will_be_submitted",
    ))
    .await;
    st.record_per_symbol_target_state(target_state(
        "AAPL",
        "swing_momentum",
        10,
        10,
        "already_at_target",
    ))
    .await;

    let rows = st.per_symbol_target_states().await;
    assert_eq!(rows.len(), 1, "T03: normalized symbol key must overwrite");
    assert_eq!(rows[0].symbol, "AAPL");
    assert_eq!(rows[0].current_qty, 10);
    assert_eq!(rows[0].no_order_reason, "already_at_target");
}

#[tokio::test]
async fn t04_returned_states_are_deterministic_sorted_by_symbol() {
    let st = bare_state().await;

    st.record_per_symbol_target_state(target_state(
        "TSLA",
        "swing_momentum",
        0,
        1,
        "order_will_be_submitted",
    ))
    .await;
    st.record_per_symbol_target_state(target_state(
        "AAPL",
        "swing_momentum",
        0,
        1,
        "order_will_be_submitted",
    ))
    .await;
    st.record_per_symbol_target_state(target_state(
        "MSFT",
        "swing_momentum",
        0,
        1,
        "order_will_be_submitted",
    ))
    .await;

    let symbols: Vec<String> = st
        .per_symbol_target_states()
        .await
        .into_iter()
        .map(|row| row.symbol)
        .collect();
    assert_eq!(
        symbols,
        vec!["AAPL".to_string(), "MSFT".to_string(), "TSLA".to_string()],
        "T04: BTreeMap-backed rows must be sorted by normalized symbol"
    );
}

#[tokio::test]
async fn t05_clear_per_symbol_target_states_removes_all_rows() {
    let st = bare_state().await;
    st.record_per_symbol_target_state(target_state(
        "AAPL",
        "swing_momentum",
        0,
        1,
        "order_will_be_submitted",
    ))
    .await;
    st.record_per_symbol_target_state(target_state(
        "MSFT",
        "swing_momentum",
        0,
        1,
        "order_will_be_submitted",
    ))
    .await;

    st.clear_per_symbol_target_states().await;

    assert!(
        st.per_symbol_target_states().await.is_empty(),
        "T05: clear_per_symbol_target_states must remove every row"
    );
}

#[tokio::test]
async fn t06_target_state_stores_already_at_target_with_zero_delta() {
    let st = bare_state().await;
    st.record_per_symbol_target_state(target_state(
        "AAPL",
        "swing_momentum",
        10,
        10,
        "already_at_target",
    ))
    .await;

    let row = st.per_symbol_target_state_for_symbol("aapl").await.unwrap();
    assert_eq!(row.current_qty, 10);
    assert_eq!(row.target_qty, 10);
    assert_eq!(row.delta, 0);
    assert_eq!(row.no_order_reason, "already_at_target");
}

#[tokio::test]
async fn t07_target_state_stores_b5_short_sale_guard_without_decision_outcome() {
    let st = bare_state().await;
    st.record_per_symbol_target_state(target_state(
        "AAPL",
        "swing_momentum",
        0,
        -5,
        "b5_short_sale_guard",
    ))
    .await;

    let row = st.per_symbol_target_state_for_symbol("AAPL").await.unwrap();
    assert_eq!(row.delta, -5);
    assert_eq!(row.no_order_reason, "b5_short_sale_guard");
    assert_eq!(row.last_decision_id, None);
    assert_eq!(row.last_decision_disposition, None);
}

#[tokio::test]
async fn t08_target_state_stores_order_will_be_submitted_before_submission() {
    let st = bare_state().await;
    st.record_per_symbol_target_state(target_state(
        "AAPL",
        "swing_momentum",
        0,
        5,
        "order_will_be_submitted",
    ))
    .await;

    let row = st.per_symbol_target_state_for_symbol("AAPL").await.unwrap();
    assert_eq!(row.delta, 5);
    assert_eq!(row.no_order_reason, "order_will_be_submitted");
    assert_eq!(row.last_decision_id, None);
    assert_eq!(row.last_decision_disposition, None);
}

#[tokio::test]
async fn t09_accepted_outcome_can_update_last_decision_id_and_disposition() {
    let st = bare_state().await;
    let mut row = target_state("AAPL", "swing_momentum", 0, 5, "order_will_be_submitted");
    st.record_per_symbol_target_state(row.clone()).await;

    row.last_decision_id = Some("decision-accepted".to_string());
    row.last_decision_disposition = Some("accepted".to_string());
    row.updated_at_utc = "2026-01-01T00:00:01Z".to_string();
    st.record_per_symbol_target_state(row).await;

    let row = st.per_symbol_target_state_for_symbol("AAPL").await.unwrap();
    assert_eq!(row.last_decision_id.as_deref(), Some("decision-accepted"));
    assert_eq!(row.last_decision_disposition.as_deref(), Some("accepted"));
}

#[tokio::test]
async fn t10_rejected_or_unavailable_outcome_can_update_last_decision_state() {
    let st = bare_state().await;
    let mut row = target_state("AAPL", "swing_momentum", 0, 5, "order_will_be_submitted");
    st.record_per_symbol_target_state(row.clone()).await;

    row.last_decision_id = Some("decision-unavailable".to_string());
    row.last_decision_disposition = Some("unavailable".to_string());
    row.updated_at_utc = "2026-01-01T00:00:02Z".to_string();
    st.record_per_symbol_target_state(row).await;

    let row = st.per_symbol_target_state_for_symbol("AAPL").await.unwrap();
    assert_eq!(
        row.last_decision_id.as_deref(),
        Some("decision-unavailable")
    );
    assert_eq!(
        row.last_decision_disposition.as_deref(),
        Some("unavailable")
    );
}

#[tokio::test]
async fn t11_max_new_orders_per_tick_reached_can_be_recorded_without_order_submission() {
    let st = bare_state().await;
    assert!(
        st.db.is_none(),
        "T11 precondition: bare test state has no DB"
    );

    st.record_per_symbol_target_state(target_state(
        "MSFT",
        "swing_momentum",
        3,
        3,
        "max_new_orders_per_tick_reached",
    ))
    .await;

    let row = st.per_symbol_target_state_for_symbol("MSFT").await.unwrap();
    assert_eq!(row.delta, 0);
    assert_eq!(row.no_order_reason, "max_new_orders_per_tick_reached");
    assert_eq!(row.last_decision_id, None);
}

#[tokio::test]
async fn t12_symbol_mismatch_skipped_no_target_state_can_be_recorded_honestly() {
    let st = bare_state().await;
    st.record_per_symbol_target_state(target_state(
        "TSLA",
        "swing_momentum",
        7,
        7,
        "symbol_mismatch_skipped",
    ))
    .await;

    let row = st.per_symbol_target_state_for_symbol("TSLA").await.unwrap();
    assert_eq!(row.current_qty, row.target_qty);
    assert_eq!(row.delta, 0);
    assert_eq!(row.no_order_reason, "symbol_mismatch_skipped");
}

#[test]
fn t13_no_approved_for_live_field_or_live_routing_authority_exists() {
    let state_rs = include_str!("../src/state.rs");
    let start = state_rs
        .find("pub struct PerSymbolTargetState")
        .expect("T13: PerSymbolTargetState struct must exist");
    let tail = &state_rs[start..];
    let end = tail.find("\n}").expect("T13: struct end must exist");
    let block = &tail[..end];

    assert!(
        !block.contains("approved_for_live"),
        "T13: PerSymbolTargetState must not contain approved_for_live"
    );
    assert!(
        !block.contains("live"),
        "T13: PerSymbolTargetState must carry no live-routing authority field"
    );
}

#[tokio::test]
async fn t14_no_db_persistence_or_migration_is_involved() {
    let st = bare_state().await;
    assert!(st.db.is_none(), "T14 precondition: no DB is configured");

    st.record_per_symbol_target_state(target_state(
        "AAPL",
        "swing_momentum",
        0,
        0,
        "already_at_target",
    ))
    .await;

    assert_eq!(
        st.per_symbol_target_states().await.len(),
        1,
        "T14: in-memory target-state methods must work without DB persistence"
    );
}
