//! AUTON-PAPER-01: Autonomous session controller for Paper+Alpaca.
//!
//! Watches an autonomous session schedule and auto-starts / auto-stops the
//! execution runtime at the configured boundaries. All start-gate logic
//! remains in `start_execution_runtime` — this controller calls that function
//! and responds to refusals; it does not bypass any gate.
//!
//! # Scheduling contract
//!
//! Default: `AutonomousSessionSchedule::NyseRegularSession`
//! - Uses `CalendarSpec::NyseWeekdays` with the daemon's session-clock seam.
//! - DST-safe, holiday-safe, and testable via `AppState::set_session_clock_ts_for_test`.
//! - Half-day handling remains limited by the current calendar seam; no fake
//!   half-day support is claimed here.
//!
//! Optional legacy override:
//! - `MQK_SESSION_START_HH_MM`
//! - `MQK_SESSION_STOP_HH_MM`
//!
//! When both override env vars are present and valid, the controller uses a
//! fixed UTC window instead of the NYSE regular-session seam. This preserves
//! backward compatibility for operator-driven overrides without forcing the
//! autonomous paper path to stay tied to raw UTC windows.
//!
//! # Session logic
//!
//! - **Auto-start**: on every poll tick while in-session and no active run,
//!   calls `start_execution_runtime`. Refused starts (gate failures) are
//!   logged and Discord-alerted; the controller retries on the next tick.
//! - **Auto-stop**: when the session closes while the controller owns the
//!   active run, calls `stop_execution_runtime`.
//! - **Recovery**: if the controller started a run and it ends unexpectedly
//!   (WS gap halt, deadman, orchestrator error), the controller detects the
//!   missing `locally_owned_run_id` and retries start. The BRK-00R-04 gate
//!   keeps that fail-closed until WS continuity is re-established.
//! - **Operator-managed runs**: if a run is active that the controller did not
//!   start, it does not stop it.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Timelike, Utc};
use mqk_integrity::CalendarSpec;
use tracing::{info, warn};

use super::runtime_session_source::{
    evaluate_runtime_session_source_active_decision, runtime_session_source_mode_from_env,
    RuntimeSessionSourceMode,
};
use super::types::{AutonomousSessionTruth, DeploymentMode, StrategyMarketDataSource};
use super::AppState;
use crate::notify::{CriticalAlertPayload, OperatorNotifyPayload, RunStatusPayload};

/// UTC session start time env var. Format: `"HH:MM"`.
pub const SESSION_START_HH_MM_ENV: &str = "MQK_SESSION_START_HH_MM";
/// UTC session stop time env var. Format: `"HH:MM"`.
pub const SESSION_STOP_HH_MM_ENV: &str = "MQK_SESSION_STOP_HH_MM";
const SESSION_POLL_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutonomousSessionSchedule {
    FixedUtcWindow(SessionWindow),
    NyseRegularSession,
}

impl AutonomousSessionSchedule {
    /// The single production in-window/outside-window decision consumed by
    /// the autonomous session controller tick, `/api/v1/system/preflight`,
    /// and `/api/v1/autonomous/paper-status` (`routes/autonomous_paper_status.rs`).
    ///
    /// ASSET-CORE-05E: the `NyseRegularSession` arm additionally checks
    /// `MQK_RUNTIME_SESSION_SOURCE`. By default (unset, `"legacy"`, or
    /// `"v2_equity_shadow"`) this performs exactly the original legacy
    /// calendar check below — zero new env reads' effect, no v2 registry
    /// load, no behavior change. Only an explicit
    /// `"v2_equity_active"` selection evaluates the v2 equity candidate; if
    /// it proves safe, its in-window boolean is authoritative for this call,
    /// and if it is refused, this returns `false` (fail closed) rather than
    /// silently falling back to the legacy boolean. `FixedUtcWindow` (an
    /// explicit, non-calendar operator override) is never affected by this
    /// hook.
    pub async fn is_in_session(&self, state: &Arc<AppState>, now: chrono::DateTime<Utc>) -> bool {
        match self {
            Self::FixedUtcWindow(window) => window.is_in_session(now),
            Self::NyseRegularSession => {
                let ts = state.session_now_ts().await;
                let legacy_in_session =
                    CalendarSpec::NyseWeekdays.classify_market_session(ts) == "regular";

                if runtime_session_source_mode_from_env().resolved_mode()
                    != RuntimeSessionSourceMode::V2EquityActive
                {
                    return legacy_in_session;
                }

                let now_for_eval = DateTime::<Utc>::from_timestamp(ts, 0).unwrap_or(now);
                evaluate_runtime_session_source_active_decision(
                    &state.instrument_registry_path,
                    now_for_eval,
                )
                .in_session_fail_closed()
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionWindow {
    pub start_hh: u32,
    pub start_mm: u32,
    pub stop_hh: u32,
    pub stop_mm: u32,
}

impl SessionWindow {
    pub fn parse(start: &str, stop: &str) -> Option<Self> {
        let (sh, sm) = parse_hh_mm(start)?;
        let (eh, em) = parse_hh_mm(stop)?;
        if (sh, sm) >= (eh, em) {
            warn!(
                start = start,
                stop = stop,
                "session_controller: start >= stop; overnight windows unsupported; autonomous session disabled"
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

pub fn session_window_from_env() -> Option<SessionWindow> {
    let start = std::env::var(SESSION_START_HH_MM_ENV)
        .ok()
        .filter(|s| !s.is_empty())?;
    let stop = std::env::var(SESSION_STOP_HH_MM_ENV)
        .ok()
        .filter(|s| !s.is_empty())?;
    SessionWindow::parse(&start, &stop)
}

pub fn autonomous_session_schedule_from_env() -> AutonomousSessionSchedule {
    session_window_from_env()
        .map(AutonomousSessionSchedule::FixedUtcWindow)
        .unwrap_or(AutonomousSessionSchedule::NyseRegularSession)
}

pub fn spawn_autonomous_session_controller(
    state: Arc<AppState>,
) -> Option<tokio::task::JoinHandle<()>> {
    if state.deployment_mode() != DeploymentMode::Paper {
        return None;
    }
    if state.strategy_market_data_source() != StrategyMarketDataSource::ExternalSignalIngestion {
        return None;
    }
    let schedule = autonomous_session_schedule_from_env();
    match schedule {
        AutonomousSessionSchedule::FixedUtcWindow(window) => info!(
            start = format!("{:02}:{:02} UTC", window.start_hh, window.start_mm),
            stop = format!("{:02}:{:02} UTC", window.stop_hh, window.stop_mm),
            "autonomous_session_controller: enabled with fixed UTC window override (AUTON-PAPER-01)"
        ),
        AutonomousSessionSchedule::NyseRegularSession => info!(
            "autonomous_session_controller: enabled with NYSE regular-session truth seam (AUTON-PAPER-01)"
        ),
    }
    Some(tokio::spawn(run_session_controller(state, schedule)))
}

async fn run_session_controller(state: Arc<AppState>, schedule: AutonomousSessionSchedule) {
    let mut ticker = tokio::time::interval(SESSION_POLL_INTERVAL);
    let mut locally_started = false;

    loop {
        ticker.tick().await;
        run_session_controller_tick(&state, schedule, &mut locally_started, Utc::now()).await;
    }
}

pub async fn run_session_controller_tick(
    state: &Arc<AppState>,
    schedule: AutonomousSessionSchedule,
    locally_started: &mut bool,
    now: chrono::DateTime<Utc>,
) {
    let in_session = schedule.is_in_session(state, now).await;
    let has_active_run = state.locally_owned_run_id().await.is_some();

    match (in_session, *locally_started, has_active_run) {
        (true, true, true) => {}
        (true, true, false) => {
            *locally_started = false;
            let env = env_label(state);
            state
                .set_autonomous_session_truth(AutonomousSessionTruth::RunEndedUnexpectedly {
                    detail: "autonomous paper run ended unexpectedly during the session window; controller will retry start on the next tick".to_string(),
                })
                .await;
            warn!(
                "autonomous_session_controller: run ended unexpectedly during session window; will attempt recovery restart on next tick"
            );
            state
                .discord_notifier
                .notify_critical_alert(&CriticalAlertPayload {
                    alert_class: "autonomous.session.run_ended_unexpectedly".to_string(),
                    severity: "warning".to_string(),
                    summary: "Autonomous paper session: run ended unexpectedly during session window. Will retry start (BRK-00R-04 will gate-keep until WS re-establishes Live; REST catch-up from persisted cursor follows).".to_string(),
                    detail: None,
                    environment: env,
                    run_id: None,
                    ts_utc: now.to_rfc3339(),
                })
                .await;
        }
        (true, false, _) => {
            attempt_auto_start(state, now, locally_started).await;
        }
        (false, true, true) => {
            attempt_auto_stop(state, now, locally_started).await;
        }
        (false, false, true) => {}
        (false, _, false) => {
            *locally_started = false;
            state.clear_autonomous_session_truth().await;
        }
    }
}

async fn attempt_auto_start(
    state: &Arc<AppState>,
    now: chrono::DateTime<Utc>,
    locally_started: &mut bool,
) {
    let env = env_label(state);

    if let Err(reason) = state.try_autonomous_arm().await {
        state
            .set_autonomous_session_truth(AutonomousSessionTruth::StartRefused {
                detail: reason.clone(),
            })
            .await;
        info!(
            reason = %reason,
            "autonomous_session_controller: autonomous arm not possible on this tick; will retry"
        );
        return;
    }

    match state.start_execution_runtime().await {
        Ok(snap) => {
            *locally_started = true;
            let current_truth = state.autonomous_session_truth().await;
            // BRK-GAP-01: preserve WsGapPartialRecovery after start so the
            // operator surface continues to show that non-fill lifecycle events
            // from the gap window remain permanently unrecoverable.
            if !matches!(
                current_truth,
                AutonomousSessionTruth::RecoverySucceeded { .. }
                    | AutonomousSessionTruth::WsGapPartialRecovery { .. }
            ) {
                state.clear_autonomous_session_truth().await;
            }
            let run_id = snap.active_run_id.map(|id| id.to_string());
            info!(run_id = ?run_id, "autonomous_session_controller: auto-started execution run");
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
            let detail = format!("fault_class={} error={}", err.fault_class(), err);
            state
                .set_autonomous_session_truth(AutonomousSessionTruth::StartRefused {
                    detail: detail.clone(),
                })
                .await;
            warn!(detail = %detail, "autonomous_session_controller: auto-start refused; will retry");
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
            state
                .set_autonomous_session_truth(AutonomousSessionTruth::StoppedAtBoundary {
                    detail: "autonomous paper run stopped at the configured session boundary"
                        .to_string(),
                })
                .await;
            info!(run_id = ?run_id_before, "autonomous_session_controller: auto-stopped at session boundary");
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
            state
                .set_autonomous_session_truth(AutonomousSessionTruth::StopFailed {
                    detail: err.to_string(),
                })
                .await;
            warn!(error = %err, "autonomous_session_controller: auto-stop failed; will retry");
        }
    }
}

fn env_label(state: &AppState) -> Option<String> {
    Some(state.deployment_mode().as_api_label().to_string())
}

#[cfg(test)]
mod tests {
    use super::super::runtime_session_source::{
        RUNTIME_SESSION_SOURCE_ENV, RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK,
    };
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn sw01_parse_valid_window() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        assert_eq!((w.start_hh, w.start_mm), (14, 30));
        assert_eq!((w.stop_hh, w.stop_mm), (21, 0));
    }

    #[test]
    fn sw02_start_equals_stop_rejected() {
        assert!(SessionWindow::parse("14:30", "14:30").is_none());
    }

    #[test]
    fn sw03_start_after_stop_rejected() {
        assert!(SessionWindow::parse("21:00", "14:30").is_none());
    }

    #[test]
    fn sw04_invalid_format_rejected() {
        assert!(SessionWindow::parse("bad", "21:00").is_none());
        assert!(SessionWindow::parse("14:30", "").is_none());
        assert!(SessionWindow::parse("25:00", "21:00").is_none());
        assert!(SessionWindow::parse("14:60", "21:00").is_none());
    }

    #[test]
    fn sw05_is_in_session_inside() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 30, 15, 0, 0).unwrap();
        assert!(w.is_in_session(ts));
    }

    #[test]
    fn sw06_is_in_session_at_start() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 30, 14, 30, 0).unwrap();
        assert!(w.is_in_session(ts));
    }

    #[test]
    fn sw07_is_in_session_at_stop_exclusive() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 30, 21, 0, 0).unwrap();
        assert!(!w.is_in_session(ts));
    }

    #[test]
    fn sw08_is_in_session_before_start() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 30, 9, 0, 0).unwrap();
        assert!(!w.is_in_session(ts));
    }

    #[test]
    fn sw09_is_in_session_after_stop() {
        let w = SessionWindow::parse("14:30", "21:00").unwrap();
        let ts = Utc.with_ymd_and_hms(2026, 3, 30, 22, 0, 0).unwrap();
        assert!(!w.is_in_session(ts));
    }

    #[test]
    fn sw09b_default_schedule_uses_nyse_regular_session() {
        std::env::remove_var(SESSION_START_HH_MM_ENV);
        std::env::remove_var(SESSION_STOP_HH_MM_ENV);
        assert_eq!(
            autonomous_session_schedule_from_env(),
            AutonomousSessionSchedule::NyseRegularSession
        );
    }

    #[tokio::test]
    async fn sw10_out_of_session_idle_clears_autonomous_session_truth() {
        let state = Arc::new(AppState::new_for_test_with_broker_kind(
            super::super::types::BrokerKind::Alpaca,
        ));
        state
            .set_autonomous_session_truth(AutonomousSessionTruth::RecoverySucceeded {
                resume_source: super::super::types::AutonomousRecoveryResumeSource::PersistedCursor,
                detail: "recovered".to_string(),
            })
            .await;
        let schedule = AutonomousSessionSchedule::FixedUtcWindow(
            SessionWindow::parse("14:30", "21:00").unwrap(),
        );
        let now = Utc.with_ymd_and_hms(2026, 3, 30, 22, 0, 0).unwrap();
        let mut locally_started = false;

        run_session_controller_tick(&state, schedule, &mut locally_started, now).await;

        assert_eq!(
            state.autonomous_session_truth().await,
            AutonomousSessionTruth::Clear,
            "out-of-session idle must clear stale autonomous session truth"
        );
    }

    #[tokio::test]
    async fn sw11_nyse_regular_session_weekday_is_in_session() {
        // ASSET-CORE-05E: is_in_session's NyseRegularSession arm now reads
        // RUNTIME_SESSION_SOURCE_ENV. Take the shared lock and force it
        // unset so this pre-existing legacy-path test cannot observe a
        // transient v2_equity_active mutation from a concurrently-running
        // test in this same `--lib` binary (sw16-20 / runtime_session_source
        // tests share this same process-global env var).
        let _guard = RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
        let state = Arc::new(AppState::new_for_test_with_broker_kind(
            super::super::types::BrokerKind::Alpaca,
        ));
        // Monday 2026-03-30 14:00:00 UTC = 10:00:00 ET (regular session, DST).
        state
            .set_session_clock_ts_for_test(
                Utc.with_ymd_and_hms(2026, 3, 30, 14, 0, 0)
                    .unwrap()
                    .timestamp(),
            )
            .await;

        let in_session = AutonomousSessionSchedule::NyseRegularSession
            .is_in_session(&state, Utc.with_ymd_and_hms(2026, 3, 30, 14, 0, 0).unwrap())
            .await;
        assert!(
            in_session,
            "NYSE regular weekday session must be in-session"
        );
    }

    #[tokio::test]
    async fn sw12_nyse_premarket_is_out_of_session() {
        let _guard = RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
        let state = Arc::new(AppState::new_for_test_with_broker_kind(
            super::super::types::BrokerKind::Alpaca,
        ));
        // Monday 2026-03-30 13:00:00 UTC = 09:00:00 ET (premarket).
        state
            .set_session_clock_ts_for_test(
                Utc.with_ymd_and_hms(2026, 3, 30, 13, 0, 0)
                    .unwrap()
                    .timestamp(),
            )
            .await;

        let in_session = AutonomousSessionSchedule::NyseRegularSession
            .is_in_session(&state, Utc.with_ymd_and_hms(2026, 3, 30, 13, 0, 0).unwrap())
            .await;
        assert!(!in_session, "NYSE premarket must remain out-of-session");
    }

    #[tokio::test]
    async fn sw13_nyse_after_hours_is_out_of_session() {
        let _guard = RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
        let state = Arc::new(AppState::new_for_test_with_broker_kind(
            super::super::types::BrokerKind::Alpaca,
        ));
        // Monday 2026-03-30 21:00:00 UTC = 17:00:00 ET (after-hours).
        state
            .set_session_clock_ts_for_test(
                Utc.with_ymd_and_hms(2026, 3, 30, 21, 0, 0)
                    .unwrap()
                    .timestamp(),
            )
            .await;

        let in_session = AutonomousSessionSchedule::NyseRegularSession
            .is_in_session(&state, Utc.with_ymd_and_hms(2026, 3, 30, 21, 0, 0).unwrap())
            .await;
        assert!(!in_session, "NYSE after-hours must remain out-of-session");
    }

    #[tokio::test]
    async fn sw14_nyse_weekend_is_out_of_session() {
        let _guard = RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
        let state = Arc::new(AppState::new_for_test_with_broker_kind(
            super::super::types::BrokerKind::Alpaca,
        ));
        // Saturday 2026-03-28 15:00:00 UTC.
        state
            .set_session_clock_ts_for_test(
                Utc.with_ymd_and_hms(2026, 3, 28, 15, 0, 0)
                    .unwrap()
                    .timestamp(),
            )
            .await;

        let in_session = AutonomousSessionSchedule::NyseRegularSession
            .is_in_session(&state, Utc.with_ymd_and_hms(2026, 3, 28, 15, 0, 0).unwrap())
            .await;
        assert!(!in_session, "NYSE weekend must remain out-of-session");
    }

    #[tokio::test]
    async fn sw15_nyse_holiday_is_out_of_session() {
        let _guard = RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
        let state = Arc::new(AppState::new_for_test_with_broker_kind(
            super::super::types::BrokerKind::Alpaca,
        ));
        // Friday 2026-07-03 is the observed Independence Day market holiday.
        // The current seam can honestly prove holiday closure here.
        state
            .set_session_clock_ts_for_test(
                Utc.with_ymd_and_hms(2026, 7, 3, 14, 0, 0)
                    .unwrap()
                    .timestamp(),
            )
            .await;

        let in_session = AutonomousSessionSchedule::NyseRegularSession
            .is_in_session(&state, Utc.with_ymd_and_hms(2026, 7, 3, 14, 0, 0).unwrap())
            .await;
        assert!(!in_session, "NYSE holiday must remain out-of-session");
        assert_eq!(
            mqk_integrity::CalendarSpec::NyseWeekdays.classify_exchange_calendar(
                Utc.with_ymd_and_hms(2026, 7, 3, 14, 0, 0)
                    .unwrap()
                    .timestamp(),
            ),
            "holiday",
            "current NYSE seam can honestly prove holiday closure"
        );
    }

    // ── ASSET-CORE-05E: v2_equity_active hook on is_in_session ──────────────

    fn real_registry_path() -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::path::PathBuf::from(manifest_dir)
            .join("../../../config/instruments/equities.json")
            .canonicalize()
            .expect("registry path must resolve from CARGO_MANIFEST_DIR")
            .to_string_lossy()
            .to_string()
    }

    fn state_with_registry(path: String) -> Arc<AppState> {
        let mut st =
            AppState::new_for_test_with_broker_kind(super::super::types::BrokerKind::Alpaca);
        st.instrument_registry_path = path;
        Arc::new(st)
    }

    #[tokio::test]
    async fn sw16_v2_equity_active_real_registry_drives_in_session_true_at_regular_open() {
        let _guard = RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_active");
        let state = state_with_registry(real_registry_path());
        let ts = Utc.with_ymd_and_hms(2026, 6, 25, 15, 0, 0).unwrap();
        state.set_session_clock_ts_for_test(ts.timestamp()).await;

        let in_session = AutonomousSessionSchedule::NyseRegularSession
            .is_in_session(&state, ts)
            .await;
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

        assert!(
            in_session,
            "v2_equity_active with a clean real registry must agree with legacy regular-open truth"
        );
    }

    #[tokio::test]
    async fn sw17_v2_equity_active_real_registry_drives_in_session_false_at_closed_weekend() {
        let _guard = RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_active");
        let state = state_with_registry(real_registry_path());
        // Saturday 2026-06-27 15:00:00 UTC — same fixed closed-weekend
        // timestamp runtime_session_source.rs's own tests use.
        let ts = Utc.with_ymd_and_hms(2026, 6, 27, 15, 0, 0).unwrap();
        state.set_session_clock_ts_for_test(ts.timestamp()).await;

        let in_session = AutonomousSessionSchedule::NyseRegularSession
            .is_in_session(&state, ts)
            .await;
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

        assert!(
            !in_session,
            "v2_equity_active with a clean real registry must agree with legacy closed-weekend truth"
        );
    }

    #[tokio::test]
    async fn sw18_v2_equity_active_missing_registry_fails_closed_even_at_legacy_regular_open() {
        let _guard = RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_active");
        let missing_path = std::env::temp_dir()
            .join(format!(
                "mqk_session_controller_missing_registry_{}.json",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string();
        let state = state_with_registry(missing_path);
        // Regular NYSE open — legacy alone would say "in session". The active
        // hook must still refuse and fail closed because the registry cannot
        // be loaded, proving this is a real override, not a coincidental
        // agreement with legacy.
        let ts = Utc.with_ymd_and_hms(2026, 6, 25, 15, 0, 0).unwrap();
        state.set_session_clock_ts_for_test(ts.timestamp()).await;

        let in_session = AutonomousSessionSchedule::NyseRegularSession
            .is_in_session(&state, ts)
            .await;
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

        assert!(
            !in_session,
            "v2_equity_active must fail closed (false) when the registry cannot be loaded, \
             even though the legacy NYSE calendar alone would report regular-open"
        );
    }

    #[tokio::test]
    async fn sw19_v2_equity_shadow_never_changes_is_in_session_decision() {
        let _guard = RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_shadow");
        // Point at a missing registry: if shadow mode touched this decision
        // at all, a registry load failure would have some observable effect.
        // It must not — is_in_session must match the plain legacy boolean.
        let missing_path = std::env::temp_dir()
            .join(format!(
                "mqk_session_controller_shadow_missing_registry_{}.json",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string();
        let state = state_with_registry(missing_path);
        let ts = Utc.with_ymd_and_hms(2026, 6, 25, 15, 0, 0).unwrap();
        state.set_session_clock_ts_for_test(ts.timestamp()).await;

        let in_session = AutonomousSessionSchedule::NyseRegularSession
            .is_in_session(&state, ts)
            .await;
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

        assert!(
            in_session,
            "v2_equity_shadow must never drive is_in_session; legacy regular-open truth must \
             pass through unchanged regardless of v2 registry health"
        );
    }

    #[tokio::test]
    async fn sw20_default_unset_mode_is_in_session_unaffected_by_missing_registry() {
        let _guard = RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
        let missing_path = std::env::temp_dir()
            .join(format!(
                "mqk_session_controller_default_missing_registry_{}.json",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string();
        let state = state_with_registry(missing_path);
        let ts = Utc.with_ymd_and_hms(2026, 6, 25, 15, 0, 0).unwrap();
        state.set_session_clock_ts_for_test(ts.timestamp()).await;

        let in_session = AutonomousSessionSchedule::NyseRegularSession
            .is_in_session(&state, ts)
            .await;

        assert!(
            in_session,
            "default (unset) mode must never depend on the v2 registry at all; legacy \
             regular-open truth must pass through unchanged"
        );
    }
}
