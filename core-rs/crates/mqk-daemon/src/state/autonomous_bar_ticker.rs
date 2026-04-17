//! AUTON-PAPER-BLOCKER-02: Autonomous strategy bar ticker for paper+alpaca.
//!
//! Periodically deposits a [`StrategyBarInput`] into
//! `AppState::pending_strategy_bar_input` so the execution loop can drive
//! native strategy `on_bar` dispatch without requiring an external manual POST
//! to `/api/v1/strategy/signal`.
//!
//! # Conditions checked on each tick (fail-closed)
//!
//! 1. **WS continuity must be `Live`** — gap or cold-start means fill tracking
//!    is unreliable; this tick is skipped without depositing.
//! 2. **NYSE session must be `regular`** — paper+alpaca is exchange-backed;
//!    no bar inputs are deposited outside regular-session hours.
//! 3. **Per-run signal limit not exceeded** — when `day_signal_limit_exceeded()`
//!    is true Gate 1d in the decision submission path would refuse anyway.
//!
//! # What the ticker does NOT do
//!
//! - Enqueue to the outbox directly — the B1C bridge in the execution loop does
//!   that after `on_bar` returns `TargetPosition` intents.
//! - Fabricate market data or historical price bars — `limit_price` is always
//!   `None` (honest: no price reference).  Strategies that require a complete
//!   bar (`bar.is_complete == true`) will return empty targets — correct
//!   conservative behaviour, not a silent error.
//! - Bypass any gate in the strategy admission chain.
//!
//! # Configuration
//!
//! - `MQK_STRATEGY_BAR_INTERVAL_SECS` — cadence in seconds (default: 60).
//! - `MQK_STRATEGY_DEFAULT_QTY` — volume context for the bar stub (default: 1).
//!   This is the bar's `volume` field, not the final order quantity.  The
//!   strategy's `TargetPosition.qty` is the absolute target; the actual order
//!   size is the delta computed by `bar_result_to_decisions` in the loop.

use std::sync::Arc;
use std::time::Duration;

use mqk_integrity::CalendarSpec;
use tracing::{debug, info, warn};

use super::types::{AlpacaWsContinuityState, StrategyMarketDataSource};
use super::{AppState, StrategyBarInput};

/// Env var: bar-generation cadence in seconds.
pub const BAR_INTERVAL_SECS_ENV: &str = "MQK_STRATEGY_BAR_INTERVAL_SECS";
/// Env var: volume quantity deposited in each autonomous bar stub.
pub const DEFAULT_QTY_ENV: &str = "MQK_STRATEGY_DEFAULT_QTY";
const DEFAULT_BAR_INTERVAL_SECS: u64 = 60;
const DEFAULT_BAR_QTY: i64 = 1;

pub(super) fn bar_interval_from_env() -> Duration {
    let secs = std::env::var(BAR_INTERVAL_SECS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_BAR_INTERVAL_SECS);
    Duration::from_secs(secs)
}

pub(super) fn default_qty_from_env() -> i64 {
    std::env::var(DEFAULT_QTY_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&q| q > 0)
        .unwrap_or(DEFAULT_BAR_QTY)
}

/// Spawn the autonomous bar ticker for paper+alpaca.
///
/// Returns `None` when the deployment is not configured for
/// `ExternalSignalIngestion` (the only path that benefits from this ticker).
/// No-ops silently for all other deployment/broker combinations.
pub fn spawn_autonomous_bar_ticker(
    state: Arc<AppState>,
) -> Option<tokio::task::JoinHandle<()>> {
    if state.strategy_market_data_source != StrategyMarketDataSource::ExternalSignalIngestion {
        return None;
    }
    let interval = bar_interval_from_env();
    let qty = default_qty_from_env();
    info!(
        interval_secs = interval.as_secs(),
        default_qty = qty,
        "autonomous_bar_ticker: enabled for paper+alpaca (AUTON-PAPER-BLOCKER-02)"
    );
    Some(tokio::spawn(run_bar_ticker(state, interval, qty)))
}

async fn run_bar_ticker(state: Arc<AppState>, interval: Duration, qty: i64) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        run_bar_tick(&state, qty).await;
    }
}

/// Perform one autonomous bar-tick attempt.
///
/// `pub(super)` so sibling tests in the `state` module can call it directly
/// without waiting for the real interval.
pub(super) async fn run_bar_tick(state: &Arc<AppState>, qty: i64) {
    // Gate 1: WS continuity must be Live.
    let ws = state.alpaca_ws_continuity().await;
    if !matches!(ws, AlpacaWsContinuityState::Live { .. }) {
        warn!(
            continuity = ws.as_status_str(),
            "autonomous_bar_ticker: skip_ws_not_live — \
             WS continuity not Live; no bar deposited this tick"
        );
        return;
    }

    // Gate 2: NYSE session must be regular.
    let session_ts = state.session_now_ts().await;
    let session = CalendarSpec::NyseWeekdays.classify_market_session(session_ts);
    if session != "regular" {
        debug!(
            session = session,
            "autonomous_bar_ticker: skip — NYSE session is not regular"
        );
        return;
    }

    // Gate 3: per-run signal limit not exceeded.
    if state.day_signal_limit_exceeded() {
        warn!(
            count = state.day_signal_count(),
            "autonomous_bar_ticker: skip_limit_exceeded — \
             per-run signal limit reached; no bar deposited this tick"
        );
        return;
    }

    let now_tick = state.day_signal_count() as u64;
    let end_ts = session_ts;
    state
        .deposit_strategy_bar_input(StrategyBarInput {
            now_tick,
            end_ts,
            limit_price: None,
            qty,
        })
        .await;
    info!(
        now_tick,
        end_ts,
        qty,
        "autonomous_bar_ticker: bar_deposited"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::types::BrokerKind;

    /// ABT-01: Ticker skips deposit when WS continuity is ColdStartUnproven.
    #[tokio::test]
    async fn abt01_skips_when_ws_not_live() {
        let state = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
        // Default for Alpaca is ColdStartUnproven — no deposit expected.
        run_bar_tick(&state, 1).await;
        assert!(
            state.pending_strategy_bar_input_is_none_for_test().await,
            "ABT-01: bar tick must be skipped when WS is ColdStartUnproven"
        );
    }

    /// ABT-02: Ticker deposits when WS is Live and NYSE session is regular.
    #[tokio::test]
    async fn abt02_deposits_when_live_and_in_session() {
        let state = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
        // Advance WS continuity to Live.
        state
            .update_ws_continuity(AlpacaWsContinuityState::Live {
                last_message_id: "abt02-msg".to_string(),
                last_event_at: "2026-03-30T14:00:00Z".to_string(),
            })
            .await;
        // Monday 2026-03-30 14:00:00 UTC = 10:00:00 ET (regular session, DST).
        state
            .set_session_clock_ts_for_test(
                chrono::DateTime::parse_from_rfc3339("2026-03-30T14:00:00Z")
                    .unwrap()
                    .timestamp(),
            )
            .await;
        run_bar_tick(&state, 5).await;
        assert!(
            !state.pending_strategy_bar_input_is_none_for_test().await,
            "ABT-02: bar tick must deposit when WS is Live and NYSE session is regular"
        );
    }

    /// ABT-03: Ticker skips when per-run signal limit is exceeded.
    #[tokio::test]
    async fn abt03_skips_when_signal_limit_exceeded() {
        let state = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
        state
            .update_ws_continuity(AlpacaWsContinuityState::Live {
                last_message_id: "abt03-msg".to_string(),
                last_event_at: "2026-03-30T14:00:00Z".to_string(),
            })
            .await;
        state
            .set_session_clock_ts_for_test(
                chrono::DateTime::parse_from_rfc3339("2026-03-30T14:00:00Z")
                    .unwrap()
                    .timestamp(),
            )
            .await;
        // Saturate the per-run signal limit (PT-AUTO-02).
        state.set_day_signal_count_for_test(100);
        run_bar_tick(&state, 1).await;
        assert!(
            state.pending_strategy_bar_input_is_none_for_test().await,
            "ABT-03: bar tick must be skipped when per-run signal limit is exceeded"
        );
    }

    /// ABT-04: Ticker skips outside NYSE regular session (weekend).
    #[tokio::test]
    async fn abt04_skips_outside_session() {
        let state = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
        state
            .update_ws_continuity(AlpacaWsContinuityState::Live {
                last_message_id: "abt04-msg".to_string(),
                last_event_at: "2026-03-28T15:00:00Z".to_string(),
            })
            .await;
        // Saturday 2026-03-28 15:00:00 UTC — weekend, out of session.
        state
            .set_session_clock_ts_for_test(
                chrono::DateTime::parse_from_rfc3339("2026-03-28T15:00:00Z")
                    .unwrap()
                    .timestamp(),
            )
            .await;
        run_bar_tick(&state, 1).await;
        assert!(
            state.pending_strategy_bar_input_is_none_for_test().await,
            "ABT-04: bar tick must be skipped outside NYSE regular session"
        );
    }

    /// ABT-05: spawn_autonomous_bar_ticker returns None for non-ExternalSignalIngestion.
    #[tokio::test]
    async fn abt05_not_spawned_for_paper_paper() {
        // Paper+Paper: ExternalSignalIngestion is not configured.
        let state = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Paper));
        let handle = spawn_autonomous_bar_ticker(Arc::clone(&state));
        assert!(
            handle.is_none(),
            "ABT-05: ticker must not spawn for non-ExternalSignalIngestion deployments"
        );
    }
}
