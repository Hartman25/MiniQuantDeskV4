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
// Closed vocabulary -- exactly these eight states, no others:
//   active
//   fill_history_incomplete
//   accounting_epoch_unavailable
//   accounting_snapshot_mismatch
//   not_found
//   query_failed
//   unsupported_source
//   invalid_snapshot
//
// DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-READ-SIDE-AUTHORITY-REPAIR: the
// write seam (`snapshot.rs`, `mqk_db::upsert_paper_portfolio_accounting_state`)
// already refuses to *persist* a non-USD, runless, blank-symbol, or otherwise
// invalid authoritative snapshot -- but a legacy or directly-inserted
// malformed row can still exist in `sys_paper_portfolio_snapshots` and, prior
// to this repair, every read seam selected the latest run-scoped row using
// only `deployment_mode`/`source`/`run_id`, never independently re-validating
// its currency/truth_state/position content before reporting it as
// authoritative. `validate_run_scoped_snapshot_authority` /
// `validate_snapshot_scalar_authority` below are the one shared, pure
// validator every durable read route (`durable_portfolio.rs`,
// `paper_lifecycle.rs`) now calls before trusting a selected row --
// `InvalidSnapshot` fails closed on the selected latest row rather than
// falling back to an older one, so corruption or legacy-incompatible truth
// stays visible instead of silently masked.

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
    InvalidSnapshot,
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
            Self::InvalidSnapshot => "invalid_snapshot",
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
/// `snapshot_invalid` -- `true` when a run-scoped snapshot exists
/// (`snapshot_id.is_some()`) but failed
/// [`validate_run_scoped_snapshot_authority`]. Always `false` when no
/// snapshot exists at all (`snapshot_id.is_none()`) -- that case is the
/// pre-existing `AccountingEpochUnavailable` path below, unrelated to this
/// repair. Checked before the accounting row is even inspected: a malformed
/// selected snapshot must never be paired with an accounting row, however
/// well-formed that accounting row is (B4 final read-side authority
/// repair).
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
///
/// Additional behavior (B4 final read-side authority repair):
/// - `snapshot_invalid == true` -> `InvalidSnapshot`, checked immediately
///   after the query-failure gate and before the accounting row is even
///   matched -- a malformed selected snapshot fails closed regardless of
///   whether an accounting row exists or what it contains.
pub(crate) fn classify_portfolio_provenance(
    is_paper_mode: bool,
    snapshot_query_failed: bool,
    accounting_query_failed: bool,
    snapshot_id: Option<Uuid>,
    snapshot_invalid: bool,
    accounting: Option<&mqk_db::PaperPortfolioAccountingStateRecord>,
) -> PortfolioProvenanceState {
    if !is_paper_mode {
        return PortfolioProvenanceState::UnsupportedSource;
    }
    if snapshot_query_failed || accounting_query_failed {
        return PortfolioProvenanceState::QueryFailed;
    }
    if snapshot_invalid {
        return PortfolioProvenanceState::InvalidSnapshot;
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

// ---------------------------------------------------------------------------
// Shared read-side snapshot-authority validator (B4 final read-side
// authority repair).
// ---------------------------------------------------------------------------

/// Bounded, closed-vocabulary reason a selected durable snapshot fails the
/// read-side authority contract. Never carries raw row content (no symbol
/// text, no ids, no DB error) -- every route projects this into a fixed
/// message via [`SnapshotAuthorityViolation::blocker_message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotAuthorityViolation {
    /// Run-scoped check only: the selected snapshot's `run_id` does not
    /// equal the resolved run. (SQL `run_id = $run_id` already prevents this
    /// in practice for the run-scoped fetch helper, but the validator checks
    /// it independently rather than assuming the caller passed the right
    /// row.)
    RunIdMismatch,
    /// Scalar/history check only: the row's `run_id` is `NULL`.
    RunIdMissing,
    NotPaperDeploymentMode,
    NotExternalAlpacaSource,
    NotUsdCurrency,
    NotActiveTruthState,
    BlankPositionSymbol,
    DuplicatePositionSymbol,
    NonzeroQtyWithNonPositiveAvgPrice,
    PositionProvenanceNotExternalAlpaca,
}

impl SnapshotAuthorityViolation {
    /// Fixed, bounded message safe to place on a public response's
    /// `blockers`/`*_unavailable_reason` field. Never includes the raw
    /// symbol, snapshot id, or any other row content.
    pub(crate) fn blocker_message(self) -> &'static str {
        match self {
            Self::RunIdMismatch => {
                "selected durable snapshot does not belong to the resolved run"
            }
            Self::RunIdMissing => "durable snapshot row has no recorded run_id",
            Self::NotPaperDeploymentMode => {
                "durable snapshot deployment_mode is not 'paper'"
            }
            Self::NotExternalAlpacaSource => {
                "durable snapshot source is not 'external_alpaca'"
            }
            Self::NotUsdCurrency => "durable snapshot currency is not 'USD'",
            Self::NotActiveTruthState => "durable snapshot truth_state is not 'active'",
            Self::BlankPositionSymbol => {
                "durable snapshot contains a blank position symbol"
            }
            Self::DuplicatePositionSymbol => {
                "durable snapshot contains a duplicate position symbol"
            }
            Self::NonzeroQtyWithNonPositiveAvgPrice => {
                "durable snapshot contains a nonzero position with a non-positive average entry price"
            }
            Self::PositionProvenanceNotExternalAlpaca => {
                "durable snapshot contains a position with provenance other than 'external_alpaca'"
            }
        }
    }
}

/// Scalar-only authority fields shared by both validators below:
/// `deployment_mode == "paper"`, `source == "external_alpaca"`,
/// `currency == "USD"`, `truth_state == "active"`. Does not check `run_id`
/// -- the two callers below have different run-identity requirements (exact
/// match vs. merely non-null).
fn validate_scalar_fields(
    record: &mqk_db::PaperPortfolioSnapshotRecord,
) -> Result<(), SnapshotAuthorityViolation> {
    if record.deployment_mode != "paper" {
        return Err(SnapshotAuthorityViolation::NotPaperDeploymentMode);
    }
    if record.source != mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA {
        return Err(SnapshotAuthorityViolation::NotExternalAlpacaSource);
    }
    if record.currency != "USD" {
        return Err(SnapshotAuthorityViolation::NotUsdCurrency);
    }
    if record.truth_state != "active" {
        return Err(SnapshotAuthorityViolation::NotActiveTruthState);
    }
    Ok(())
}

/// Scalar-only authority check for one snapshot-history row (no position
/// children are fetched for the history surface -- see
/// `mqk_db::fetch_recent_paper_portfolio_snapshots`). Requires a non-null
/// `run_id` (never a specific run -- the history surface is not run-scoped)
/// plus the same paper/external_alpaca/USD/active scalar fields every
/// authoritative row must carry.
pub(crate) fn validate_snapshot_scalar_authority(
    record: &mqk_db::PaperPortfolioSnapshotRecord,
) -> Result<(), SnapshotAuthorityViolation> {
    if record.run_id.is_none() {
        return Err(SnapshotAuthorityViolation::RunIdMissing);
    }
    validate_scalar_fields(record)
}

/// Full authority check for one run-scoped selected snapshot (with its
/// position children), as used by durable-summary, durable-positions, and
/// paper-lifecycle. `resolved_run_id` is the already-resolved run this
/// snapshot was selected for -- the snapshot's own `run_id` must equal it
/// exactly. Position content is validated in the order the contract lists:
/// nonblank-after-trim symbol, per-snapshot symbol uniqueness, a strictly
/// positive average entry price whenever quantity is nonzero, and
/// `provenance == "external_alpaca"`. Never trims, repairs, skips,
/// deduplicates, or substitutes malformed data -- the first violation found
/// is returned and the snapshot is rejected whole.
pub(crate) fn validate_run_scoped_snapshot_authority(
    snapshot: &mqk_db::PaperPortfolioSnapshotWithPositions,
    resolved_run_id: Uuid,
) -> Result<(), SnapshotAuthorityViolation> {
    if snapshot.snapshot.run_id != Some(resolved_run_id) {
        return Err(SnapshotAuthorityViolation::RunIdMismatch);
    }
    validate_scalar_fields(&snapshot.snapshot)?;

    let mut seen_symbols = std::collections::HashSet::new();
    for position in &snapshot.positions {
        let trimmed = position.symbol.trim();
        if trimmed.is_empty() {
            return Err(SnapshotAuthorityViolation::BlankPositionSymbol);
        }
        if !seen_symbols.insert(trimmed) {
            return Err(SnapshotAuthorityViolation::DuplicatePositionSymbol);
        }
        if position.qty_signed != 0 && position.avg_entry_price_micros <= 0 {
            return Err(SnapshotAuthorityViolation::NonzeroQtyWithNonPositiveAvgPrice);
        }
        if position.provenance != mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA {
            return Err(SnapshotAuthorityViolation::PositionProvenanceNotExternalAlpaca);
        }
    }
    Ok(())
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

    fn valid_snapshot(
        run_id: Uuid,
        snapshot_id: Uuid,
    ) -> mqk_db::PaperPortfolioSnapshotWithPositions {
        mqk_db::PaperPortfolioSnapshotWithPositions {
            snapshot: mqk_db::PaperPortfolioSnapshotRecord {
                snapshot_id,
                captured_at_utc: Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
                deployment_mode: "paper".to_string(),
                source: mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
                equity_micros: 100_000_000_000,
                cash_micros: 40_000_000_000,
                currency: "USD".to_string(),
                truth_state: "active".to_string(),
                run_id: Some(run_id),
                operation_id: None,
            },
            positions: vec![mqk_db::PaperPortfolioSnapshotPosition {
                symbol: "AAPL".to_string(),
                qty_signed: 10,
                avg_entry_price_micros: 150_000_000,
                provenance: mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            }],
        }
    }

    #[test]
    fn non_paper_run_is_unsupported_source_regardless_of_other_inputs() {
        let snap = snap_id("a");
        let row = accounting_row("complete", Some(snap));
        assert_eq!(
            classify_portfolio_provenance(false, false, false, Some(snap), false, Some(&row)),
            PortfolioProvenanceState::UnsupportedSource
        );
    }

    #[test]
    fn snapshot_query_failure_is_query_failed_not_not_found() {
        assert_eq!(
            classify_portfolio_provenance(true, true, false, None, false, None),
            PortfolioProvenanceState::QueryFailed
        );
    }

    #[test]
    fn accounting_query_failure_is_query_failed_not_not_found() {
        assert_eq!(
            classify_portfolio_provenance(true, false, true, None, false, None),
            PortfolioProvenanceState::QueryFailed
        );
    }

    #[test]
    fn no_accounting_row_is_not_found() {
        let snap = snap_id("a");
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(snap), false, None),
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
            classify_portfolio_provenance(true, false, false, None, false, Some(&row)),
            PortfolioProvenanceState::AccountingEpochUnavailable
        );
    }

    #[test]
    fn missing_source_snapshot_id_is_accounting_epoch_unavailable() {
        let snap = snap_id("a");
        let row = accounting_row("complete", None);
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(snap), false, Some(&row)),
            PortfolioProvenanceState::AccountingEpochUnavailable
        );
    }

    #[test]
    fn mismatched_source_snapshot_is_accounting_snapshot_mismatch() {
        let selected = snap_id("selected");
        let stale = snap_id("stale");
        let row = accounting_row("complete", Some(stale));
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(selected), false, Some(&row)),
            PortfolioProvenanceState::AccountingSnapshotMismatch
        );
    }

    #[test]
    fn mismatch_takes_priority_over_incomplete_epoch() {
        let selected = snap_id("selected");
        let stale = snap_id("stale");
        let row = accounting_row("incomplete", Some(stale));
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(selected), false, Some(&row)),
            PortfolioProvenanceState::AccountingSnapshotMismatch
        );
    }

    #[test]
    fn incomplete_epoch_with_matching_snapshot_is_fill_history_incomplete() {
        let snap = snap_id("a");
        let row = accounting_row("incomplete", Some(snap));
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(snap), false, Some(&row)),
            PortfolioProvenanceState::FillHistoryIncomplete
        );
    }

    #[test]
    fn matching_snapshot_and_complete_epoch_is_active() {
        let snap = snap_id("a");
        let row = accounting_row("complete", Some(snap));
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(snap), false, Some(&row)),
            PortfolioProvenanceState::Active
        );
    }

    // -----------------------------------------------------------------
    // B4 final read-side authority repair: InvalidSnapshot precedence.
    // -----------------------------------------------------------------

    #[test]
    fn invalid_snapshot_wins_even_with_a_well_formed_accounting_row() {
        let snap = snap_id("a");
        let row = accounting_row("complete", Some(snap));
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(snap), true, Some(&row)),
            PortfolioProvenanceState::InvalidSnapshot,
            "a malformed selected snapshot must never be paired with an accounting row, however well-formed"
        );
    }

    #[test]
    fn invalid_snapshot_wins_even_with_no_accounting_row() {
        let snap = snap_id("a");
        assert_eq!(
            classify_portfolio_provenance(true, false, false, Some(snap), true, None),
            PortfolioProvenanceState::InvalidSnapshot,
            "invalid_snapshot must not be collapsed into not_found"
        );
    }

    #[test]
    fn query_failed_still_takes_priority_over_snapshot_invalid() {
        let snap = snap_id("a");
        assert_eq!(
            classify_portfolio_provenance(true, true, false, Some(snap), true, None),
            PortfolioProvenanceState::QueryFailed
        );
    }

    // -----------------------------------------------------------------
    // Shared snapshot-authority validator
    // -----------------------------------------------------------------

    #[test]
    fn valid_run_scoped_snapshot_passes() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let snap = valid_snapshot(run_id, snap_id("valid"));
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Ok(())
        );
    }

    #[test]
    fn run_scoped_validator_rejects_run_id_mismatch() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let other_run = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-y");
        let snap = valid_snapshot(run_id, snap_id("valid"));
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, other_run),
            Err(SnapshotAuthorityViolation::RunIdMismatch)
        );
    }

    #[test]
    fn run_scoped_validator_rejects_null_run_id() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.snapshot.run_id = None;
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Err(SnapshotAuthorityViolation::RunIdMismatch)
        );
    }

    #[test]
    fn run_scoped_validator_rejects_non_paper_deployment_mode() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.snapshot.deployment_mode = "live".to_string();
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Err(SnapshotAuthorityViolation::NotPaperDeploymentMode)
        );
    }

    #[test]
    fn run_scoped_validator_rejects_non_external_alpaca_source() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.snapshot.source =
            mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_SYNTHETIC_DIAGNOSTIC.to_string();
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Err(SnapshotAuthorityViolation::NotExternalAlpacaSource)
        );
    }

    #[test]
    fn run_scoped_validator_rejects_non_usd_currency() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.snapshot.currency = "EUR".to_string();
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Err(SnapshotAuthorityViolation::NotUsdCurrency)
        );
    }

    #[test]
    fn run_scoped_validator_rejects_non_active_truth_state() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.snapshot.truth_state = "quarantined".to_string();
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Err(SnapshotAuthorityViolation::NotActiveTruthState)
        );
    }

    #[test]
    fn run_scoped_validator_rejects_blank_position_symbol() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.positions[0].symbol = "   ".to_string();
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Err(SnapshotAuthorityViolation::BlankPositionSymbol)
        );
    }

    #[test]
    fn run_scoped_validator_rejects_duplicate_position_symbol() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        let dup = snap.positions[0].clone();
        snap.positions.push(dup);
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Err(SnapshotAuthorityViolation::DuplicatePositionSymbol)
        );
    }

    #[test]
    fn run_scoped_validator_rejects_nonzero_qty_with_non_positive_avg_price() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.positions[0].avg_entry_price_micros = 0;
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Err(SnapshotAuthorityViolation::NonzeroQtyWithNonPositiveAvgPrice)
        );
    }

    #[test]
    fn run_scoped_validator_allows_zero_qty_with_zero_avg_price() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.positions[0].qty_signed = 0;
        snap.positions[0].avg_entry_price_micros = 0;
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Ok(())
        );
    }

    #[test]
    fn run_scoped_validator_rejects_position_provenance_not_external_alpaca() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.positions[0].provenance = "manual_override".to_string();
        assert_eq!(
            validate_run_scoped_snapshot_authority(&snap, run_id),
            Err(SnapshotAuthorityViolation::PositionProvenanceNotExternalAlpaca)
        );
    }

    #[test]
    fn scalar_validator_accepts_a_valid_row() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let snap = valid_snapshot(run_id, snap_id("valid"));
        assert_eq!(validate_snapshot_scalar_authority(&snap.snapshot), Ok(()));
    }

    #[test]
    fn scalar_validator_rejects_null_run_id() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.snapshot.run_id = None;
        assert_eq!(
            validate_snapshot_scalar_authority(&snap.snapshot),
            Err(SnapshotAuthorityViolation::RunIdMissing)
        );
    }

    #[test]
    fn scalar_validator_does_not_require_a_specific_run() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let other_run = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-y");
        let snap = valid_snapshot(run_id, snap_id("valid"));
        // Scalar authority (history surface) is not run-scoped -- any
        // non-null run_id is accepted, unlike the run-scoped validator.
        assert_ne!(snap.snapshot.run_id, Some(other_run));
        assert_eq!(validate_snapshot_scalar_authority(&snap.snapshot), Ok(()));
    }

    #[test]
    fn scalar_validator_rejects_non_usd_currency() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test.portfolio-provenance.v1|run-x");
        let mut snap = valid_snapshot(run_id, snap_id("valid"));
        snap.snapshot.currency = "EUR".to_string();
        assert_eq!(
            validate_snapshot_scalar_authority(&snap.snapshot),
            Err(SnapshotAuthorityViolation::NotUsdCurrency)
        );
    }
}
