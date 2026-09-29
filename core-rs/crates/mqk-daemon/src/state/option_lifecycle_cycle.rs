//! D1: one options-lifecycle cycle -- ingest all four provider activity
//! types, then (re-)correlate every lifecycle event that has not yet been
//! applied and persist the resulting state.
//!
//! This is the unit the daemon's `option_lifecycle_poll` task runs on a
//! timer; tests drive it with a mock fetcher (no real broker call). Each
//! activity type ingests in its own transaction with its own account-scoped
//! cursor, so a failure on one type never advances another's cursor past
//! evidence it did not durably record.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_db::option_lifecycle_activity::{
    fetch_option_lifecycle_activity, list_option_lifecycle_activities, OptionLifecycleActivityType,
};
use mqk_db::{
    list_option_lifecycle_event_states, record_lifecycle_evaluation, LifecycleEvaluation,
    LifecycleEventState,
};

use super::option_lifecycle_correlation::{correlate_lifecycle_event, CorrelationOutcome};
use super::option_lifecycle_ingestion::{
    ingest_option_lifecycle_activities_once, OPTION_LIFECYCLE_EXECUTION_DOMAIN,
};
use super::OptionLifecycleActivityFetcher;

/// Engine identity under which the daemon's lifecycle cursors are scoped.
pub const OPTION_LIFECYCLE_ENGINE_ID: &str = "mqk-daemon";

/// The four provider activity types, ingested in this fixed order.
pub const OPTION_LIFECYCLE_ACTIVITY_TYPES: [&str; 4] = ["OPEXC", "OPASN", "OPEXP", "OPTRD"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleCycleReport {
    /// activity type -> (newly inserted, already existed)
    pub ingested: BTreeMap<String, (usize, usize)>,
    pub evaluated: usize,
    pub ready: usize,
    pub pending_evidence: usize,
    pub pending_ambiguous: usize,
}

/// Ingest all four activity types, then evaluate pending lifecycle events.
pub async fn run_option_lifecycle_cycle(
    pool: &PgPool,
    fetcher: &dyn OptionLifecycleActivityFetcher,
    engine_id: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<LifecycleCycleReport> {
    let authority = fetcher
        .broker_account_authority()
        .map_err(|e| anyhow::anyhow!("run_option_lifecycle_cycle: account authority: {e}"))?;
    let mode = authority.deployment_mode().to_string();
    let mut report = LifecycleCycleReport::default();

    for activity_type in OPTION_LIFECYCLE_ACTIVITY_TYPES {
        let outcome = ingest_option_lifecycle_activities_once(
            pool,
            fetcher,
            engine_id,
            &mode,
            activity_type,
            now_utc,
        )
        .await?;
        report.ingested.insert(
            activity_type.to_string(),
            (outcome.newly_inserted, outcome.already_existed),
        );
    }

    evaluate_pending_lifecycle_events(pool, &authority.key(), now_utc, &mut report).await?;
    Ok(report)
}

fn evaluation_from_outcome(outcome: CorrelationOutcome) -> LifecycleEvaluation {
    match outcome {
        CorrelationOutcome::Ready {
            underlying_symbol,
            basis,
            optrd_activity_id,
        } => LifecycleEvaluation {
            state: LifecycleEventState::ReadyToApply,
            reason: format!("correlated:{}", basis.as_str()),
            underlying_symbol: Some(underlying_symbol),
            correlated_optrd_activity_id: optrd_activity_id,
            correlation_basis: Some(basis),
        },
        CorrelationOutcome::PendingEvidence {
            underlying_symbol,
            reason,
        } => LifecycleEvaluation {
            state: LifecycleEventState::PendingEvidence,
            reason,
            underlying_symbol,
            correlated_optrd_activity_id: None,
            correlation_basis: None,
        },
        CorrelationOutcome::PendingAmbiguous {
            underlying_symbol,
            reason,
            ..
        } => LifecycleEvaluation {
            state: LifecycleEventState::PendingAmbiguous,
            reason,
            underlying_symbol: Some(underlying_symbol),
            correlated_optrd_activity_id: None,
            correlation_basis: None,
        },
    }
}

/// Correlate every not-yet-applied lifecycle event of `account_key` and
/// persist the outcome (compare-and-set: applied/reconciled rows are never
/// touched).
pub async fn evaluate_pending_lifecycle_events(
    pool: &PgPool,
    account_key: &str,
    now_utc: DateTime<Utc>,
    report: &mut LifecycleCycleReport,
) -> anyhow::Result<()> {
    let domain = OPTION_LIFECYCLE_EXECUTION_DOMAIN.as_str();
    let states = list_option_lifecycle_event_states(pool, account_key, domain).await?;
    let optrds = list_option_lifecycle_activities(
        pool,
        account_key,
        OptionLifecycleActivityType::PairedTrade,
    )
    .await?;

    for state in &states {
        if !matches!(
            state.state,
            LifecycleEventState::PendingEvidence
                | LifecycleEventState::PendingAmbiguous
                | LifecycleEventState::ReadyToApply
        ) {
            continue;
        }

        // OPTRD ids this event must not use: the same-id partner of a
        // DIFFERENT lifecycle event, and any trade another event has already
        // claimed (correlated, applied or reconciled).
        let mut excluded: BTreeSet<String> = BTreeSet::new();
        for other in &states {
            let is_self = other.lifecycle_activity_id == state.lifecycle_activity_id
                && other.lifecycle_activity_type == state.lifecycle_activity_type;
            if is_self {
                continue;
            }
            excluded.insert(other.lifecycle_activity_id.clone());
            if let Some(claimed) = &other.correlated_optrd_activity_id {
                excluded.insert(claimed.clone());
            }
        }

        let Some(raw) = fetch_option_lifecycle_activity(
            pool,
            account_key,
            &state.lifecycle_activity_id,
            state.lifecycle_activity_type,
        )
        .await?
        else {
            anyhow::bail!(
                "evaluate_pending_lifecycle_events: state row {:?} has no raw lifecycle row",
                state.lifecycle_activity_id
            );
        };

        let evaluation =
            evaluation_from_outcome(correlate_lifecycle_event(&raw, &optrds, &excluded));
        let counted = evaluation.state;
        if record_lifecycle_evaluation(
            pool,
            account_key,
            &state.lifecycle_activity_id,
            state.lifecycle_activity_type,
            &evaluation,
            now_utc,
        )
        .await?
        {
            report.evaluated += 1;
            match counted {
                LifecycleEventState::ReadyToApply => report.ready += 1,
                LifecycleEventState::PendingEvidence => report.pending_evidence += 1,
                LifecycleEventState::PendingAmbiguous => report.pending_ambiguous += 1,
                _ => {}
            }
        }
    }
    Ok(())
}
