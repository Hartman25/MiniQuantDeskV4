//! DeadmanTruth — per-run deadman enforcement and status derivation.
//!
//! Extracted from `state.rs` (MT-06).  Contains the private `DeadmanTruth`
//! struct and the `AppState::deadman_truth_for_run` helper that enforces
//! the deadman heartbeat TTL and derives the deadman status string for the
//! `StatusSnapshot`.
//!
//! `deadman_truth_for_run` is `pub(super)` so that the parent `state.rs`
//! module can call it via `self`.

use chrono::Utc;
use uuid::Uuid;

use super::{AppState, RuntimeLifecycleError, DEADMAN_TTL_SECONDS};

// ---------------------------------------------------------------------------
// DeadmanTruth
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(super) struct DeadmanTruth {
    pub(super) status: String,
    pub(super) last_heartbeat_utc: Option<String>,
}

impl AppState {
    pub(super) async fn deadman_truth_for_run(
        &self,
        run_id: Uuid,
    ) -> Result<DeadmanTruth, RuntimeLifecycleError> {
        let db = self.db_pool()?;
        let now = Utc::now();
        let halted = mqk_db::enforce_deadman_or_halt(&db, run_id, DEADMAN_TTL_SECONDS, now)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("deadman enforce failed", err))?;
        let run = mqk_db::fetch_run(&db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("deadman fetch_run failed", err))?;

        if halted {
            mqk_db::persist_arm_state_canonical(
                &db,
                mqk_db::ArmState::Disarmed,
                Some(mqk_db::DisarmReason::DeadmanExpired),
            )
            .await
            .map_err(|err| {
                RuntimeLifecycleError::internal("deadman persist_arm_state failed", err)
            })?;
            {
                let mut integrity = self.integrity.write().await;
                integrity.disarmed = true;
                integrity.halted = true;
            }
        }

        let status = match run.status {
            mqk_db::RunStatus::Running => {
                let expired = mqk_db::deadman_expired(&db, run_id, DEADMAN_TTL_SECONDS, now)
                    .await
                    .map_err(|err| RuntimeLifecycleError::internal("deadman check failed", err))?;
                if expired {
                    "expired"
                } else {
                    "healthy"
                }
            }
            mqk_db::RunStatus::Halted => "expired",
            mqk_db::RunStatus::Armed | mqk_db::RunStatus::Created | mqk_db::RunStatus::Stopped => {
                "inactive"
            }
        }
        .to_string();

        Ok(DeadmanTruth {
            status,
            last_heartbeat_utc: run.last_heartbeat_utc.map(|ts| ts.to_rfc3339()),
        })
    }
}
