//! AUTON-PAPER-01: Autonomous session controller for Paper+Alpaca.
//!
//! Watches a configurable UTC daily session window and auto-starts / auto-stops
//! the execution runtime at the configured boundaries.  All start-gate logic
//! remains in `start_execution_runtime` — this controller calls that function
//! and responds to refusals; it does not bypass any gate.
//!
//! # Configuration
//!
//! | Env var                    | Example    | Meaning                          |
//! |----------------------------|------------|----------------------------------|
//! | `MQK_SESSION_START_HH_MM`  | `"14:30"`  | Session open (UTC, HH:MM format) |
//! | `MQK_SESSION_STOP_HH_MM`   | `"21:00"`  | Session close (UTC, HH:MM format)|
//!
//! When either variable is absent or invalid the controller is disabled and
//! the operator manages start/stop manually — backward compatible.
//!
//! # Session logic
//!
//! - **Auto-start**: on every poll tick while in-session and no active run,
//!   calls `start_execution_runtime`.  Refused starts (gate failures) are
//!   logged and Discord-alerted; the controller retries on the next tick.
//!
//! - **Auto-stop**: when the clock crosses the session stop boundary while
//!   the controller owns the active run, calls `stop_execution_runtime`.
//!
//! - **Recovery**: if the controller started a run and it ends unexpectedly
//!   (WS gap halt, deadman, orchestrator error), the controller detects the
//!   missing `locally_owned_run_id` and immediately retries start.  The
//!   BRK-00R-04 gate will refuse if WS continuity is not yet re-established,
//!   so the retry loop naturally waits for the WS transport to reconnect and
//!   advance to `Live` before a new run can start.  REST catch-up then occurs
//!   via the orchestrator Phase 2 polling from `rest_activity_after` in the
//!   persisted cursor (BRK-07R).
//!
//! - **Operator-managed runs**: if a run is active that the controller did not
//!   start, it does not stop it.  The operator owns that run.
//!
//! # Deployment guard
//!
//! Only activates for `Paper` + `ExternalSignalIngestion` (i.e., Paper+Alpaca).
//! Returns `None` for all other deployments.
//!
//! # Discord alerts emitted
//!
//! | Event                         | Payload type            | Severity  |
//! |-------------------------------|-------------------------|-----------|
//! | Auto-start success            | `notify_operator_action`| —         |
//! | Auto-start refused (gate)     | `notify_critical_alert` | warning   |
//! | Run ended unexpectedly        | `notify_critical_alert` | warning   |
//! | Auto-stop at session boundary | `notify_run_status`     | —         |

use std::sync::Arc;
use std::time::Duration;

use chrono::{Timelike, Utc};
use tracing::{info, warn};

use super::types::{DeploymentMode, StrategyMarketDataSource};
use super::AppState;
use crate::notify::{CriticalAlertPayload, OperatorNotifyPayload, RunStatusPayload};

// ---------------------------------------------------------------------------
// Configuration constants
// ---------------------------------------------------------------------------

/// UTC session start time env var.  Format: `"HH:MM"` (e.g. `"14:30"`).
pub const SESSION_START_HH_MM_ENV: &str = "MQK_SESSION_START_HH_MM";

/// UTC session stop time env var.  Format: `"HH:MM"` (e.g. `"21:00"`).
pub const SESSION_STOP_HH_MM_ENV: &str = "MQK_SESSION_STOP_HH_MM";

/// Controller poll interval.  30 seconds is sufficient for minute-resolution
/// session boundaries while keeping resource usage negligible.
const SESSION_POLL_INTERVAL: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// SessionWindow
// ---------------------------------------------------------------------------

/// Parsed UTC daily session window [start, stop).
///
/// Both bounds are given as (hour, minute) in UTC.  Overnight windows
/// (stop < start) are not supported; the parser rejects them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionWindow {
    pub start_hh: u32,
    pub start_mm: u32,
    pub stop_hh: u32,
    pub stop_mm: u32,
}

impl SessionWindow {
    /// Parse from "HH:MM" strings.  Returns `None` on any parse failure or
    /// if `start >= stop` (zero-duration or overnight window).
    pub fn parse(start: &str, stop: &str) -> Option<Self> {
        let (sh, sm) = parse_hh_mm(start)?;
        let (eh, em) = parse_hh_mm(stop)?;
        if (sh, sm) >= (eh, em) {
            warn!(
                start = start,
                stop = stop,
                "session_controller: start >= stop; overnight windows unsupported; \
                 autonomous session disabled (set MQK_SESSION_START_HH_MM < MQK_SESSION_STOP_HH_MM)"
            );
            return None;
        }
        Some(SessionWindow {
            start_hh: sh,
            start_mm: sm,
            stop_hh: eh,
            stop_mm: em,
        })
    }

    /// Returns `true` when `now` falls in `[start, stop)` on the current UTC day.
    pub fn is_in_session(&self, now: chrono::DateTime<Utc>) -> bool {
        let now_mins = now.hour() * 60 + now.minute();
        let start_mins = self.start_hh * 60 + self.start_mm;
        let stop_mins = self.stop_hh * 60 + self.stop_mm;
        now_mins >= start_mins && now_mins < stop_mins
    }
}

fn parse_hh_mm(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.splitn(2, ':');
    let hh: u32 = parts.next()?.parse().ok()?;
    let mm: u32 = parts.next()?.parse().ok()?;
    if hh > 23 || mm > 59 {
        return None;
    }
    Some((hh, mm))
}

/// Resolve the session window from environment variables.
///
/// Returns `None` when either variable is absent, empty, or invalid.
pub fn session_window_from_env() -> Option<SessionWindow> {
    let start = std::env::var(SESSION_START_HH_MM_ENV)
        .ok()
        .filter(|s| !s.is_empty())?;
    let stop = std::env::var(SESSION_STOP_HH_MM_ENV)
        .ok()
        .filter(|s| !s.is_empty())?;
    SessionWindow::parse(&start, &stop)
}

// ---------------------------------------------------------------------------
// spawn_autonomous_session_controller
// ---------------------------------------------------------------------------

/// Spawn the autonomous session controller background task.
///
/// Returns `None` when:
/// - Deployment is not Paper+Alpaca (`ExternalSignalIngestion`).
/// - `MQK_SESSION_START_HH_MM` or `MQK_SESSION_STOP_HH_MM` is absent/invalid.
///
/// The caller must retain the returned `JoinHandle` for the lifetime of the
/// daemon; dropping it cancels the task.
pub fn spawn_autonomous_session_controller(
    state: Arc<AppState>,
) -> Option<tokio::task::JoinHandle<()>> {
    if state.deployment_mode() != DeploymentMode::Paper {
        return None;
    }
    if state.strategy_market_data_source() != StrategyMarketDataSource::ExternalSignalIngestion {
        return None;
    }
    let window = session_window_from_env()?;
    info!(
        start = format!("{:02}:{:02} UTC", window.start_hh, window.start_mm),
        stop  = format!("{:02}:{:02} UTC", window.stop_hh, window.stop_mm),
        "autonomous_session_controller: enabled (AUTON-PAPER-01)"
    );
    Some(tokio::spawn(run_session_controller(state, window)))
}

// ---------------------------------------------------------------------------
// Core controller loop
// ---------------------------------------------------------------------------

async fn run_session_controller(state: Arc<AppState>, window: SessionWindow) {
    let mut ticker = tokio::time::interval(SESSION_POLL_INTERVAL);

    // Whether this controller instance was the one that started the current run.
    // Scoped to this task's lifetime — not persisted across daemon restarts.
    let mut locally_started = false;

    loop {
        ticker.tick().await;

        let now = Utc::now();
        let in_session = window.is_in_session(now);
        let has_active_run = state.locally_owned_run_id().await.is_some();

        match (in_session, locally_started, has_active_run) {
            // ── Normal: in-session, our run is running ───────────────────────
            (true, true, true) => {
                // Nothing to do — let the execution loop run.
            }

            // ── Recovery: in-session, we started it, but the run ended ───────
            //
            // The execution loop exits on WS gap halt, deadman expiry, or
            // orchestrator error.  Reset our flag and retry start on the next
            // iteration so the BRK-00R-04 gate can naturally gate-keep until
            // the WS transport re-establishes Live continuity.
            (true, true, false) => {
                locally_started = false;
                let env = env_label(&state);
                warn!(
                    "autonomous_session_controller: run ended unexpectedly during session window; \
                     will attempt recovery restart on next tick"
                );
                state
                    .discord_notifier
                    .notify_critical_alert(&CriticalAlertPayload {
                        alert_class: "autonomous.session.run_ended_unexpectedly".to_string(),
                        severity: "warning".to_string(),
                        summary: "Autonomous paper session: run ended unexpectedly during session \
                                  window. Will retry start (BRK-00R-04 will gate-keep until WS \
                                  re-establishes Live; REST catch-up from persisted cursor follows)."
                            .to_string(),
                        detail: None,
                        environment: env,
                        run_id: None,
                        ts_utc: now.to_rfc3339(),
                    })
                    .await;
            }

            // ── Auto-start: in-session, no active run ────────────────────────
            //
            // Also covers the recovery case after the flag was reset above.
            (true, false, _) => {
                attempt_auto_start(&state, now, &mut locally_started).await;
            }

            // ── Auto-stop: session boundary crossed, our run is running ──────
            (false, true, true) => {
                attempt_auto_stop(&state, now, &mut locally_started).await;
            }

            // ── Out of session, operator-managed run active — don't touch ────
            (false, false, true) => {}

            // ── Out of session, no run — idle ────────────────────────────────
            (false, _, false) => {
                // Defensive: if locally_started was true but run is gone and
                // session is over, clear the flag silently.
                locally_started = false;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn attempt_auto_start(
    state: &Arc<AppState>,
    now: chrono::DateTime<Utc>,
    locally_started: &mut bool,
) {
    let env = env_label(state);

    // AUTON-PAPER-01B: Attempt autonomous arm before start.
    //
    // Auto-arm succeeds only when DB arm state = ARMED (clean prior stop).
    // Refuses when: operator halted, no DB, no prior row (first-time install),
    // or DB state = DISARMED for any reason.
    //
    // Logged at info — arm refusal is expected at first install (before the
    // operator arms manually for the first time) and should not spam Discord.
    // The start_refused Discord alert below fires when start itself is the
    // blocker; arm refusal is a quieter pre-start condition.
    if let Err(reason) = state.try_autonomous_arm().await {
        info!(
            reason = %reason,
            "autonomous_session_controller: autonomous arm not possible on this tick; will retry"
        );
        return;
    }

    match state.start_execution_runtime().await {
        Ok(snap) => {
            *locally_started = true;
            let run_id = snap.active_run_id.map(|id| id.to_string());
            info!(
                run_id = ?run_id,
                "autonomous_session_controller: auto-started execution run"
            );
            state
                .discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "autonomous.run.start".to_string(),
                    disposition: "applied".to_string(),
                    environment: env,
                    ts_utc: now.to_rfc3339(),
                    provenance_ref: run_id.clone(),
                    run_id,
                })
                .await;
        }
        Err(err) => {
            // Gate refusal is expected during WS cold-start, gap recovery,
            // or when the operator has not yet armed.  Log at warn and alert
            // Discord; the controller will retry on the next tick.
            let detail = format!(
                "fault_class={} error={}",
                err.fault_class(),
                err
            );
            warn!(
                detail = %detail,
                "autonomous_session_controller: auto-start refused; will retry"
            );
            state
                .discord_notifier
                .notify_critical_alert(&CriticalAlertPayload {
                    alert_class: "autonomous.session.start_refused".to_string(),
                    severity: "warning".to_string(),
                    summary:
                        "Autonomous paper session start refused; will retry on next poll tick."
                            .to_string(),
                    detail: Some(detail),
                    environment: env,
                    run_id: None,
                    ts_utc: now.to_rfc3339(),
                })
                .await;
        }
    }
}

async fn attempt_auto_stop(
    state: &Arc<AppState>,
    now: chrono::DateTime<Utc>,
    locally_started: &mut bool,
) {
    let env = env_label(state);
    let run_id_before = state.locally_owned_run_id().await.map(|id| id.to_string());
    match state.stop_execution_runtime().await {
        Ok(_) => {
            *locally_started = false;
            info!(
                run_id = ?run_id_before,
                "autonomous_session_controller: auto-stopped at session boundary"
            );
            state
                .discord_notifier
                .notify_run_status(&RunStatusPayload {
                    event: "autonomous.run.stop".to_string(),
                    run_id: run_id_before,
                    environment: env,
                    note: Some("session boundary reached".to_string()),
                    ts_utc: now.to_rfc3339(),
                })
                .await;
        }
        Err(err) => {
            // Transient failure — log and retry on next tick.
            warn!(
                error = %err,
                "autonomous_session_controller: auto-stop failed; will retry"
            );
        }
    }
}

fn env_label(state: &AppState) -> Option<String> {
    Some(state.deployment_mode().as_api_label().to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // SW-01: parse valid window
    #[test]
    fn sw01_parse_valid_window() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        assert_eq!((w.start_hh, w.start_mm), (14, 30));
        assert_eq!((w.stop_hh, w.stop_mm), (21, 0));
    }

    // SW-02: start == stop is rejected (zero-duration)
    #[test]
    fn sw02_start_equals_stop_rejected() {
        assert!(SessionWindow::parse("14:30", "14:30").is_none());
    }

    // SW-03: start > stop rejected (overnight not supported)
    #[test]
    fn sw03_start_after_stop_rejected() {
        assert!(SessionWindow::parse("21:00", "14:30").is_none());
    }

    // SW-04: invalid format rejected
    #[test]
    fn sw04_invalid_format_rejected() {
        assert!(SessionWindow::parse("bad", "21:00").is_none());
        assert!(SessionWindow::parse("14:30", "").is_none());
        assert!(SessionWindow::parse("25:00", "21:00").is_none());
        assert!(SessionWindow::parse("14:60", "21:00").is_none());
    }

    // SW-05: is_in_session: inside window
    #[test]
    fn sw05_is_in_session_inside() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        // 15:00 UTC
        let ts = Utc.with_ymd_and_hms(2026, 3, 30, 15, 0, 0).unwrap();
        assert!(w.is_in_session(ts));
    }

    // SW-06: is_in_session: exactly at start boundary (inclusive)
    #[test]
    fn sw06_is_in_session_at_start() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 30, 14, 30, 0).unwrap();
        assert!(w.is_in_session(ts));
    }

    // SW-07: is_in_session: exactly at stop boundary (exclusive)
    #[test]
    fn sw07_is_in_session_at_stop_exclusive() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 30, 21, 0, 0).unwrap();
        assert!(!w.is_in_session(ts));
    }

    // SW-08: is_in_session: before window start
    #[test]
    fn sw08_is_in_session_before_start() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 30, 9, 0, 0).unwrap();
        assert!(!w.is_in_session(ts));
    }

    // SW-09: is_in_session: after window stop
    #[test]
    fn sw09_is_in_session_after_stop() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 30, 22, 0, 0).unwrap();
        assert!(!w.is_in_session(ts));
    }
}
