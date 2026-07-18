//! mqk-daemon entry point.
//!
//! This file is intentionally thin: it sets up tracing, builds the shared
//! state, wires middleware, and starts the HTTP server. All route handlers
//! live in `routes.rs`; all shared state types live in `state.rs`.

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use mqk_daemon::{bind, cors, routes, state};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{info, warn, Level};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env.local before init_tracing; store outcome so it can be logged
    // after the subscriber is ready (BOOT-VALID-01).
    let env_file_outcome = dotenvy::from_filename(".env.local");

    init_tracing();

    // BOOT-VALID-01: Log .env.local load outcome explicitly.
    //
    // A silent "not found" previously hid the root cause of missing credentials.
    // Now the operator sees a clear warn listing the vars that must come from
    // the system environment when the file is absent.
    match env_file_outcome {
        Ok(_) => {
            info!(file = ".env.local", "env_file: loaded");
        }
        Err(ref e) => {
            let is_not_found = if let dotenvy::Error::Io(ref io_err) = e {
                io_err.kind() == std::io::ErrorKind::NotFound
            } else {
                false
            };
            if is_not_found {
                warn!(
                    file = ".env.local",
                    hint = "ALPACA_API_KEY_PAPER ALPACA_API_SECRET_PAPER \
                            ALPACA_PAPER_BASE_URL MQK_DATABASE_URL",
                    "env_file: not_found — env vars must be set via system environment \
                     if .env.local is absent (BOOT-VALID-01)"
                );
            } else {
                warn!(
                    file = ".env.local",
                    error = %e,
                    "env_file: load_error — vars may be absent or malformed (BOOT-VALID-01)"
                );
            }
        }
    }

    let db = mqk_db::connect_from_env()
        .await
        .context("mqk-daemon requires MQK_DATABASE_URL for real runtime lifecycle control")?;

    let shared = Arc::new(state::AppState::new_with_db(db));

    // BRK-07R: Seed WS continuity from last persisted cursor before WS task
    // starts.  If the prior session ended with GapDetected the BRK-00R-04 gate
    // will immediately block start rather than starting as ColdStartUnproven.
    shared.seed_ws_continuity_from_db().await;

    // BROKER-POSITION-BASELINE-ADOPTION-01: Seed the broker position baseline
    // from DB so the reconcile tick's local_fn uses it immediately on the first
    // tick when no execution run is active.  Must run before spawn_reconcile_tick.
    shared.seed_broker_baseline_from_db().await;

    match shared.operator_auth_mode() {
        state::OperatorAuthMode::TokenRequired(_) => {
            info!(
                operator_auth = shared.operator_auth_mode().label(),
                "operator auth configured; privileged routes require Bearer token"
            );
        }
        state::OperatorAuthMode::ExplicitDevNoToken => {
            warn!(operator_auth = shared.operator_auth_mode().label(), "explicit debug-only no-token operator mode enabled; do not treat loopback bind as sufficient authorization");
        }
        state::OperatorAuthMode::MissingTokenFailClosed => {
            warn!(operator_auth = shared.operator_auth_mode().label(), "operator token missing; privileged routes will fail closed until MQK_OPERATOR_TOKEN is configured");
        }
    }
    state::spawn_heartbeat(shared.bus.clone(), Duration::from_secs(1));

    // BRK-GAP-REST-AUTO-TRIGGER-01: If the persisted WS cursor is GapDetected,
    // attempt REST gap fill recovery before the WS transport starts.
    // Fires only when DB, fetcher, and a non-halted prior run are all present.
    // Failure is warn-only — GapDetected already blocks start_execution_runtime
    // via BRK-00R-04; this path only inserts missed inbox rows.
    if let Some(ref outcome) = shared.try_ws_gap_auto_recovery().await {
        if outcome.gate.is_some() {
            tracing::warn!(
                run_id = %outcome.run_id,
                gate = ?outcome.gate,
                evidence = %outcome.evidence,
                "BRK-GAP-REST-AUTO-TRIGGER-01: startup gap recovery refused by internal gate"
            );
        } else {
            tracing::info!(
                run_id = %outcome.run_id,
                recovered = outcome.recovered_count,
                already_present = outcome.already_present_count,
                unknown_order = outcome.unknown_order_count,
                malformed = outcome.malformed_count,
                cursor_advanced = outcome.cursor_advanced,
                "BRK-GAP-REST-AUTO-TRIGGER-01: startup gap fill recovery completed"
            );
        }
    }

    // BRK-00R-05: Spawn the Alpaca paper WS transport if configured for paper+alpaca.
    // The handle is kept alive for the lifetime of the daemon.
    let _alpaca_ws_handle = state::spawn_alpaca_paper_ws_task(Arc::clone(&shared));

    // AUTON-PAPER-01: Spawn the autonomous session controller for Paper+Alpaca.
    // No-op for non-paper-alpaca deployments or when session env vars are absent.
    let session_controller_handle = state::spawn_autonomous_session_controller(Arc::clone(&shared));

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-COMPLETED-BAR-TASK-CUTOVER-AND-SUPERVISION:
    // spawn the supervised completed-bar driver task for Paper+Alpaca in
    // place of the legacy blind-timer autonomous bar ticker. Exactly one
    // completed-bar task may be active per daemon process (D3.6/D3.7); the
    // legacy ticker (`state::autonomous_bar_ticker`) remains in source for
    // compatibility/testing only and is never spawned in production.
    let completed_bar_task_outcome =
        state::spawn_autonomous_completed_bar_driver_task(Arc::clone(&shared)).await;

    // BOOT-VALID-01: Explicit startup task outcome log.
    //
    // For paper+alpaca all three autonomous tasks must start.  If the WS
    // transport started (credentials present) but the session controller or
    // the completed-bar task did not, that is a configuration inconsistency:
    // autonomous paper execution will not self-manage and the operator would
    // otherwise see no indication of this from the startup logs.
    {
        let ws_started = _alpaca_ws_handle.is_some();
        let ctrl_started = session_controller_handle.is_some();
        let completed_bar_task_started = matches!(
            completed_bar_task_outcome,
            state::AutonomousCompletedBarTaskSpawnOutcome::Started
        );

        if ws_started && (!ctrl_started || !completed_bar_task_started) {
            warn!(
                alpaca_ws_started = ws_started,
                session_controller_started = ctrl_started,
                completed_bar_task_outcome = ?completed_bar_task_outcome,
                "startup_truth: WS transport started but autonomous session controller \
                 or the completed-bar driver task did NOT start — autonomous paper \
                 execution will not self-manage; verify ExternalSignalIngestion config \
                 (BOOT-VALID-01)"
            );
        } else {
            info!(
                alpaca_ws_started = ws_started,
                session_controller_started = ctrl_started,
                completed_bar_task_outcome = ?completed_bar_task_outcome,
                "startup_truth: background task start outcomes (BOOT-VALID-01)"
            );
        }
    }

    // EXEC-OBS-LIVENESS-01: Session-controller death watchdog.
    //
    // `run_session_controller` is an infinite loop that must never exit.
    // If the Tokio task panics the JoinHandle resolves to Err(JoinError).
    // Without this watchdog, controller death is silent: the daemon stays up,
    // `autonomous_session_truth` freezes at its last value, and
    // /api/v1/autonomous/readiness returns a stale state that looks like
    // "waiting for session window" rather than "controller is dead".
    //
    // The watchdog awaits the handle, fires an error log, and records
    // ControllerExited in autonomous_session_truth so the API surface reflects
    // the real situation and an unattended operator can detect the failure.
    if let Some(handle) = session_controller_handle {
        let watchdog_state = Arc::clone(&shared);
        tokio::spawn(async move {
            let result = handle.await;
            tracing::error!(
                result_ok = result.is_ok(),
                "session_controller_task_exited: autonomous session controller stopped \
                 unexpectedly; unattended paper execution is now UNMANAGED \
                 (EXEC-OBS-LIVENESS-01)"
            );
            let detail = match &result {
                Ok(_) => {
                    "session controller task exited normally (unexpected — loop should be infinite)"
                        .to_string()
                }
                Err(e) => format!("session controller task panicked: {e}"),
            };
            watchdog_state
                .set_autonomous_session_truth(state::AutonomousSessionTruth::ControllerExited {
                    detail,
                })
                .await;
        });
    }

    let app = routes::build_router(Arc::clone(&shared))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(cors::gui_cors_layer());

    let addr = bind::resolve_bind_addr_from_env()?;
    info!("mqk-daemon listening on http://{}", addr);

    let shutdown_state = Arc::clone(&shared);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            // D3.10 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-SUPERVISOR-AND-
            // CRITICAL-OUTCOME-CLOSURE-01 REPAIR 6): signal cancellation of
            // the completed-bar task AND await the supervised task's
            // completion (bounded) before the execution-runtime shutdown
            // path runs — no tick can begin or remain in progress once this
            // returns, and no detached worker survives shutdown.
            shutdown_state
                .cancel_and_wait_completed_bar_task_for_shutdown()
                .await;
            shutdown_state.stop_for_shutdown().await;
        })
        .await
        .context("server crashed")?;

    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
}
