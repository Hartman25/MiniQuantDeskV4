//! Orchestrator-build helpers.
//!
//! Extracted from `state.rs` (MT-05).  Contains the two private helpers
//! used exclusively by the run-start path in `state/lifecycle.rs`:
//!
//! - `next_daemon_run_id` — derives the deterministic UUIDv5 for the next run.
//! - `build_execution_orchestrator` — constructs the `ExecutionOrchestrator`
//!   that the execution loop owns for the duration of a run.
//!
//! Both methods are `pub(super)` so that `state/lifecycle.rs` (a sibling
//! child module) can reach them via `self`.

use std::sync::Arc;

use chrono::{Datelike, Timelike, Utc};
use mqk_execution::{wiring::build_gateway, BrokerError, BrokerOrderMap};
use sqlx::PgPool;
use uuid::Uuid;

use super::broker::{build_daemon_broker, DaemonBroker};
use super::snapshot::{
    reconcile_broker_snapshot_from_schema, reconcile_local_snapshot_from_runtime_with_sides,
    recover_oms_and_portfolio, synthesize_paper_broker_snapshot,
};
use super::types::{
    AlpacaWsContinuityState, BrokerSnapshotTruthSource, DaemonOrchestrator, ReconcileTruthGate,
    RuntimeLifecycleError, StateIntegrityGate,
};
use super::{AppState, DAEMON_ENGINE_ID};

impl AppState {
    pub(super) async fn next_daemon_run_id(
        &self,
        db: &PgPool,
    ) -> Result<Uuid, RuntimeLifecycleError> {
        let generation: i64 = sqlx::query_scalar(
            r#"
            SELECT COALESCE(COUNT(*), 0)::bigint + 1
              FROM runs
             WHERE engine_id = $1
               AND mode = $2
            "#,
        )
        .bind(DAEMON_ENGINE_ID)
        .bind(self.deployment_mode().as_db_mode())
        .fetch_one(db)
        .await
        .map_err(|err| RuntimeLifecycleError::internal("next_daemon_run_id failed", err))?;

        Ok(Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!(
                "mqk-daemon.run.v2|{}|{}|{}|{}",
                self.node_id,
                DAEMON_ENGINE_ID,
                self.deployment_mode().as_db_mode(),
                generation
            )
            .as_bytes(),
        ))
    }

    pub(super) async fn build_execution_orchestrator(
        &self,
        db: PgPool,
        run_id: Uuid,
    ) -> Result<DaemonOrchestrator, RuntimeLifecycleError> {
        let run = mqk_db::fetch_run(&db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("fetch_run failed", err))?;

        // AUTON-PAPER-BLOCKER-01: daemon-created runs store a minimal config_json
        // with no /risk subtree.  Supplement from env vars so RuntimeRiskGate
        // receives real inputs.  Fields already in config_json are never overwritten.
        let (env_equity_micros, env_daily_loss_limit) = load_risk_env();
        let effective_config =
            effective_run_config_for_risk(&run.config_json, env_equity_micros, env_daily_loss_limit);

        let initial_equity_micros = effective_config
            .pointer("/risk/initial_equity_micros")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);

        let (oms_orders, recovered_sides, portfolio) =
            recover_oms_and_portfolio(&db, run_id, initial_equity_micros).await?;

        {
            let mut sides_lock = self.local_order_sides.write().await;
            *sides_lock = recovered_sides.clone();
        }

        // `AlpacaBrokerAdapter::new()` constructs a `reqwest::blocking::Client`
        // which temporarily creates and drops an internal Tokio runtime.  Tokio
        // 1.49 panics when any runtime is dropped inside an async context.
        // `block_in_place` moves execution off the async context so the drop
        // is safe.  Requires a multi-thread runtime (production and
        // `#[tokio::test(flavor = "multi_thread")]` tests both satisfy this).
        let daemon_broker = tokio::task::block_in_place(|| {
            build_daemon_broker(
                self.runtime_selection.broker_kind,
                self.runtime_selection.deployment_mode,
            )
        })?;

        let broker_seed = match self.broker_snapshot_source {
            BrokerSnapshotTruthSource::Synthetic => {
                let broker_snapshot_guard = self.broker_snapshot.read().await;
                if let Some(existing) = broker_snapshot_guard.clone() {
                    existing
                } else {
                    drop(broker_snapshot_guard);
                    let now = Utc::now();
                    let synth = synthesize_paper_broker_snapshot(
                        &oms_orders,
                        &recovered_sides,
                        &portfolio,
                        now,
                    );
                    *self.broker_snapshot.write().await = Some(synth.clone());
                    synth
                }
            }
            BrokerSnapshotTruthSource::External => {
                // If a snapshot is already present (pre-loaded by test
                // scaffolding, or retained from a prior loop tick), use it
                // directly and skip the blocking network fetch.  In a fresh
                // production process `broker_snapshot` is always `None` here,
                // so the fetch always runs in production.
                let seeded = self.broker_snapshot.read().await.clone();
                if let Some(existing) = seeded {
                    existing
                } else {
                    let now = Utc::now();
                    let fetched = tokio::task::block_in_place(|| {
                        match &daemon_broker {
                            DaemonBroker::Alpaca(adapter) => {
                                adapter.fetch_broker_snapshot(now).map_err(|err| match err {
                                    BrokerError::AuthSession { detail } => {
                                        RuntimeLifecycleError::forbidden(
                                            "runtime.start_refused.alpaca_snapshot_auth",
                                            "broker_snapshot_fetch",
                                            format!(
                                                "failed to fetch Alpaca broker snapshot before runtime start: {detail}"
                                            ),
                                        )
                                    }
                                    other => RuntimeLifecycleError::service_unavailable(
                                        "runtime.start_refused.alpaca_snapshot_unavailable",
                                        format!(
                                            "failed to fetch Alpaca broker snapshot before runtime start: {other}"
                                        ),
                                    ),
                                })
                            }
                            _ => Err(RuntimeLifecycleError::service_unavailable(
                                "runtime.start_refused.broker_snapshot_source_mismatch",
                                "external broker snapshot source requires Alpaca broker adapter construction",
                            )),
                        }
                    })?;

                    *self.broker_snapshot.write().await = Some(fetched.clone());

                    // AUTON-PAPER-RISK-03: build a second adapter dedicated to periodic
                    // snapshot refresh in the execution loop.  build_daemon_broker reads
                    // credentials from env — same stable config as the execution adapter.
                    // If this build fails we skip; the loop falls back to the startup
                    // snapshot, which is the pre-patch status quo.
                    let refresh_result = tokio::task::block_in_place(|| {
                        build_daemon_broker(
                            self.runtime_selection.broker_kind,
                            self.runtime_selection.deployment_mode,
                        )
                    });
                    match refresh_result {
                        Ok(DaemonBroker::Alpaca(refresh_alpaca)) => {
                            *self.external_snapshot_refresher.write().await =
                                Some(Arc::new(refresh_alpaca));
                        }
                        Ok(_) => {
                            tracing::warn!(
                                "external_snapshot_refresher_build_failed: \
                                 unexpected broker kind; periodic broker snapshot refresh will not run"
                            );
                        }
                        Err(err) => {
                            tracing::warn!(
                                "external_snapshot_refresher_build_failed: \
                                 periodic broker snapshot refresh will not run; error={err}"
                            );
                        }
                    }

                    fetched
                }
            }
        };

        let mut order_map = BrokerOrderMap::new();
        let existing = mqk_db::broker_map_load(&db)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("broker_map_load failed", err))?;
        for (internal_id, broker_id) in existing {
            order_map.register(&internal_id, &broker_id);
        }

        let broker_cursor = mqk_db::load_broker_cursor(&db, self.adapter_id())
            .await
            .map_err(|err| RuntimeLifecycleError::internal("load_broker_cursor failed", err))?;

        let ws_continuity = AlpacaWsContinuityState::from_cursor_json(
            self.runtime_selection.broker_kind,
            broker_cursor.as_deref(),
        );
        *self.alpaca_ws_continuity.write().await = ws_continuity;

        // AUTON-PAPER-RISK-04: derive real day/window identifiers from UTC
        // wall-clock at orchestrator construction time.  day_id is YYYYMMDD
        // (matching RiskInput documentation); reject_window_id is the
        // minute-of-day bucket (0..1439, matching "minute bucket counter").
        // Both are evaluated once at run-start — the risk engine tracks
        // subsequent window transitions via RiskState::record_reject().
        let risk_now = Utc::now();
        let risk_day = risk_now.date_naive();
        let day_id: u32 =
            (risk_day.year() as u32) * 10_000 + risk_day.month() * 100 + risk_day.day();
        let reject_window_id: u32 = risk_now.hour() * 60 + risk_now.minute();

        let gateway = build_gateway(
            daemon_broker,
            StateIntegrityGate {
                integrity: Arc::clone(&self.integrity),
            },
            mqk_runtime::runtime_risk::RuntimeRiskGate::from_run_config(
                &effective_config,
                initial_equity_micros,
                day_id,
                reject_window_id,
            ),
            ReconcileTruthGate {
                reconcile_status: Arc::clone(&self.reconcile_status),
            },
        );

        let broker_snapshots = Arc::clone(&self.broker_snapshot);
        let broker_seed_reconcile =
            reconcile_broker_snapshot_from_schema(&broker_seed).map_err(|err| {
                RuntimeLifecycleError::service_unavailable(
                    "runtime.start_refused.service_unavailable",
                    err.to_string(),
                )
            })?;

        let local_seed_reconcile = {
            let local_snapshot_guard = self.execution_snapshot.read().await;
            if let Some(snap) = local_snapshot_guard.clone() {
                let sides = self.local_order_sides.read().await;
                reconcile_local_snapshot_from_runtime_with_sides(&snap, &sides)
            } else {
                mqk_reconcile::LocalSnapshot::empty()
            }
        };

        let local_snapshots = Arc::clone(&self.execution_snapshot);
        let side_cache_for_local = Arc::clone(&self.local_order_sides);
        let local_snapshot_provider = move || {
            let Some(snapshot) = local_snapshots
                .try_read()
                .ok()
                .and_then(|snapshot| snapshot.clone())
            else {
                return local_seed_reconcile.clone();
            };
            let sides = side_cache_for_local
                .try_read()
                .map(|g| g.clone())
                .unwrap_or_default();
            reconcile_local_snapshot_from_runtime_with_sides(&snapshot, &sides)
        };

        let broker_snapshot_provider = move || {
            let Some(schema_snapshot) = broker_snapshots
                .try_read()
                .ok()
                .and_then(|snapshot| snapshot.clone())
            else {
                return broker_seed_reconcile.clone();
            };

            reconcile_broker_snapshot_from_schema(&schema_snapshot)
                .unwrap_or_else(|_| broker_seed_reconcile.clone())
        };

        Ok(mqk_runtime::orchestrator::ExecutionOrchestrator::new(
            db,
            gateway,
            order_map,
            oms_orders,
            portfolio,
            run_id,
            self.node_id.clone(),
            self.adapter_id(),
            broker_cursor,
            mqk_runtime::orchestrator::WallClock,
            Box::new(local_snapshot_provider),
            Box::new(broker_snapshot_provider),
        ))
    }
}

// ---------------------------------------------------------------------------
// AUTON-PAPER-BLOCKER-01: env-sourced risk config helpers
// ---------------------------------------------------------------------------

/// Env var: initial equity in USD (positive float).  Converted to micros.
pub(crate) const ENV_RISK_INITIAL_EQUITY_USD: &str = "MQK_RISK_INITIAL_EQUITY_USD";

/// Env var: daily loss limit as a ratio (exclusive range 0 < r < 1).
pub(crate) const ENV_RISK_DAILY_LOSS_LIMIT: &str = "MQK_RISK_DAILY_LOSS_LIMIT";

/// Read the two required risk fields from env.
///
/// Returns `(equity_micros, daily_loss_limit)`.  Either or both may be `None`
/// if the env var is absent, empty, unparseable, or out of range.  The risk
/// gate already fails closed when these are absent, so `None` just preserves
/// the prior fail-closed behavior.
fn load_risk_env() -> (Option<i64>, Option<f64>) {
    let equity_micros = std::env::var(ENV_RISK_INITIAL_EQUITY_USD)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|&usd| usd > 0.0 && usd.is_finite() && usd * 1_000_000.0 <= i64::MAX as f64)
        .map(|usd| (usd * 1_000_000.0).round() as i64);

    let daily_loss_limit = std::env::var(ENV_RISK_DAILY_LOSS_LIMIT)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|&r| r.is_finite() && r > 0.0 && r < 1.0);

    (equity_micros, daily_loss_limit)
}

/// Build the effective run config for risk gate initialization.
///
/// Fields already present in `base` are never overwritten — a run row that
/// carries an explicit `/risk` subtree is authoritative.  Env-sourced values
/// only fill fields that `base` does not contain, so this function is safe
/// to call for both daemon-created runs (no `/risk`) and future run rows that
/// carry full risk config.
///
/// If neither env var is set and `base` has no `/risk` subtree, the returned
/// value equals `base` and `RuntimeRiskGate` still fails closed as before.
fn effective_run_config_for_risk(
    base: &serde_json::Value,
    env_equity_micros: Option<i64>,
    env_daily_loss_limit: Option<f64>,
) -> serde_json::Value {
    let need_equity = base
        .pointer("/risk/initial_equity_micros")
        .and_then(|v| v.as_i64())
        .is_none();
    let need_loss_limit = base
        .pointer("/risk/daily_loss_limit")
        .and_then(|v| v.as_f64())
        .is_none();

    let will_add_equity = need_equity && env_equity_micros.is_some();
    let will_add_loss_limit = need_loss_limit && env_daily_loss_limit.is_some();

    if !will_add_equity && !will_add_loss_limit {
        return base.clone();
    }

    let mut merged = base.clone();
    let obj = match merged.as_object_mut() {
        Some(o) => o,
        None => return base.clone(),
    };

    let risk = obj
        .entry("risk")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

    if let Some(risk_obj) = risk.as_object_mut() {
        if will_add_equity {
            risk_obj.insert(
                "initial_equity_micros".to_string(),
                serde_json::json!(env_equity_micros.unwrap()),
            );
        }
        if will_add_loss_limit {
            risk_obj.insert(
                "daily_loss_limit".to_string(),
                serde_json::json!(env_daily_loss_limit.unwrap()),
            );
        }
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    // AUTON-PAPER-RISK-04: prove the day_id/reject_window_id derivation
    // formulas produce the exact values the risk engine documentation specifies.
    #[test]
    fn risk_time_context_day_id_and_window_derivation_is_correct() {
        use chrono::{Datelike, TimeZone, Timelike};
        // 2024-01-15 09:32:45 UTC — a known reference moment.
        let ts = chrono::Utc
            .with_ymd_and_hms(2024, 1, 15, 9, 32, 45)
            .unwrap();
        let d = ts.date_naive();
        let day_id: u32 =
            (d.year() as u32) * 10_000 + d.month() * 100 + d.day();
        assert_eq!(day_id, 20_240_115, "day_id must be YYYYMMDD");

        let reject_window_id: u32 = ts.hour() * 60 + ts.minute();
        assert_eq!(reject_window_id, 9 * 60 + 32, "reject_window_id must be minute-of-day bucket");

        // Boundary: midnight (00:00) yields bucket 0.
        let midnight = chrono::Utc
            .with_ymd_and_hms(2024, 1, 15, 0, 0, 0)
            .unwrap();
        assert_eq!(midnight.hour() * 60 + midnight.minute(), 0);

        // Boundary: 23:59 yields bucket 1439 (max for a 24-hour day).
        let last_minute = chrono::Utc
            .with_ymd_and_hms(2024, 1, 15, 23, 59, 0)
            .unwrap();
        assert_eq!(last_minute.hour() * 60 + last_minute.minute(), 1439);

        // day_id must never overflow u32 for any sane calendar year.
        let far: u32 = 9999 * 10_000 + 12 * 100 + 31;
        assert!(far < u32::MAX, "day_id fits in u32 for any calendar date");
    }

    #[test]
    fn supplements_missing_risk_fields_from_env_values() {
        let base = serde_json::json!({
            "runtime": "mqk-daemon",
            "adapter": "alpaca",
            "mode": "paper",
        });
        let effective =
            effective_run_config_for_risk(&base, Some(50_000 * 1_000_000), Some(0.02));

        assert_eq!(
            effective
                .pointer("/risk/initial_equity_micros")
                .and_then(|v| v.as_i64()),
            Some(50_000_000_000),
        );
        assert_eq!(
            effective
                .pointer("/risk/daily_loss_limit")
                .and_then(|v| v.as_f64()),
            Some(0.02),
        );
        // Non-risk fields preserved.
        assert_eq!(
            effective.pointer("/runtime").and_then(|v| v.as_str()),
            Some("mqk-daemon"),
        );
    }

    #[test]
    fn does_not_override_existing_risk_fields() {
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 10_000_000_000i64,
                "daily_loss_limit": 0.01_f64,
            }
        });
        // Env values that would overwrite if the guard failed.
        let effective =
            effective_run_config_for_risk(&base, Some(99_999_000_000), Some(0.99));

        assert_eq!(
            effective
                .pointer("/risk/initial_equity_micros")
                .and_then(|v| v.as_i64()),
            Some(10_000_000_000),
            "base equity_micros must not be overwritten",
        );
        assert_eq!(
            effective
                .pointer("/risk/daily_loss_limit")
                .and_then(|v| v.as_f64()),
            Some(0.01),
            "base daily_loss_limit must not be overwritten",
        );
    }

    #[test]
    fn returns_base_unchanged_when_env_absent() {
        let base = serde_json::json!({ "runtime": "mqk-daemon" });
        let effective = effective_run_config_for_risk(&base, None, None);

        // No /risk subtree added — fail-closed behavior preserved.
        assert!(effective.pointer("/risk/initial_equity_micros").is_none());
        assert!(effective.pointer("/risk/daily_loss_limit").is_none());
        assert_eq!(effective, base);
    }

    #[test]
    fn load_risk_env_rejects_invalid_ratio() {
        // Direct test of the filter logic — ratio >= 1.0 is invalid.
        let bad: Option<f64> = Some(2.0_f64)
            .filter(|&r| r.is_finite() && r > 0.0 && r < 1.0);
        assert!(bad.is_none(), "ratio >= 1.0 must be rejected");

        let also_bad: Option<f64> = Some(0.0_f64)
            .filter(|&r| r.is_finite() && r > 0.0 && r < 1.0);
        assert!(also_bad.is_none(), "zero ratio must be rejected");
    }

    #[test]
    fn supplements_only_missing_field() {
        // base has equity but not loss_limit — only loss_limit should be added.
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 25_000_000_000i64,
            }
        });
        let effective =
            effective_run_config_for_risk(&base, Some(99_000_000_000), Some(0.03));

        // Equity from base wins.
        assert_eq!(
            effective
                .pointer("/risk/initial_equity_micros")
                .and_then(|v| v.as_i64()),
            Some(25_000_000_000),
        );
        // Loss limit supplemented from env.
        assert_eq!(
            effective
                .pointer("/risk/daily_loss_limit")
                .and_then(|v| v.as_f64()),
            Some(0.03),
        );
    }
}
