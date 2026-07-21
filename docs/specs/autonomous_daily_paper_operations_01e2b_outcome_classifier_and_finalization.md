# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B — Strict Outcome Classifier and Finalization CAS

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-FINALIZATION-CAS`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Phase: Phase E2B — strict outcome classifier and durable finalization CAS.

Starting HEAD: `705c1010c19501a5550f4fe6a45ad0ed4f8cc912` (`test: close autonomous coverage
concurrency proof` — the accepted E2A commit).

Status: **IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE.** This document
records what E2B built and proved; it is not itself an acceptance record, and it does not close
Phase E or Bundle 3.

## 0. Accepted foundation (recorded, not re-litigated)

```text
D1–D4: ACCEPTED — COMPLETE
PHASE D: ACCEPTED — COMPLETE

E1: ACCEPTED — COMPLETE
E2A: ACCEPTED — COMPLETE

PHASE E: OPEN
BUNDLE 3: OPEN
```

E2B is built entirely on top of E2A's accepted durable coverage-anchor and run-lineage
authorities (`core-rs/crates/mqk-daemon/src/state/autonomous_daily_coverage_authority.rs`,
`mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage`) and the E1 outcome contract
(`docs/specs/autonomous_daily_paper_operations_01e_outcome_truth_contract.md`). Neither is
reopened or re-derived by this patch; every E2B evidence read consumes them as-is.

## 1. Scope

E2B implements exactly the strict classifier and finalization write path the E1 contract
authorizes for this sub-phase (§13):

1. the pure evidence-gathering reads (E1 §1/§5/§6), consuming E2A's authorities rather than
   recomputing them;
2. the pure classification function (E1 §8's corrected precedence, §10's reason-code table);
3. the finalization CAS write path (`outcome`/`finalized_at_utc`, atomic with the terminal state
   transition);
4. the two new `stopping`/`stop_retrying -> evidence_degraded` legal-transition edges the E1
   contract §3.3 authorizes;
5. the §9 database-failure write contract (complete-outage vs. partial-read-failure,
   authoritative re-read before any success claim).

**No coordinator invocation, no API route, no GUI surface, and no notification are implemented.**
`classify_and_finalize_autonomous_daily_operation` is not called from any production tick by this
patch — E3 (not started, not authorized by this document) owns wiring it into the coordinator's
`AwaitingOutcomeFinalization` handling.

## 2. Architecture

`core-rs/crates/mqk-daemon/src/state/autonomous_daily_outcome.rs` splits the classifier into two
strictly separated halves:

- **Evidence gathering** (`gather_autonomous_daily_outcome_evidence`, async, DB-read-only):
  performs every database read for one classification attempt up front, into one durable
  [`AutonomousDailyOutcomeEvidenceSnapshot`]. No classification decision is made here.
- **Classification** (`classify_autonomous_daily_outcome`, pure, `fn`, no `async`): applies the
  E1 contract's exact global precedence order over an already-gathered snapshot. No I/O, no wall
  clock. This split means every "REQUIRED CLASSIFIER TEST" in the test matrix below constructs a
  snapshot by hand and calls the pure function directly — no database, no `AppState`, no
  coordinator/adapter machinery.

Neither half ever reads a process-local diagnostic field: `AutonomousDailyOutcomeEvidenceSnapshot`
carries no `AppState`-shaped field, no `bar_tick_dispatch_count`, no `last_bar_signal_qty`, and no
completed-bar-task liveness cache — structurally impossible to smuggle in, not merely avoided by
convention.

## 3. Finalization eligibility (E1 §3.2)

`check_finalization_eligibility(operation, context)` is pure and requires:

```text
state ∈ {stopping, stop_retrying}          -- else NotEligible, zero evidence reads
context.matching_local_runtime_active = false
stopped_at_utc IS NOT NULL                 -- else RuntimeStopUnproven
```

`AutonomousDailyFinalizationContext { matching_local_runtime_active: bool }` is the one
process-local fact the caller supplies explicitly — E3 will source it from
`AppState::locally_owned_run_id()`; E2B never reaches into `AppState` itself.

Both `NotEligible` and `RuntimeStopUnproven` produce **zero durable writes of any kind** — per the
E1 contract §7, `unknown_runtime_stop_unproven` means "not yet eligible," not "evidence gap," so
this patch never persists that code as a blocker; it is reserved (and unused by this patch) for a
genuinely contradictory eligibility fact a future audit might locate.

`postclose_finalize_utc` is not read as an eligibility gate, matching the E1 contract's binding
rule (§3.1).

## 4. Durable evidence snapshot

`AutonomousDailyOutcomeEvidenceSnapshot` carries, per classification attempt:

```text
operation identity / current run_id
identity_match                    -- current assignment/runtime-binding identity vs. persisted
lineage: Option<Vec<Uuid>>        -- E2A's validated full run lineage, or None on failure
coverage: Option<CoverageBoundDetail>  -- E2A's Compatible anchor, or None otherwise
expected_strategy_id: Option<String>   -- current resolved strategy id
expected_bars: Vec<i64>           -- derived from `coverage` (§5 below)
claims: Vec<ExpectedBarClaimEvidence>  -- one entry per expected bar: raw claim + raw evaluation
total_claim_count: i64            -- every dispatch-claim row ever created for this operation
aggregate_bars_observed/dispatched/last_completed_bar_ts/last_dispatched_bar_ts
outbox_rows: Vec<OutboxRow>       -- every oms_outbox row across the full validated lineage
inbox_rows: Vec<InboxRow>         -- every oms_inbox row across the full validated lineage
```

`gather_autonomous_daily_outcome_evidence` populates every field with one pass of reads:
`resolve_current_coverage_policy_inputs` → `coverage_construction_inputs_from_operation` →
`construct_coverage_bound_detail` → `check_coverage_authority` (E2A's own Stage A/B authority
check) for `coverage`; `fetch_and_validate_autonomous_daily_operation_run_lineage` for `lineage`;
`mqk_db::fetch_autonomous_daily_bar_dispatch` + `mqk_db::fetch_strategy_signal_evaluation` per
expected bar for `claims`; two new narrow read helpers,
`mqk_db::outbox_load_all_for_run`/`mqk_db::inbox_load_all_for_run` (unbounded, any status/event
kind, one call per lineage `run_id`), for `outbox_rows`/`inbox_rows`.

A policy-resolution or coverage-construction *failure* is a legitimate "coverage unavailable"
result (`None`) — never a propagated error. Only a genuine database I/O failure propagates as
`Err`, which the caller (§9 below) treats as a partial-read failure.

## 5. Exact expected dispatch-bar set (pure)

`derive_expected_bar_set(&CoverageBoundDetail) -> Vec<i64>` reuses only
`daily_data_readiness::intraday_grid_starts` — no second calendar/timeframe algorithm:

```text
first_dispatchable_bar_end_ts
+ every current-session grid identity g such that:
    g's expectation instant (g + timeframe_secs + effective_grace_seconds)
      is strictly greater than the first bar's own expectation instant
      and strictly less than effective_operation_close_utc
```

This is exactly the coverage window E1 §6 item 2 defines, computed from the already-bound,
immutable anchor — never from `started_at_utc`, never narrowed for a late start or a recovery gap.
A middle bar with no durable claim under any run in the lineage is indistinguishable, to this
function's caller, from any other missing expected identity; it is never silently excused.

## 6. Global precedence (E1 §8, restated by §13)

`classify_autonomous_daily_outcome` applies, in order, the first matching rule deciding:

```text
1. identity_match                                  -> else unknown_assignment_identity_unavailable
2. lineage present                                  -> else unknown_run_lineage_unavailable
3. coverage present                                  -> else:
     lineage empty (operation never reached running) -> unknown_missing_evaluation_evidence
     lineage nonempty (operation did run)             -> unknown_incomplete_bar_coverage
4. expected-bar coverage complete + aggregate consistent -> else unknown_incomplete_bar_coverage
5. zero unresolved claims (status == completed, evaluation_id present) -> else
     unknown_unresolved_dispatch_claim / unknown_missing_evaluation_evidence
6. exact evaluation evidence per claim (evaluation_id match, lineage membership,
   strategy/symbol/timeframe/decision_stage/bar-identity agreement, no duplicate
   evaluation_id across claims) -> else unknown_missing_evaluation_evidence
7. activity tier (fill > order-submitted > decision-accepted) -> CompletedWithActivity
8. nonzero signal with zero outbox evidence -> unknown_order_evidence_conflict
9. otherwise -> CompletedNoTrade { no_trade_strategy_evaluated_no_signal }
```

Step 3's split (missing-anchor-with-empty-lineage vs. missing-anchor-with-nonempty-lineage) is a
source-grounded reconciliation of two E1 contract statements that are otherwise in tension: §6
item 2 step 6 says an operation that never reached a state where the anchor could be established
routes to `unknown_missing_evaluation_evidence`; §7's own trigger list keys the same code on "zero
durable `strategy_signal_evaluations` rows... whether the operation reached running... or it
legally reached stopping without ever reaching running at all." Because a durably-bound anchor
(§6a) is written by the coordinator's `ensure_coverage_authority` before `dispatch_by_state` ever
runs, a missing anchor for an operation that *did* bind a run is always a genuine coverage-proof
gap (`unknown_incomplete_bar_coverage`), never the "never ran" case; the empty-lineage split
therefore recovers exactly the E1 contract's intended disjoint cases without inventing a new
reason code. This mapping is proven by `classifier_18` (nonempty lineage) in the test matrix.

`sys_risk_denial_events` is never read by any function in this module (`classifier_21` is a
structural proof: the snapshot type carries no such field).

## 7. Terminal and unknown reason codes

```text
Terminal (AutonomousDailyTerminalReason):
  activity_fill_confirmed            -> completed_with_activity
  activity_order_submitted           -> completed_with_activity
  activity_decision_accepted         -> completed_with_activity
  no_trade_strategy_evaluated_no_signal -> completed_no_trade

Unknown (AutonomousDailyUnknownReason):
  unknown_assignment_identity_unavailable
  unknown_run_lineage_unavailable
  unknown_incomplete_bar_coverage
  unknown_unresolved_dispatch_claim
  unknown_missing_evaluation_evidence
  unknown_order_evidence_conflict
  unknown_database_unavailable
  unknown_runtime_stop_unproven      -- reserved; not persisted by this patch (§3 above)
```

`no_trade_all_signals_blocked`, `no_trade_no_bar_expected`, and
`unknown_insufficient_bar_evidence` are deliberately absent, per the E1 contract's binding
disposition (§9, §6 Correction 6) — not authorized for this patch under any name.

Generic `completed` is not a representable output of `AutonomousDailyOutcomeClassification` at
all: the enum has exactly three variants (`CompletedWithActivity`, `CompletedNoTrade`,
`EvidenceBlocked`), none of which can carry the `"completed"` state string
(`classifier_24_generic_completed_is_never_produced`).

## 8. Terminal finalization CAS

`mqk_db::finalize_autonomous_daily_operation` (new, `core-rs/crates/mqk-db/src/autonomous_daily_operation.rs`)
extends the existing CAS-transition contract to set `outcome`/`finalized_at_utc` atomically with
the terminal state transition:

```text
Allowed: (stopping | stop_retrying) -> (completed_no_trade | completed_with_activity)
Atomic:  state, outcome, finalized_at_utc, state_reason_code = null,
         state_blocker_signature = null, next_retry_utc = null, last_error = null,
         state_version += 1, one matching event row (reason_code = outcome)
Never:   no_trade_reason (retired per E1 contract §2, never written)
Illegal: generic `completed` target, an outcome outside the closed four-code set,
         or an expected_state outside {stopping, stop_retrying}
```

Outcome classification (`FinalizeAutonomousDailyOperationOutcome`):

```text
Applied                 -- CAS succeeded
AlreadyApplied           -- already terminal at exactly this state+outcome (read-only)
ConflictingTerminalTruth -- already terminal at a *different* state/outcome (never rewritten)
StaleState                -- not yet terminal, but state/version didn't match
NotFound / IllegalTarget
```

## 9. Evidence-degraded CAS and recovery

Two new legal edges (`is_legal_operation_transition`, `mqk-db`):

```text
stopping       -> evidence_degraded
stop_retrying  -> evidence_degraded
```

No migration: `evidence_degraded` was already a legal `state`/`from_state`/`to_state` value; only
the pure Rust transition-graph function changes, exactly the same seam D2 already extended five
times.

`apply_evidence_degraded_blocker` persists the exact `unknown_*` reason via the existing D1
blocker-signature mechanism reused verbatim: `AutonomousCoordinatorReason::UnclassifiedFailClosed
{ fault_class: reason.as_str() }` through `blocker_signature()` — no new variant added to that
enum, no change to `autonomous_retry_policy.rs`. When the operation is already
`evidence_degraded`, the same-state `refresh_autonomous_daily_operation_blocker` CAS is used
instead of a transition, so an exact-reason replay is a genuine no-op (zero duplicate event,
`store_41`).

`evidence_degraded -> stopping` (already legal) is the recovery edge: once a later classification
attempt's evidence resolves to a terminal classification, `recover_evidence_degraded_to_stopping`
takes that edge and returns `RecoveredToStopping` — **never finalizes directly from
`evidence_degraded`**. A later invocation, now starting from `stopping`, performs the ordinary
terminal CAS. A mid-run `evidence_degraded` row (`stopped_at_utc IS NULL`, reached via the
pre-existing `running -> evidence_degraded` edge) is never eligible for this recovery path
(`store_43`).

## 10. Commit uncertainty and database-failure contract (E1 §9)

Every CAS write (`finalize_with_commit_uncertainty`, `apply_evidence_degraded_blocker`) follows
the same authoritative-re-read discipline D4 established: on `StaleState`/`NotFound`/a store
error, the row is re-read by `operation_id` and the exact expected fields
(`state`/`outcome`/`finalized_at_utc`, or `state`/`state_reason_code`/null-`outcome`) are checked
before any success is claimed. Never re-runs classification on an uncertain write without first
re-reading.

Database-failure disposition, matched exactly to E1 §9:

```text
Complete outage (operation row itself cannot be loaded)
  -> DatabaseUnavailable, zero write attempts of any kind (store_46)

Partial read failure (operation row loaded, a later evidence query fails)
  -> best-effort unknown_database_unavailable blocker write, confirmed by re-read
  -> re-read confirms  -> EvidenceDegraded
  -> re-read fails too -> DatabaseUnavailable, never a fabricated "blocker written" claim
     (store_47 proves the ordinary non-DB-outage evidence-gap path through the same seam)
```

No raw SQL/connection/credential text ever enters a durable field — every persisted reason is one
of the closed `unknown_*` codes above.

## 11. High-level entry point

```rust
pub async fn classify_and_finalize_autonomous_daily_operation(
    pool: &PgPool,
    operation_id: Uuid,
    now_utc: DateTime<Utc>,
    context: AutonomousDailyFinalizationContext,
    policy_inputs: &AutonomousDailyFinalizationPolicyInputs<'_>,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome>
```

`AutonomousDailyFinalizationPolicyInputs` (`calendar_provider`, `config`, `binding`,
`strategy_registry`) mirrors exactly the parameters the coordinator's own
`ensure_coverage_authority`/`resolve_current_coverage_policy_inputs` already thread through — a
narrower, source-aligned signature than a full `state: &Arc<AppState>` parameter, per the E2B
mission's own allowance; none of these four inputs require `AppState` to construct. E3's job is to
resolve them from `AppState`/env exactly as the coordinator does today, and supply the process-local
`matching_local_runtime_active` fact.

`AutonomousDailyFinalizationOutcome` has exactly the seven required variants: `NotEligible`,
`AlreadyFinalized`, `Finalized`, `EvidenceDegraded`, `RecoveredToStopping`, `DatabaseUnavailable`,
`Conflict`.

Not called from any production tick by this patch.

## 12. Test matrix

File:
`core-rs/crates/mqk-daemon/tests/scenario_autonomous_daily_outcome_classifier_and_finalization_01.rs`
(54 tests total).

```text
Group 1 (pure classifier + eligibility, no DB): 29 tests
  classifier_01–classifier_25 (the 25 required classifier scenarios)
  eligibility_* (4 pure eligibility-gate proofs)

Group 2 (DB-backed finalization/blocker CAS store proof, --ignored): 22 tests
  store_26–store_47 (the 22 required store scenarios)

Group 3 (DB-backed integrated end-to-end proof, --ignored): 3 tests
  integrated_01_clean_no_trade_evidence_reaches_completed_no_trade
  integrated_02_clean_fill_evidence_reaches_completed_with_activity
  integrated_03_unresolved_claim_degrades_then_repairs_then_finalizes
    (unresolved claim -> evidence_degraded -> repair fixture ->
     evidence_degraded -> stopping -> later invocation -> completed_no_trade)
```

All 54 pass locally (`--include-ignored --test-threads=1`, isolated port-5434 test DB).

Regressions re-run clean against the same isolated DB, one binary at a time
(`--include-ignored --test-threads=1`):
`scenario_autonomous_daily_coverage_anchor_and_run_lineage_01` (41/41),
`scenario_autonomous_daily_operation_store_01` (26/26),
`scenario_autonomous_daily_operation_lifecycle_01` (36/36),
`scenario_autonomous_daily_operation_data_evidence_01` (9/9),
`scenario_autonomous_daily_phase_d_integration_01` (8/8),
`scenario_signal_evaluation_journal_auton_no_signal_obs_01` (7/7).

`scenario_autonomous_completed_bar_driver_01`'s known baseline (47 passed, 9 pre-existing
failures, confirmed unrelated by E2A's own audit) is unaffected — this patch touches no production
seam in that file and was not re-run per the mission's own instruction.

## 13. Guard-status note (E2A boundary guard)

`scripts/guards/validate_autonomous_daily_paper_operations_01e2a_coverage_anchor_and_run_lineage.ps1`
now fails with **2 violations** against this commit — both expected, documented consequences of
E2A's acceptance and E2B's authorized work landing in the same commit, not defects:

1. **Check `[10]`** ("no E2B classifier/finalizer surface was introduced") — its substring check
   that `core-rs/crates/mqk-db/src/autonomous_daily_operation.rs` never contains
   `"finalized_at_utc ="` now fails. That check was written as a point-in-time boundary proof for
   the **E2A patch itself** (frozen at E2A's accepted commit `705c1010`), verifying E2A's own patch
   did not overreach into E2B's scope. E2B is the explicitly authorized next patch, and its entire
   mission is to add exactly the finalization surface that check was written to detect the
   *absence* of at E2A-acceptance-time.
2. **The README `[11]` acceptance-phrase check** — its check that README.md still contains the
   pre-acceptance phrase `"E2A repair implementation is complete, awaiting independent"` now fails,
   because this commit correctly updates that sentence to record E2A as **accepted** (per this
   patch's own starting-point instruction: `E2A: ACCEPTED — COMPLETE`). Requiring the old
   not-yet-accepted phrase to persist forever would make it impossible to ever honestly record
   E2A's acceptance in README.md.

Neither check is a permanent prohibition on the codebase — both were point-in-time proofs scoped
to the E2A patch's own acceptance state, and continuing to require either to pass would make it
impossible to both (a) honestly record E2A's acceptance and (b) implement E2B's explicitly
authorized mission. Every other required existing guard passes; see the final handoff for the
complete list.

## 14. E3 boundary

E2B implements no coordinator invocation, no notification, no API route, no GUI. E3 (not started,
not authorized here) wires `classify_and_finalize_autonomous_daily_operation` into the
coordinator's handling of `AwaitingOutcomeFinalization`, sources
`AutonomousDailyFinalizationPolicyInputs` and `AutonomousDailyFinalizationContext` from
`AppState`/env exactly as the coordinator already does for coverage-anchor binding, wires the §12
notification contract (frozen, unimplemented), and proves restart-safety across a crash between
stop and finalization. None of this exists yet. No `completed*` transition is reachable through
any production code path added by E2B.
