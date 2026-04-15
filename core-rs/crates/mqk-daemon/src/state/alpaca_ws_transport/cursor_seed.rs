//! BRK-07R: WS session cursor seeding and resume classification.
//!
//! These helpers determine how the in-session cursor is seeded at WS session
//! start and what recovery path the session should announce to the autonomous
//! session truth surface.
//!
//! Both functions are pure with respect to session/transport state — they read
//! the DB (or return a cold-start sentinel) and perform a simple enum mapping.
//! Neither touches WS connection state or continuity directly.

use mqk_broker_alpaca::types::{AlpacaFetchCursor, AlpacaTradeUpdatesResume};

use crate::state::types::AutonomousRecoveryResumeSource;
use crate::state::AppState;

/// Derive the autonomous recovery resume source from the last persisted cursor.
///
/// - `ColdStartUnproven` → `ColdStart`  (no prior event position to anchor to).
/// - `GapDetected` or `Live` → `PersistedCursor` (position is known; REST
///   catch-up will be anchored to the preserved `rest_activity_after` field).
pub(crate) fn recovery_resume_source_from_cursor(
    cursor: &AlpacaFetchCursor,
) -> AutonomousRecoveryResumeSource {
    match &cursor.trade_updates {
        AlpacaTradeUpdatesResume::ColdStartUnproven => AutonomousRecoveryResumeSource::ColdStart,
        AlpacaTradeUpdatesResume::GapDetected { .. } | AlpacaTradeUpdatesResume::Live { .. } => {
            AutonomousRecoveryResumeSource::PersistedCursor
        }
    }
}

/// BRK-07R: Load the last persisted WS cursor from DB to seed the in-session
/// cursor at session start.
///
/// Returns `cold_start_unproven(None)` when:
/// - No DB pool is available (`AppState::db` is `None`).
/// - No cursor row exists for this adapter in the DB.
/// - The stored cursor JSON cannot be parsed (fail-closed: never panics).
pub(crate) async fn load_session_cursor_from_db(state: &AppState) -> AlpacaFetchCursor {
    let Some(pool) = state.db.as_ref() else {
        return AlpacaFetchCursor::cold_start_unproven(None);
    };
    match mqk_db::load_broker_cursor(pool, state.adapter_id()).await {
        Ok(Some(json)) => match serde_json::from_str::<AlpacaFetchCursor>(&json) {
            Ok(cursor) => {
                tracing::debug!("alpaca_ws: seeded session cursor from DB (BRK-07R)");
                cursor
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "alpaca_ws: cursor parse failed at session seed; \
                     starting ColdStartUnproven (BRK-07R)"
                );
                AlpacaFetchCursor::cold_start_unproven(None)
            }
        },
        Ok(None) => AlpacaFetchCursor::cold_start_unproven(None),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "alpaca_ws: cursor DB load failed at session seed; \
                 starting ColdStartUnproven (BRK-07R)"
            );
            AlpacaFetchCursor::cold_start_unproven(None)
        }
    }
}
