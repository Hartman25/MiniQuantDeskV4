//! BRK-00R-05: Real Alpaca paper WebSocket transport.
//!
//! This module owns the Alpaca paper WS connection lifecycle:
//!
//! 1. Connect to the Alpaca paper WS endpoint (`wss://paper-api.alpaca.markets/stream`
//!    by default, overridable via `ALPACA_PAPER_BASE_URL`).
//! 2. Authenticate using canonical paper credentials (`ALPACA_API_KEY_PAPER` /
//!    `ALPACA_API_SECRET_PAPER` — ENV-TRUTH-01 canonical names).
//! 3. Subscribe to the `trade_updates` stream.
//! 4. On confirmed subscription: update daemon-owned continuity to `Live`.
//! 5. Route inbound trade-update frames through `process_ws_inbound_batch`
//!    when an execution run is active (has a run_id + DB pool).
//! 6. On disconnect: mark `GapDetected`, wait backoff, reconnect.
//!
//! The task starts at daemon boot (see `main.rs`) and runs independently
//! of the execution lifecycle.  Its primary responsibility before any run
//! is to establish the `Live` continuity state required by the BRK-00R-04
//! gate in `start_execution_runtime`.
//!
//! # Credential and URL contract (ENV-TRUTH-01)
//!
//! | Env var                  | Purpose                       |
//! |--------------------------|-------------------------------|
//! | `ALPACA_API_KEY_PAPER`   | Paper API key ID              |
//! | `ALPACA_API_SECRET_PAPER`| Paper API secret key          |
//! | `ALPACA_PAPER_BASE_URL`  | REST base URL (optional)      |
//!
//! The WS URL is derived from the REST base URL by replacing `https://`
//! with `wss://` and appending `/stream`.
//!
//! # Pre-run vs during-run
//!
//! Before a run is active: frames are received to prove connectivity; no
//! durable ingest occurs (no run_id available for `process_ws_inbound_batch`).
//! This is correct because paper+alpaca has no orders in flight before a run.
//!
//! During an active run: frames are routed through `process_ws_inbound_batch`
//! for durable inbox ingest per the BRK-01R / BRK-02R ordering contract.
//!
//! # Resume boundary (BRK-00R-05B)
//!
//! This transport supports **reconnect + fail-closed continuity
//! re-establishment** only.  It does NOT implement persisted WS resume.
//!
//! On each reconnect:
//! - The in-session cursor starts as `ColdStartUnproven` (not seeded from DB).
//! - Continuity advances to `Live` when subscription is confirmed by the server.
//! - Events that arrived during the disconnect window are NOT replayed from
//!   the WS stream.
//!
//! Gap handling (BRK-07R):
//! - On disconnect the reconnect loop marks `GapDetected` (fail-closed).
//! - The orchestrator halts on `GapDetected` cursor state via
//!   `BrokerError::InboundContinuityUnproven`.
//! - Events from the gap window must be recovered via the REST polling path
//!   (`BrokerAdapter::fetch_events`) on the next run restart.
//!
//! **Persisted cursor seeding** (BRK-07R): At the start of each WS session,
//! `load_session_cursor_from_db` loads the last persisted cursor from DB
//! (via `mqk_db::load_broker_cursor`).  This anchors gap-detection to the
//! prior session's last known event position.  It does NOT recover missed
//! events — the WS stream does not replay the gap window.  Events from the
//! gap window must be recovered via the REST polling path
//! (`BrokerAdapter::fetch_events`) on the next run restart.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use mqk_broker_alpaca::types::AlpacaFetchCursor;
use mqk_broker_alpaca::{parse_ws_message, AlpacaWsMessage};
use mqk_runtime::alpaca_inbound::{
    advance_cursor_after_ws_establish, persist_ws_gap_cursor, process_ws_inbound_batch,
    WsIngestOutcome,
};
use tokio::task::JoinHandle;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::types::{
    AlpacaWsContinuityState, AutonomousRecoveryResumeSource, AutonomousSessionTruth, BrokerKind,
    DeploymentMode,
};
use super::AppState;

const DEFAULT_PAPER_BASE_URL: &str = "https://paper-api.alpaca.markets";
const WS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const WS_RECONNECT_BACKOFF: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Public pure functions — testable without network
// ---------------------------------------------------------------------------

/// Build the Alpaca WS authentication JSON message.
///
/// Wire format: `{"action":"auth","key":"<key>","secret":"<secret>"}`
pub fn build_ws_auth_message(key: &str, secret: &str) -> String {
    serde_json::json!({ "action": "auth", "key": key, "secret": secret }).to_string()
}

/// Build the Alpaca WS subscribe message for the `trade_updates` stream.
///
/// Wire format: `{"action":"listen","data":{"streams":["trade_updates"]}}`
pub fn build_ws_subscribe_message() -> String {
    serde_json::json!({
        "action": "listen",
        "data": { "streams": ["trade_updates"] }
    })
    .to_string()
}

/// Derive the Alpaca paper WS URL from the REST base URL.
///
/// Replaces `https://` with `wss://` and appends `/stream`.
///
/// ```text
/// "https://paper-api.alpaca.markets"   → "wss://paper-api.alpaca.markets/stream"
/// "https://paper-api.alpaca.markets/"  → "wss://paper-api.alpaca.markets/stream"
/// ```
pub fn ws_url_from_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let ws_base = if let Some(host) = trimmed.strip_prefix("https://") {
        format!("wss://{host}")
    } else {
        trimmed.to_string()
    };
    format!("{ws_base}/stream")
}

fn recovery_resume_source_from_cursor(
    cursor: &AlpacaFetchCursor,
) -> AutonomousRecoveryResumeSource {
    match &cursor.trade_updates {
        mqk_broker_alpaca::types::AlpacaTradeUpdatesResume::ColdStartUnproven => {
            AutonomousRecoveryResumeSource::ColdStart
        }
        mqk_broker_alpaca::types::AlpacaTradeUpdatesResume::GapDetected { .. }
        | mqk_broker_alpaca::types::AlpacaTradeUpdatesResume::Live { .. } => {
            AutonomousRecoveryResumeSource::PersistedCursor
        }
    }
}

// ---------------------------------------------------------------------------
// Spawn entry point
// ---------------------------------------------------------------------------

/// Spawn the Alpaca paper WS transport background task.
///
/// Returns `None` when:
/// - The daemon is not configured for `paper+alpaca`.
/// - `ALPACA_API_KEY_PAPER` is absent from the environment.
/// - `ALPACA_API_SECRET_PAPER` is absent from the environment.
///
/// The caller MUST retain the returned `JoinHandle`; dropping it aborts the task.
/// In `main.rs` the handle is kept alive for the lifetime of the daemon.
pub fn spawn_alpaca_paper_ws_task(state: Arc<AppState>) -> Option<JoinHandle<()>> {
    if state.deployment_mode() != DeploymentMode::Paper {
        return None;
    }
    if state.runtime_selection().broker_kind != Some(BrokerKind::Alpaca) {
        return None;
    }

    let key = match std::env::var(super::ALPACA_KEY_PAPER_ENV) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                "alpaca_ws: {} not set; WS transport will not start (BRK-00R-05)",
                super::ALPACA_KEY_PAPER_ENV,
            );
            return None;
        }
    };
    let secret = match std::env::var(super::ALPACA_SECRET_PAPER_ENV) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                "alpaca_ws: {} not set; WS transport will not start (BRK-00R-05)",
                super::ALPACA_SECRET_PAPER_ENV,
            );
            return None;
        }
    };
    let base_url = std::env::var(super::ALPACA_BASE_URL_PAPER_ENV)
        .unwrap_or_else(|_| DEFAULT_PAPER_BASE_URL.to_string());
    let ws_url = ws_url_from_base_url(&base_url);

    tracing::info!(
        ws_url,
        "alpaca_ws: spawning paper WS transport (BRK-00R-05)"
    );
    Some(tokio::spawn(alpaca_ws_loop(state, ws_url, key, secret)))
}

// ---------------------------------------------------------------------------
// Reconnect loop
// ---------------------------------------------------------------------------

/// Outer loop: reconnect on disconnect with backoff, marking GapDetected each
/// time before waiting.
async fn alpaca_ws_loop(state: Arc<AppState>, ws_url: String, key: String, secret: String) {
    loop {
        match alpaca_ws_session(&state, &ws_url, &key, &secret).await {
            Ok(()) => {
                tracing::info!("alpaca_ws: session closed cleanly; reconnecting after backoff");
            }
            Err(e) => {
                tracing::warn!(error = %e, "alpaca_ws: session error; reconnecting after backoff");
            }
        }
        // Mark gap before reconnect: any events during the disconnect window are
        // undelivered.  The BRK-07R contract requires GapDetected before resuming.
        state
            .update_ws_continuity(AlpacaWsContinuityState::GapDetected {
                last_message_id: None,
                last_event_at: None,
                detail: "alpaca_ws: transport reconnecting after disconnect".to_string(),
            })
            .await;

        // BRK-08R: Persist GapDetected to DB so the DB cursor is honest about
        // the gap.  Without this, DB state would remain at the last `Live`
        // cursor and the orchestrator could proceed with REST polling as if
        // no gap occurred.  We load the last known cursor (which may already
        // carry last_message_id / rest_activity_after) and demote it.
        if let Some(pool) = state.db.as_ref() {
            let last_cursor = load_session_cursor_from_db(&state).await;
            match persist_ws_gap_cursor(
                pool,
                state.adapter_id(),
                &last_cursor,
                "alpaca_ws: transport disconnect",
                Utc::now(),
            )
            .await
            {
                Ok(_) => tracing::debug!("alpaca_ws: gap cursor persisted to DB (BRK-08R)"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "alpaca_ws: failed to persist gap cursor to DB; \
                     DB state may not reflect disconnect (BRK-08R)"
                ),
            }
        }

        tokio::time::sleep(WS_RECONNECT_BACKOFF).await;
    }
}

// ---------------------------------------------------------------------------
// Single WS session
// ---------------------------------------------------------------------------

/// One WS session: connect → auth → subscribe → receive loop.
///
/// Returns `Ok(())` on a clean remote close.
/// Returns `Err(...)` on transport error, auth failure, or subscribe failure.
async fn alpaca_ws_session(
    state: &Arc<AppState>,
    ws_url: &str,
    key: &str,
    secret: &str,
) -> anyhow::Result<()> {
    tracing::info!(ws_url, "alpaca_ws: connecting");
    let (mut ws_stream, _) = connect_async(ws_url)
        .await
        .map_err(|e| anyhow::anyhow!("alpaca_ws: connect failed: {e}"))?;

    // Alpaca sends a connected-welcome frame on open.  Drain it before sending auth.
    match tokio::time::timeout(WS_HANDSHAKE_TIMEOUT, ws_stream.next()).await {
        Ok(Some(Ok(_))) => {}
        Ok(Some(Err(e))) => return Err(anyhow::anyhow!("alpaca_ws: connection error: {e}")),
        Ok(None) => return Err(anyhow::anyhow!("alpaca_ws: stream closed at welcome")),
        Err(_) => {
            return Err(anyhow::anyhow!(
                "alpaca_ws: timeout waiting for welcome frame"
            ))
        }
    }

    // Send authentication.
    ws_stream
        .send(Message::Text(build_ws_auth_message(key, secret)))
        .await
        .map_err(|e| anyhow::anyhow!("alpaca_ws: auth send failed: {e}"))?;

    // Receive auth response and check for "authorized" status.
    let auth_bytes = recv_text_frame_timeout(&mut ws_stream, WS_HANDSHAKE_TIMEOUT, "auth").await?;
    let auth_msgs = parse_ws_message(&auth_bytes)
        .map_err(|e| anyhow::anyhow!("alpaca_ws: auth response parse failed: {e}"))?;
    let authorized = auth_msgs
        .iter()
        .any(|m| matches!(m, AlpacaWsMessage::Authorization { status } if status == "authorized"));
    if !authorized {
        return Err(anyhow::anyhow!(
            "alpaca_ws: authentication rejected — check {} / {}",
            super::ALPACA_KEY_PAPER_ENV,
            super::ALPACA_SECRET_PAPER_ENV,
        ));
    }
    tracing::info!("alpaca_ws: authenticated");

    // Subscribe to trade_updates.
    ws_stream
        .send(Message::Text(build_ws_subscribe_message()))
        .await
        .map_err(|e| anyhow::anyhow!("alpaca_ws: subscribe send failed: {e}"))?;

    // Receive listening confirmation.
    let listen_bytes =
        recv_text_frame_timeout(&mut ws_stream, WS_HANDSHAKE_TIMEOUT, "subscribe").await?;
    let listen_msgs = parse_ws_message(&listen_bytes)
        .map_err(|e| anyhow::anyhow!("alpaca_ws: subscribe response parse failed: {e}"))?;
    let listening = listen_msgs.iter().any(|m| {
        matches!(
            m,
            AlpacaWsMessage::Listening { streams }
                if streams.iter().any(|s| s == "trade_updates")
        )
    });
    if !listening {
        return Err(anyhow::anyhow!(
            "alpaca_ws: subscription to trade_updates not confirmed by server"
        ));
    }
    tracing::info!("alpaca_ws: subscribed to trade_updates; evaluating continuity repair");

    // BRK-07R: Seed in-session cursor from last persisted position.
    // This anchors gap-detection to the prior session's last known WS event.
    // Does NOT recover missed events (WS does not replay the gap window).
    let prev_cursor = load_session_cursor_from_db(state).await;
    let resume_source = recovery_resume_source_from_cursor(&prev_cursor);

    if matches!(
        resume_source,
        AutonomousRecoveryResumeSource::PersistedCursor
    ) {
        state
            .set_autonomous_session_truth(AutonomousSessionTruth::RecoveryRetrying {
                resume_source: resume_source.clone(),
                detail: "WS transport re-established after restart/disconnect; repairing continuity from the persisted broker cursor before autonomous paper start/resume is allowed".to_string(),
            })
            .await;
    }

    // BRK-08R: If the persisted cursor was GapDetected or ColdStartUnproven,
    // repair it to Live now that subscription is confirmed. `rest_activity_after`
    // is preserved so the next orchestrator tick's REST poll resumes from the
    // last known fill position and recovers FILL/PARTIAL_FILL events from the
    // gap window. Live cursors are returned unchanged.
    let mut current_cursor = if let Some(pool) = state.db.as_ref() {
        match advance_cursor_after_ws_establish(pool, state.adapter_id(), &prev_cursor, Utc::now())
            .await
        {
            Ok(repaired) => {
                let restored_continuity = AlpacaWsContinuityState::from_fetch_cursor(&repaired);
                state.update_ws_continuity(restored_continuity).await;
                state
                    .set_autonomous_session_truth(AutonomousSessionTruth::RecoverySucceeded {
                        resume_source: resume_source.clone(),
                        detail: match resume_source {
                            AutonomousRecoveryResumeSource::PersistedCursor => "WS continuity restored from persisted broker cursor; REST catch-up remains anchored to the preserved cursor position".to_string(),
                            AutonomousRecoveryResumeSource::ColdStart => "WS continuity established from a cold start; autonomous paper start may proceed once the remaining gates pass".to_string(),
                        },
                    })
                    .await;
                tracing::info!(
                    "alpaca_ws: repaired cursor to Live after WS re-establish;                      REST poll will recover gap-window fills (BRK-08R)"
                );
                repaired
            }
            Err(e) => {
                let detail =
                    format!("alpaca_ws: cursor repair failed after subscribe confirmation: {e}");
                state
                    .set_autonomous_session_truth(AutonomousSessionTruth::RecoveryFailed {
                        resume_source: resume_source.clone(),
                        detail: detail.clone(),
                    })
                    .await;
                state
                    .update_ws_continuity(AlpacaWsContinuityState::GapDetected {
                        last_message_id: None,
                        last_event_at: None,
                        detail: detail.clone(),
                    })
                    .await;
                return Err(anyhow::anyhow!(detail));
            }
        }
    } else {
        state
            .update_ws_continuity(AlpacaWsContinuityState::Live {
                last_message_id: String::new(),
                last_event_at: String::new(),
            })
            .await;
        state
            .set_autonomous_session_truth(AutonomousSessionTruth::RecoverySucceeded {
                resume_source: resume_source.clone(),
                detail: "WS continuity established from a cold start; autonomous paper start may proceed once the remaining gates pass".to_string(),
            })
            .await;
        prev_cursor
    };

    // ---------------------------------------------------------------------------
    // Receive loop
    // ---------------------------------------------------------------------------
    while let Some(msg_result) = ws_stream.next().await {
        let msg = msg_result.map_err(|e| anyhow::anyhow!("alpaca_ws: receive error: {e}"))?;

        let raw: Vec<u8> = match msg {
            Message::Text(t) => t.into_bytes(),
            Message::Binary(b) => b,
            Message::Ping(payload) => {
                // Echo pongs to keep the connection alive.
                ws_stream
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|e| anyhow::anyhow!("alpaca_ws: pong failed: {e}"))?;
                continue;
            }
            Message::Close(_) => break,
            _ => continue,
        };

        // Route through the durable ingest path if a run is active.
        let run_id = state.active_owned_run_id().await;
        if let (Some(run_id), Some(pool)) = (run_id, state.db.as_ref()) {
            match process_ws_inbound_batch(
                pool,
                run_id,
                state.adapter_id(),
                &raw,
                &current_cursor,
                Utc::now(),
            )
            .await
            {
                Ok(WsIngestOutcome::EventsIngested { new_cursor, count }) => {
                    tracing::debug!("alpaca_ws: ingested {count} events");
                    // Advance in-memory continuity to Live with the new cursor position.
                    let new_cont = AlpacaWsContinuityState::from_fetch_cursor(&new_cursor);
                    if matches!(&new_cont, AlpacaWsContinuityState::Live { .. }) {
                        state.update_ws_continuity(new_cont).await;
                    }
                    current_cursor = new_cursor;
                }
                Ok(WsIngestOutcome::NoActionableEvents) => {
                    // Protocol-level frame (auth/listen/error) — no-op.
                }
                Err(e) => {
                    // Ingest failure is non-fatal for the WS session.  The cursor
                    // is not advanced; inbox dedup prevents double-apply on retry.
                    tracing::warn!("alpaca_ws: ingest failed (frame skipped): {e}");
                }
            }
        }
        // No active run: no durable ingest (no orders in flight before run start).
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// BRK-07R: Load the last persisted WS cursor from DB to seed the in-session
/// cursor at session start.
///
/// Returns `cold_start_unproven(None)` when:
/// - No DB pool is available (`AppState::db` is `None`).
/// - No cursor row exists for this adapter in the DB.
/// - The stored cursor JSON cannot be parsed (fail-closed: never panics).
async fn load_session_cursor_from_db(state: &AppState) -> AlpacaFetchCursor {
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

/// Read the next text or binary frame from the WS stream, with a timeout.
/// Returns the raw bytes of the frame.
async fn recv_text_frame_timeout(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    timeout: Duration,
    label: &str,
) -> anyhow::Result<Vec<u8>> {
    let msg = tokio::time::timeout(timeout, ws.next())
        .await
        .map_err(|_| anyhow::anyhow!("alpaca_ws: timeout waiting for {label} response"))?
        .ok_or_else(|| anyhow::anyhow!("alpaca_ws: stream closed before {label} response"))?
        .map_err(|e| anyhow::anyhow!("alpaca_ws: error reading {label} response: {e}"))?;
    match msg {
        Message::Text(t) => Ok(t.into_bytes()),
        Message::Binary(b) => Ok(b),
        other => Err(anyhow::anyhow!(
            "alpaca_ws: unexpected message type for {label}: {other:?}"
        )),
    }
}

// ---------------------------------------------------------------------------
// Unit tests — real session path proof (BRK-00R-05B)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! BRK-00R-05B: Real session path proof tests.
    //!
    //! Uses an in-process plain-TCP WebSocket server to exercise the
    //! production `alpaca_ws_session` and `alpaca_ws_loop` code paths.
    //!
    //! No network access required; all tests run fully in-process.

    use super::{alpaca_ws_loop, alpaca_ws_session};
    use crate::state::{
        types::AlpacaWsContinuityState, AppState, AutonomousRecoveryResumeSource,
        AutonomousSessionTruth, BrokerKind, DeploymentMode,
    };
    use futures_util::{SinkExt, StreamExt};
    use mqk_broker_alpaca::types::AlpacaFetchCursor;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message;

    // -----------------------------------------------------------------------
    // Mock server infrastructure
    // -----------------------------------------------------------------------

    /// Bind a plain-TCP WS server on a random port, spawn a handler for the
    /// first connection, and return the `ws://` URL.
    async fn start_mock_ws_server<F, Fut>(handler: F) -> String
    where
        F: FnOnce(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            handler(ws).await;
        });
        format!("ws://127.0.0.1:{port}")
    }

    // -----------------------------------------------------------------------
    // Canonical Alpaca WS wire frames
    // -----------------------------------------------------------------------

    fn frame_connected() -> String {
        r#"[{"T":"success","msg":"connected"}]"#.to_string()
    }
    fn frame_authorized() -> String {
        r#"[{"T":"authorization","status":"authorized","action":"authenticate"}]"#.to_string()
    }
    fn frame_listening() -> String {
        r#"[{"T":"listening","streams":["trade_updates"]}]"#.to_string()
    }
    fn frame_trade_update_new() -> String {
        serde_json::json!([{
            "T": "trade_updates",
            "data": {
                "event": "new",
                "timestamp": "2026-01-01T00:00:00Z",
                "order": {
                    "id": "brk-order-001",
                    "client_order_id": "test-order-001",
                    "symbol": "AAPL",
                    "side": "buy",
                    "qty": "10",
                    "filled_qty": "0"
                }
            }
        }])
        .to_string()
    }

    // -----------------------------------------------------------------------
    // Test helper
    // -----------------------------------------------------------------------

    fn paper_alpaca_state() -> Arc<AppState> {
        Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ))
    }

    async fn db_pool_or_skip() -> Option<sqlx::PgPool> {
        let url = match std::env::var("MQK_DATABASE_URL") {
            Ok(v) => v,
            Err(_) => return None,
        };
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("BRK00R05B DB test: failed to connect to MQK_DATABASE_URL"),
        )
    }

    // -----------------------------------------------------------------------
    // BRK07R-U1 — load_session_cursor_from_db: no DB → cold start unproven
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn brk07r_u1_load_session_cursor_no_db_returns_cold_start() {
        use super::load_session_cursor_from_db;
        use mqk_broker_alpaca::types::AlpacaTradeUpdatesResume;

        let state = paper_alpaca_state();
        // state has no DB (new_for_test_with_mode_and_broker always uses db = None).
        let cursor = load_session_cursor_from_db(&state).await;
        assert!(
            matches!(
                cursor.trade_updates,
                AlpacaTradeUpdatesResume::ColdStartUnproven
            ),
            "U1: no-DB path must return ColdStartUnproven; got: {:?}",
            cursor.trade_updates,
        );
    }

    // -----------------------------------------------------------------------
    // BRK00R05B-S1 — Happy path: auth + subscribe → continuity Live
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn brk00r05b_s1_session_happy_path_marks_continuity_live() {
        let url = start_mock_ws_server(|mut ws| async move {
            ws.send(Message::Text(frame_connected())).await.unwrap();
            let _ = ws.next().await; // consume auth message
            ws.send(Message::Text(frame_authorized())).await.unwrap();
            let _ = ws.next().await; // consume subscribe message
            ws.send(Message::Text(frame_listening())).await.unwrap();
            ws.send(Message::Close(None)).await.ok();
        })
        .await;

        let state = paper_alpaca_state();
        let result = alpaca_ws_session(&state, &url, "test-key", "test-secret").await;
        assert!(
            result.is_ok(),
            "S1: session must succeed on happy path; got: {result:?}"
        );
        let cont = state.alpaca_ws_continuity().await;
        assert!(
            cont.is_continuity_proven(),
            "S1: continuity must be Live after auth+subscribe confirmed; got: {cont:?}"
        );
    }

    // -----------------------------------------------------------------------
    // BRK00R05B-S2 — Auth rejected: Err returned, continuity unproven
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn brk00r05b_s2_session_auth_rejected_returns_err_and_continuity_unproven() {
        let url = start_mock_ws_server(|mut ws| async move {
            ws.send(Message::Text(frame_connected())).await.unwrap();
            let _ = ws.next().await; // consume auth message
            ws.send(Message::Text(
                r#"[{"T":"authorization","status":"rejected","action":"authenticate"}]"#
                    .to_string(),
            ))
            .await
            .unwrap();
        })
        .await;

        let state = paper_alpaca_state();
        let result = alpaca_ws_session(&state, &url, "bad-key", "bad-secret").await;
        assert!(
            result.is_err(),
            "S2: session must return Err on auth rejection"
        );
        let cont = state.alpaca_ws_continuity().await;
        assert!(
            !cont.is_continuity_proven(),
            "S2: continuity must remain unproven after auth rejection; got: {cont:?}"
        );
    }

    // -----------------------------------------------------------------------
    // BRK00R05B-S3 — Trade-update frame received and dispatched (no active run)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn brk00r05b_s3_trade_update_received_dispatched_through_production_path() {
        let url = start_mock_ws_server(|mut ws| async move {
            ws.send(Message::Text(frame_connected())).await.unwrap();
            let _ = ws.next().await;
            ws.send(Message::Text(frame_authorized())).await.unwrap();
            let _ = ws.next().await;
            ws.send(Message::Text(frame_listening())).await.unwrap();
            // Send a trade-update frame; session has no active run so ingest
            // is correctly skipped (no run_id / no pool).
            ws.send(Message::Text(frame_trade_update_new()))
                .await
                .unwrap();
            ws.send(Message::Close(None)).await.ok();
        })
        .await;

        let state = paper_alpaca_state();
        let result = alpaca_ws_session(&state, &url, "test-key", "test-secret").await;
        assert!(
            result.is_ok(),
            "S3: session must survive trade-update with no active run; got: {result:?}"
        );
        let cont = state.alpaca_ws_continuity().await;
        assert!(
            cont.is_continuity_proven(),
            "S3: continuity must remain Live after trade-update frame; got: {cont:?}"
        );
    }

    // -----------------------------------------------------------------------
    // BRK00R05B-S4 — Reconnect loop marks GapDetected after session disconnect
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn brk00r05b_s4_loop_marks_gap_detected_after_session_disconnect() {
        // Server completes a full happy-path session then closes cleanly.
        // The loop marks GapDetected before waiting for the 5-second backoff.
        let url = start_mock_ws_server(|mut ws| async move {
            ws.send(Message::Text(frame_connected())).await.unwrap();
            let _ = ws.next().await;
            ws.send(Message::Text(frame_authorized())).await.unwrap();
            let _ = ws.next().await;
            ws.send(Message::Text(frame_listening())).await.unwrap();
            ws.send(Message::Close(None)).await.ok();
        })
        .await;

        let state = paper_alpaca_state();
        let state_clone = Arc::clone(&state);
        let task = tokio::spawn(alpaca_ws_loop(
            state_clone,
            url,
            "test-key".to_string(),
            "test-secret".to_string(),
        ));

        // Wait long enough for the loop to mark GapDetected but well under
        // the 5-second reconnect backoff window.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let cont = state.alpaca_ws_continuity().await;
        assert!(
            matches!(cont, AlpacaWsContinuityState::GapDetected { .. }),
            "S4: continuity must be GapDetected after session disconnect; got: {cont:?}"
        );

        task.abort();
    }

    // -----------------------------------------------------------------------
    // BRK00R05B-S5 — DB-backed restart repair restores continuity from persisted cursor
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn brk00r05b_s5_db_backed_restart_repair_sets_recovery_truth() {
        let Some(pool) = db_pool_or_skip().await else {
            eprintln!("S5: skipped (MQK_DATABASE_URL not set)");
            return;
        };
        mqk_db::migrate(&pool).await.expect("S5: migration failed");
        let adapter_id = "brk00r05b-s5-test";

        let gap_cursor = AlpacaFetchCursor::gap_detected(
            Some("rest-s5-anchor".to_string()),
            Some("alpaca:order-s5:filled:2026-01-07T00:00:00Z".to_string()),
            Some("2026-01-07T00:00:00Z".to_string()),
            "s5 persisted gap",
        );
        let cursor_json = serde_json::to_string(&gap_cursor).expect("S5: serialize cursor");
        mqk_db::advance_broker_cursor(&pool, adapter_id, &cursor_json, chrono::Utc::now())
            .await
            .expect("S5: persist gap cursor");

        let url = start_mock_ws_server(|mut ws| async move {
            ws.send(Message::Text(frame_connected())).await.unwrap();
            let _ = ws.next().await;
            ws.send(Message::Text(frame_authorized())).await.unwrap();
            let _ = ws.next().await;
            ws.send(Message::Text(frame_listening())).await.unwrap();
            ws.send(Message::Close(None)).await.ok();
        })
        .await;

        let mut state_inner = AppState::new_for_test_with_db_mode_and_broker(
            pool,
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        );
        state_inner.set_adapter_id_for_test(adapter_id);
        let state = Arc::new(state_inner);

        let result = alpaca_ws_session(&state, &url, "test-key", "test-secret").await;
        assert!(result.is_ok(), "S5: session must succeed; got: {result:?}");

        let cont = state.alpaca_ws_continuity().await;
        assert!(
            matches!(cont, AlpacaWsContinuityState::Live { .. }),
            "S5: continuity must be Live after restart repair; got: {cont:?}"
        );
        let truth = state.autonomous_session_truth().await;
        assert!(
            matches!(
                truth,
                AutonomousSessionTruth::RecoverySucceeded {
                    resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
                    ..
                }
            ),
            "S5: recovery truth must record persisted-cursor success; got: {truth:?}"
        );

        let stored_json = mqk_db::load_broker_cursor(state.db.as_ref().unwrap(), adapter_id)
            .await
            .expect("S5: load cursor")
            .expect("S5: stored cursor must exist");
        let stored: AlpacaFetchCursor =
            serde_json::from_str(&stored_json).expect("S5: parse stored cursor");
        assert!(
            matches!(
                stored.trade_updates,
                mqk_broker_alpaca::types::AlpacaTradeUpdatesResume::Live { .. }
            ),
            "S5: stored cursor must be Live after repair; got: {:?}",
            stored.trade_updates
        );
        assert_eq!(
            stored.rest_activity_after.as_deref(),
            Some("rest-s5-anchor"),
            "S5: rest_activity_after must remain anchored for REST catch-up"
        );
    }
}
