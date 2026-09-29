//! D1: the production scheduling seam for options-lifecycle ingestion.
//!
//! No account-activity polling loop existed in the daemon (the WS transport's
//! gap recovery is event-driven and fill-only), so this is the narrowest
//! addition: one supervised interval task, spawned from `main`, that runs
//! [`run_option_lifecycle_cycle`] against the daemon's configured
//! `OptionLifecycleActivityFetcher`.
//!
//! Default OFF: it starts only when `MQK_OPTION_LIFECYCLE_POLL_INTERVAL_SECS`
//! is a positive integer AND a database and a fetcher (Alpaca credentials)
//! are configured. It is read-only against the broker (account-activity GETs),
//! never submits an order, and activates no execution capability. A failing
//! cycle is logged and retried next tick; a cursor only advances inside the
//! transaction that durably records the rows it names, so a failure can never
//! skip evidence.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use super::option_lifecycle_cycle::{
    run_option_lifecycle_cycle, LifecycleCycleReport, OPTION_LIFECYCLE_ENGINE_ID,
};
use super::AppState;

/// Env var: poll cadence in seconds. Unset, blank, `0` or malformed disables
/// the task.
pub const OPTION_LIFECYCLE_POLL_INTERVAL_SECS_ENV: &str = "MQK_OPTION_LIFECYCLE_POLL_INTERVAL_SECS";

/// Pure: classify an already-read env value. `None` means disabled.
pub fn poll_interval_from_env_value(raw: Option<&str>) -> Option<Duration> {
    raw.map(str::trim)
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map(Duration::from_secs)
}

/// Spawn the lifecycle poll task from the environment. `None` when disabled
/// or when the daemon has no DB / no lifecycle fetcher.
pub fn spawn_option_lifecycle_poll_task(
    state: Arc<AppState>,
) -> Option<tokio::task::JoinHandle<()>> {
    let interval = poll_interval_from_env_value(
        std::env::var(OPTION_LIFECYCLE_POLL_INTERVAL_SECS_ENV)
            .ok()
            .as_deref(),
    )?;
    spawn_option_lifecycle_poll_task_with_interval(state, interval)
}

/// Same as [`spawn_option_lifecycle_poll_task`] with an explicit cadence (the
/// env-reading wrapper above is the only production caller).
pub fn spawn_option_lifecycle_poll_task_with_interval(
    state: Arc<AppState>,
    interval: Duration,
) -> Option<tokio::task::JoinHandle<()>> {
    if state.db.is_none() || state.option_lifecycle_activity_fetcher.is_none() {
        warn!(
            "option_lifecycle_poll: enabled by env but no database or lifecycle fetcher is \
             configured; task not started"
        );
        return None;
    }
    info!(
        interval_secs = interval.as_secs(),
        "option_lifecycle_poll: enabled"
    );
    Some(tokio::spawn(run_poll_loop(state, interval)))
}

async fn run_poll_loop(state: Arc<AppState>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_poll_tick(&state).await {
            Ok(report) => info!(?report, "option_lifecycle_poll: cycle complete"),
            Err(err) => warn!(error = %err, "option_lifecycle_poll: cycle failed; will retry"),
        }
    }
}

/// One poll tick: exactly [`run_option_lifecycle_cycle`] against the
/// configured pool and fetcher.
pub async fn run_poll_tick(state: &AppState) -> anyhow::Result<LifecycleCycleReport> {
    let Some(pool) = state.db.as_ref() else {
        anyhow::bail!("option_lifecycle_poll: no database configured");
    };
    let Some(fetcher) = state.option_lifecycle_activity_fetcher.as_ref() else {
        anyhow::bail!("option_lifecycle_poll: no lifecycle fetcher configured");
    };
    run_option_lifecycle_cycle(
        pool,
        fetcher.as_ref(),
        OPTION_LIFECYCLE_ENGINE_ID,
        chrono::Utc::now(), // allow: ops-metadata ingestion timestamp
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_parse_table() {
        assert_eq!(poll_interval_from_env_value(None), None);
        assert_eq!(poll_interval_from_env_value(Some("")), None);
        assert_eq!(poll_interval_from_env_value(Some("0")), None);
        assert_eq!(poll_interval_from_env_value(Some("-5")), None);
        assert_eq!(poll_interval_from_env_value(Some("abc")), None);
        assert_eq!(
            poll_interval_from_env_value(Some(" 30 ")),
            Some(Duration::from_secs(30))
        );
    }

    #[tokio::test]
    async fn no_fetcher_means_no_task() {
        let state = Arc::new(AppState::new_for_test_with_broker_kind(
            crate::state::BrokerKind::Alpaca,
        ));
        assert!(
            spawn_option_lifecycle_poll_task_with_interval(state, Duration::from_secs(1)).is_none()
        );
    }
}
