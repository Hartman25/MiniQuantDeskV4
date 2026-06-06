//! PAPER-AUTONOMOUS-COMPLETION-BUNDLE-01: Autonomous paper status summary.
//!
//! GET /api/v1/autonomous/paper-status
//!
//! Single read-only surface that aggregates all autonomous paper trading gate
//! state into one response.  Designed for rapid pre-smoke operator inspection
//! and Claude-assisted triage without hitting multiple endpoints.
//!
//! ## Guarantees
//!
//! - Read-only: no DB mutations, no broker calls, no order submission.
//! - Fail-soft: returns degraded fields (not 500) when a subsystem is absent.
//! - `live_routing_enabled` is always `false` for paper mode — surfaced
//!   explicitly as an invariant check, not derived from run state.
//! - `flatten_available` mirrors the exact gate sequence enforced by the
//!   flatten-paper-positions action route.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;

use crate::api_types::AutonomousPaperStatusResponse;
use crate::state::{AppState, DeploymentMode, StrategyMarketDataSource};
use crate::watchlist_intake::evaluate_watchlist_intake_from_env;

use super::system::autonomous_session_truth_to_api;

// ---------------------------------------------------------------------------
// GET /api/v1/autonomous/paper-status
// ---------------------------------------------------------------------------

pub(crate) async fn autonomous_paper_status(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let now = Utc::now();
    let now_utc = now.to_rfc3339();

    let is_paper_alpaca = st.deployment_mode() == DeploymentMode::Paper
        && st.strategy_market_data_source() == StrategyMarketDataSource::ExternalSignalIngestion;

    let mode = st.deployment_mode().as_api_label().to_string();
    // Paper mode never enables live routing.  Explicit invariant surface.
    let live_routing_enabled = false;

    if !is_paper_alpaca {
        return (
            StatusCode::OK,
            Json(AutonomousPaperStatusResponse {
                canonical_route: "/api/v1/autonomous/paper-status".to_string(),
                truth_state: "not_applicable".to_string(),
                mode,
                live_routing_enabled,
                runtime_status: "not_applicable".to_string(),
                arm_state: "not_applicable".to_string(),
                kill_switch_active: false,
                deadman_status: "not_applicable".to_string(),
                ws_continuity: "not_applicable".to_string(),
                reconcile_status: "not_applicable".to_string(),
                mismatch_count: 0,
                open_order_count: 0,
                position_count: 0,
                current_symbol: None,
                current_position_qty: None,
                target_qty: None,
                computed_delta_qty: None,
                no_order_reason: None,
                last_strategy_decision: None,
                flatten_available: false,
                flatten_blockers: vec![
                    "autonomous paper status only applies to the Paper+Alpaca deployment path"
                        .to_string(),
                ],
                evidence_ready: true,
                gui_visibility_ready: true,
                discord_visibility_ready: true,
                watchlist_outcome: "not_applicable".to_string(),
                watchlist_approved: false,
                readiness_classification: "market_proof_pending".to_string(),
                blockers: vec![
                    "deployment is not paper+alpaca; autonomous paper status not applicable"
                        .to_string(),
                ],
                autonomous_session_state: "not_applicable".to_string(),
                next_operator_action: "Set deployment mode to Paper+Alpaca (MQK_DAEMON_ADAPTER_ID=alpaca) to use autonomous paper trading".to_string(),
                now_utc,
            }),
        )
            .into_response();
    }

    // --- Gather live state from AppState ---

    // Status snapshot (fail-soft if unavailable).
    let status = match st.current_status_snapshot().await {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::OK,
                Json(AutonomousPaperStatusResponse {
                    canonical_route: "/api/v1/autonomous/paper-status".to_string(),
                    truth_state: "degraded".to_string(),
                    mode,
                    live_routing_enabled,
                    runtime_status: "unavailable".to_string(),
                    arm_state: "unavailable".to_string(),
                    kill_switch_active: false,
                    deadman_status: "unavailable".to_string(),
                    ws_continuity: st
                        .alpaca_ws_continuity()
                        .await
                        .as_status_str()
                        .to_string(),
                    reconcile_status: "unavailable".to_string(),
                    mismatch_count: 0,
                    open_order_count: 0,
                    position_count: 0,
                    current_symbol: None,
                    current_position_qty: None,
                    target_qty: None,
                    computed_delta_qty: None,
                    no_order_reason: None,
                    last_strategy_decision: None,
                    flatten_available: false,
                    flatten_blockers: vec!["status snapshot unavailable".to_string()],
                    evidence_ready: true,
                    gui_visibility_ready: true,
                    discord_visibility_ready: true,
                    watchlist_outcome: "not_checked".to_string(),
                    watchlist_approved: false,
                    readiness_classification: "blocked".to_string(),
                    blockers: vec!["daemon status snapshot unavailable".to_string()],
                    autonomous_session_state: "not_applicable".to_string(),
                    next_operator_action: "Check daemon health at /v1/health".to_string(),
                    now_utc,
                }),
            )
                .into_response();
        }
    };

    let kill_switch_active = status.state == "halted";
    let deadman_status = status.deadman_status.clone();
    let runtime_status = status.state.clone();

    // WS continuity.
    let ws_continuity = st.alpaca_ws_continuity().await;
    let ws_continuity_str = ws_continuity.as_status_str().to_string();
    let ws_continuity_ready = ws_continuity.is_continuity_proven();

    // Reconcile.
    let reconcile = st.current_reconcile_snapshot().await;
    let reconcile_status_str = reconcile.status.clone();
    let reconcile_ready = !matches!(reconcile_status_str.as_str(), "dirty" | "stale");
    let mismatch_count = reconcile.mismatched_positions
        + reconcile.mismatched_orders
        + reconcile.mismatched_fills
        + reconcile.unmatched_broker_events;

    // Arm state (mirrors autonomous_readiness logic exactly).
    let (arm_state, arm_ready): (String, bool) = {
        let (halted, in_memory_disarmed) = {
            let ig = st.integrity.read().await;
            (ig.halted, ig.disarmed)
        };
        if halted {
            ("halted".to_string(), false)
        } else if in_memory_disarmed {
            let db_state = if let Some(ref pool) = st.db {
                match mqk_db::load_arm_state(pool).await {
                    Ok(Some((ref s, _))) if s == "ARMED" => "arm_pending",
                    Ok(Some(_)) => "disarmed_db",
                    Ok(None) | Err(_) => "arm_pending",
                }
            } else {
                "arm_pending"
            };
            (db_state.to_string(), false)
        } else {
            ("armed".to_string(), true)
        }
    };

    // Autonomous session truth.
    let autonomous_truth = st.autonomous_session_truth().await;
    let (autonomous_state_str, _) = autonomous_session_truth_to_api(&autonomous_truth);

    // Signal ingestion and session window.
    let signal_ingestion_configured =
        st.strategy_market_data_source() == StrategyMarketDataSource::ExternalSignalIngestion;

    // Strategy fleet check (mirrors autonomous_readiness).
    let strategy_fleet_empty = st
        .strategy_fleet_snapshot()
        .await
        .is_none_or(|f| f.is_empty());

    // Runtime start: no locally-owned run.
    let locally_owned_run = st.locally_owned_run_id().await;
    let runtime_start_allowed = locally_owned_run.is_none();

    // --- Execution snapshot for position/order counts ---
    let exec_snap = st.execution_snapshot.read().await.clone();
    let (open_order_count, position_count) = match &exec_snap {
        None => (0usize, 0usize),
        Some(snap) => (snap.active_orders.len(), snap.portfolio.positions.len()),
    };

    // --- Strategy symbol and position diagnostics ---
    let current_symbol = std::env::var("MQK_STRATEGY_SYMBOL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string());

    // current_position_qty: look up symbol in exec snapshot portfolio.
    let current_position_qty: Option<i64> = current_symbol.as_deref().and_then(|sym| {
        exec_snap.as_ref()?.portfolio.positions.iter().find_map(|p| {
            if p.symbol == sym {
                Some(p.net_qty)
            } else {
                None
            }
        })
    });

    // target_qty from last bar signal.
    let target_qty: Option<i64> = st.last_bar_signal_qty();

    // computed_delta = target - current (signed).
    let computed_delta_qty: Option<i64> = match (target_qty, current_position_qty) {
        (Some(t), Some(c)) => Some(t - c),
        _ => None,
    };

    // last_strategy_decision from diagnostics.
    let last_strategy_decision: Option<String> = st
        .last_strategy_diagnostics()
        .await
        .map(|d| d.decision.to_string());

    // --- Flatten availability (mirrors flatten-paper-positions gates) ---
    let mut flatten_blockers: Vec<String> = Vec::new();

    // Gate 1: paper mode (already confirmed above since is_paper_alpaca)
    // Gate 2: DB required
    if st.db.is_none() {
        flatten_blockers.push(
            "flatten requires a DB connection for durable outbox writes (gate=db_unavailable)"
                .to_string(),
        );
    }
    // Gate 3: reconcile must be ok
    if reconcile_status_str != "ok" {
        flatten_blockers.push(format!(
            "flatten blocked: reconcile status is '{}' (gate=reconcile_not_ok)",
            reconcile_status_str
        ));
    }
    // Gate 4: armed
    if !arm_ready {
        flatten_blockers.push(format!(
            "flatten blocked: arm state is '{}' (gate=not_armed)",
            arm_state
        ));
    }
    // Gate 5: active run present
    if locally_owned_run.is_none() {
        flatten_blockers.push(
            "flatten blocked: no active run (gate=no_active_run); start a session first"
                .to_string(),
        );
    }
    // Gate 6: runtime must be running
    if runtime_status != "running" {
        flatten_blockers.push(format!(
            "flatten blocked: runtime state is '{}' not 'running' (gate=not_running)",
            runtime_status
        ));
    }
    // Gate 7: execution snapshot present
    if exec_snap.is_none() {
        flatten_blockers.push(
            "flatten blocked: no execution snapshot loaded (gate=no_execution_snapshot)"
                .to_string(),
        );
    }
    // Gate 8: positions non-empty
    if position_count == 0 {
        flatten_blockers.push(
            "flatten blocked: no open positions to flatten (gate=no_positions)".to_string(),
        );
    }

    let flatten_available = flatten_blockers.is_empty();

    // --- Watchlist ---
    let watchlist = evaluate_watchlist_intake_from_env();
    let watchlist_outcome = watchlist.status_label().to_string();
    let watchlist_approved = watchlist.approved_for_autonomous_paper();

    // --- Start blockers (mirrors autonomous_readiness gate order) ---
    let mut blockers: Vec<String> = Vec::new();

    if !ws_continuity_ready {
        blockers.push(format!(
            "WS continuity not proven (current: '{}'); requires ws_continuity=live before start (BRK-00R-04)",
            ws_continuity_str
        ));
    }
    if !reconcile_ready {
        blockers.push(format!(
            "reconcile status is '{}'; cannot start with dirty or stale reconcile (BRK-09R)",
            reconcile_status_str
        ));
    }
    if !arm_ready {
        match arm_state.as_str() {
            "halted" => blockers.push(
                "integrity is halted; operator must arm manually before autonomous start"
                    .to_string(),
            ),
            "disarmed_db" => blockers.push(
                "DB arm state is DISARMED; re-arm via POST /api/v1/ops/action {\"action\":\"arm-execution\"} (AUTON-NO-TRADE-01)".to_string(),
            ),
            _ => blockers.push(format!(
                "arm state is '{}'; auto-heals on next session-window tick if DB is ARMED",
                arm_state
            )),
        }
    }
    if !signal_ingestion_configured {
        blockers.push(
            "ExternalSignalIngestion not configured; signal ingestion path absent".to_string(),
        );
    }
    if strategy_fleet_empty {
        blockers.push(
            "strategy bootstrap dormant: MQK_STRATEGY_IDS absent or empty (STRATEGY-DORMANCY-01)"
                .to_string(),
        );
    }
    if !runtime_start_allowed {
        blockers.push(
            "a locally-owned execution run is already active; start would return 409 Conflict"
                .to_string(),
        );
    }

    // Session window — informational (not fatal, but surfaces as market_proof_pending).
    let schedule = crate::state::autonomous_session_schedule_from_env();
    let session_in_window = schedule.is_in_session(&st, now).await;
    if !session_in_window {
        blockers.push(
            "current time is outside the autonomous session window; the session controller \
             will not attempt a start until the window opens"
                .to_string(),
        );
    }

    // --- Readiness classification ---
    // Separate fatal blockers (prevent start even during session window) from
    // the session-window-only blocker (expected off-hours).
    let session_window_blocker = "current time is outside the autonomous session window";
    let run_active_blocker = "a locally-owned execution run is already active";
    let fatal_blockers_count = blockers
        .iter()
        .filter(|b| {
            !b.starts_with(session_window_blocker) && !b.starts_with(run_active_blocker)
        })
        .count();

    let readiness_classification = if fatal_blockers_count > 0 || kill_switch_active {
        "blocked"
    } else if blockers.is_empty() {
        // All gates pass including session window.
        "ready_for_market_smoke"
    } else {
        // Only session-window or active-run blockers — code is ready.
        "market_proof_pending"
    };

    // --- next_operator_action ---
    let next_operator_action = if kill_switch_active {
        "Kill switch is active. Clear the halted run: POST /api/v1/ops/action {\"action\":\"clear-halted-run\"}, then re-arm".to_string()
    } else if ws_continuity_str == "gap_detected" {
        "WS gap detected. Review reconcile mismatches at /api/v1/reconcile/mismatches and perform operator gap recovery".to_string()
    } else if !reconcile_ready {
        format!("Reconcile is '{}'. Review /api/v1/reconcile/mismatches and resolve before start", reconcile_status_str)
    } else if arm_state == "disarmed_db" {
        "DB arm state is DISARMED. Re-arm: POST /api/v1/ops/action {\"action\":\"arm-execution\"}".to_string()
    } else if arm_state == "halted" {
        "Integrity halted. Arm: POST /api/v1/ops/action {\"action\":\"arm-execution\"} after resolving halt cause".to_string()
    } else if !signal_ingestion_configured {
        "Set MQK_STRATEGY_IDS to a registered strategy name and restart daemon".to_string()
    } else if strategy_fleet_empty {
        "Set MQK_STRATEGY_IDS to a registered strategy name (e.g. intraday_scalper) and restart daemon".to_string()
    } else if !ws_continuity_ready {
        "WS continuity is not live. Wait for WS reconnect or restart daemon to re-establish".to_string()
    } else if !runtime_start_allowed {
        "An autonomous session run is active. Monitor at /api/v1/autonomous/readiness".to_string()
    } else if !session_in_window {
        "System is ready. Awaiting market hours (NYSE regular session). No operator action needed.".to_string()
    } else {
        "All gates pass. System is ready for autonomous paper market session.".to_string()
    };

    (
        StatusCode::OK,
        Json(AutonomousPaperStatusResponse {
            canonical_route: "/api/v1/autonomous/paper-status".to_string(),
            truth_state: "active".to_string(),
            mode,
            live_routing_enabled,
            runtime_status,
            arm_state,
            kill_switch_active,
            deadman_status,
            ws_continuity: ws_continuity_str,
            reconcile_status: reconcile_status_str,
            mismatch_count,
            open_order_count,
            position_count,
            current_symbol,
            current_position_qty,
            target_qty,
            computed_delta_qty,
            no_order_reason: None,
            last_strategy_decision,
            flatten_available,
            flatten_blockers,
            evidence_ready: true,
            gui_visibility_ready: true,
            discord_visibility_ready: true,
            watchlist_outcome,
            watchlist_approved,
            readiness_classification: readiness_classification.to_string(),
            blockers,
            autonomous_session_state: autonomous_state_str,
            next_operator_action,
            now_utc,
        }),
    )
        .into_response()
}
