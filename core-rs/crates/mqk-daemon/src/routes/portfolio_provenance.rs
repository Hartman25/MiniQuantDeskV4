// core-rs/crates/mqk-daemon/src/routes/portfolio_provenance.rs
//
// DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-COHERENCE-AND-ACCEPTANCE-PROOF
// (Phase B1): one shared, pure provenance classifier for the relationship
// among a resolved run, its run-scoped durable snapshot, and its durable
// accounting row. Both `routes/durable_portfolio.rs` (durable-summary
// projection) and `routes/paper_lifecycle.rs` (paper-lifecycle P&L truth)
// must derive their accounting/P&L truth_state from this single function --
// two independently hand-rolled classifications is exactly how the two
// routes drifted before this repair (durable-summary's `accounting_fields`
// never compared the accounting row's `source_snapshot_id` against the
// currently-selected snapshot at all, so a stale accounting row from an
// older snapshot could be reported "active" beside a newer snapshot).
//
// Closed vocabulary -- exactly these seven states, no others:
//   active
//   fill_history_incomplete
//   accounting_epoch_unavailable
//   accounting_snapshot_mismatch
//   not_found
//   query_failed
//   unsupported_source

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortfolioProvenanceState {
    Active,
    FillHistoryIncomplete,
    AccountingEpochUnavailable,
    AccountingSnapshotMismatch,
    NotFound,
    QueryFailed,
    UnsupportedSource,
}

impl PortfolioProvenanceState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::FillHistoryIncomplete => "fill_history_incomplete",
            Self::AccountingEpochUnavailable => "accounting_epoch_unavailable",
            Self::AccountingSnapshotMismatch => "accounting_snapshot_mismatch",
            Self::NotFound => "not_found",
            Self::QueryFailed => "query_failed",
            Self::UnsupportedSource => "unsupported_source",
        }
    }
}

/// Classify the accounting/P&L provenance for one resolved run.
///
/// `is_paper_mode` -- the resolved run's `mode == "PAPER"`. A non-paper run
/// can never surface active durable portfolio/P&L truth (B4 closure repair,
/// Repair G) -- checked first, before any other input is even considered.
///
/// `snapshot_query_failed` / `accounting_query_failed` -- `true` when the
/// respective DB query itself failed (not merely returned zero rows). A
/// query failure must never be silently reported as `not_found` (B4 closure
/// repair, Defect 5) -- checked second.
///
/// `snapshot_id` -- the run-scoped, currently-selected durable snapshot's
/// id, or `None` if no such snapshot exists yet for this run. Determines
/// whether an accounting row's `source_snapshot_id` still points at live,
/// current provenance.
///
/// `accounting` -- the run's durable accounting-state row, or `None` if none
/// exists yet.
///
/// Required behavior (frozen contract, B4 Phase B):
/// - no accounting row at all -> `NotFound`, regardless of snapshot
///   presence.
/// - no snapshot exists for this run -> never `Active`, even if an
///   accounting row happens to exist (legacy `source_snapshot_id = None`
///   row, or any other pre-existing state) -- `AccountingEpochUnavailable`.
/// - accounting row's `source_snapshot_id` is `None` -> completeness cannot
///   be traced -> `AccountingEpochUnavailable`.
/// - accounting row's `source_snapshot_id` does not equal the selected
///   snapshot's id -> `AccountingSnapshotMismatch` (checked before the
///   epoch check below -- a stale-snapshot accounting row must never be
///   reported as merely "incomplete").
/// - `accounting_epoch == "incomplete"` -> `FillHistoryIncomplete`.
/// - otherwise (snapshot present, ids match, epoch complete) -> `Active`.
pub(crate) fn classify_portfolio_provenance(
    is_paper_mode: bool,
    snapshot_query_failed: bool,
    accounting_query_failed: bool,
    snapshot_id: Option<Uuid>,
    accounting: Option<&mqk_db::PaperPortfolioAccountingStateRecord>,
) -> PortfolioProvenanceState {
    if !is_paper_mode {
        return PortfolioProvenanceState::UnsupportedSource;
    }
    if snapshot_query_failed || accounting_query_failed {
        return PortfolioProvenanceState::QueryFailed;
    }
    let Some(accounting) = accounting else {
        return PortfolioProvenanceState::NotFound;
    };
    let Some(snapshot_id) = snapshot_id else {
        return PortfolioProvenanceState::AccountingEpochUnavailable;
    };
    match accounting.source_snapshot_id {
        None => PortfolioProvenanceState::AccountingEpochUnavailable,
        Some(id) if id != snapshot_id => PortfolioProvenanceState::AccountingSnapshotMismatch,
        Some(_) if accounting.accounting_epoch == "incomplete" => {
            PortfolioProvenanceState::FillHistoryIncomplete
        }
        Some(_) => PortfolioProvenanceState::Active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn accounting_row(
        epoch: &str,
        source_snapshot_id: Option<Uuid>,
    ) -> mqk_db::PaperPortfolioAccountingStateRecord {
        mqk_db::PaperPortfolioAccountingStateRecord {
            run_id: Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run"),
            cash_micros: 0,
            realized_pnl_micros: 0,
            fees_micros: 0,
            last_applied_inbox_id: 1,
            accounting_epoch: epoch.to_string(),
            accounting_epoch_reason: None,
            updated_at_utc: Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
            source_snapshot_id,
        }
    }

    fn snap_id(seed: &str) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!("test.portfolio-provenance.v1|snapshot|{seed}").as_bytes(),
        )
    }

    #[test]
    fn non_paper_run_is_unsupported_source_regardless_of_other_inputs() {
        let snap = snap_id("a");
        let row = accounting_row("complete", Some(snap));
        assert_eq!(
            classify_portfolio_provenance(false, false, false, Some(snap), Some(&row)),
            PortfolioProvenanceState::UnsupportedSource
        );
    }

    #[test]
    fn snapshot_query_failure_is_query_failed_not_not_found() {
        assert_eq!(
            classify_portfolio_provenance(true, true, false, None, None),
            PortfolioProvenanceState::QueryFailed
        );
    }

    #[test]
    fn accounting_query_failure_is_query_failed_not_not_found() {
        assert_eq!(
            classify_portfolio_provenance(true, false, true, None, None),
            PortfolioProvenanceState::QueryFailed
        );
    }

    #[test]
    fn no_accounting_row_is_not_found() {
        let snap = snap_id("a");
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(snap), None),
            PortfolioProvenanceState::NotFound
        );
    }

    #[test]
    fn no_snapshot_with_existing_accounting_row_cannot_be_active() {
        let snap = snap_id("a");
        // Accounting row claims complete + a source_snapshot_id, but no
        // snapshot resolves for this run at all -- must never be Active.
        let row = accounting_row("complete", Some(snap));
        assert_eq!(
            classify_portfolio_provenance(true, false, false, None, Some(&row)),
            PortfolioProvenanceState::AccountingEpochUnavailable
        );
    }

    #[test]
    fn missing_source_snapshot_id_is_accounting_epoch_unavailable() {
        let snap = snap_id("a");
        let row = accounting_row("complete", None);
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(snap), Some(&row)),
            PortfolioProvenanceState::AccountingEpochUnavailable
        );
    }

    #[test]
    fn mismatched_source_snapshot_is_accounting_snapshot_mismatch() {
        let selected = snap_id("selected");
        let stale = snap_id("stale");
        let row = accounting_row("complete", Some(stale));
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(selected), Some(&row)),
            PortfolioProvenanceState::AccountingSnapshotMismatch
        );
    }

    #[test]
    fn mismatch_takes_priority_over_incomplete_epoch() {
        let selected = snap_id("selected");
        let stale = snap_id("stale");
        let row = accounting_row("incomplete", Some(stale));
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(selected), Some(&row)),
            PortfolioProvenanceState::AccountingSnapshotMismatch
        );
    }

    #[test]
    fn incomplete_epoch_with_matching_snapshot_is_fill_history_incomplete() {
        let snap = snap_id("a");
        let row = accounting_row("incomplete", Some(snap));
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(snap), Some(&row)),
            PortfolioProvenanceState::FillHistoryIncomplete
        );
    }

    #[test]
    fn matching_snapshot_and_complete_epoch_is_active() {
        let snap = snap_id("a");
        let row = accounting_row("complete", Some(snap));
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(snap), Some(&row)),
            PortfolioProvenanceState::Active
        );
    }
}
