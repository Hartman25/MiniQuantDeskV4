# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5 — Integrated Phase E Proof and Closure

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-CLOSURE`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Phase: Phase E5 — integrated Phase E proof and closure.

Starting HEAD: `11664945e90a582e6984f0eab66cf89690120769` (`fix: require exact autonomous market
date query` — the accepted E4 closing commit).

Status: **IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE.** This document
records what E5 built and proved; it is not itself an acceptance record, and does not itself close
Phase E or Bundle 3 — closure requires independent ChatGPT/operator acceptance of this patch.

## 0. Accepted foundation (recorded, not re-litigated)

```text
D1–D4: ACCEPTED — COMPLETE
PHASE D: ACCEPTED — COMPLETE

E1:  ACCEPTED — COMPLETE
E2A: ACCEPTED — COMPLETE
E2B: ACCEPTED — COMPLETE
E3:  ACCEPTED — COMPLETE
E4:  ACCEPTED — COMPLETE

E5:      IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE
PHASE E: IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE
BUNDLE 3: OPEN
```

Accepted commits, per phase (recorded for reconciliation; `git log`/the committed repo remain the
authoritative source per `.claude/rules/audit_repo_truth_rules.md`):

```text
E1  binding contract: docs/specs/autonomous_daily_paper_operations_01e_outcome_truth_contract.md
    (four correction passes; accepted at 3591064a "docs: require coverage authority before
    bar processing")
E2A 6c77b6f1 "daemon: bind autonomous coverage authority" (original) + 0e3799bd (closure repair)
    + 705c1010 "test: close autonomous coverage concurrency proof" (final proof repair, accepted)
E2B 1918cee9 "daemon: add durable autonomous outcome finalizer" (original) + 1739864f
    "fix: harden autonomous outcome terminal truth" (terminal-truth-precedence closure, accepted)
E3  1891bd32 "daemon: integrate autonomous daily finalization" (original) + 6f96b984
    "fix: preserve finalization eligibility on policy failure" (matching-runtime-policy-failure-
    gate repair, accepted)
E4  176b4149 "api: expose autonomous daily operation outcomes" (original) + b2328a93
    "fix: preserve daily operation read truth on evidence failure" (read-truth repair) +
    11664945 "fix: require exact autonomous market date query" (exact-parser repair, accepted)
```

This document does not reopen or redesign any accepted E1–E4 behavior. No source-proven correctness
contradiction was found during the E5 integrated audit that would make closure unsafe (per the
mission's own binding condition for reopening prior phases).

## 1. Scope

E5 implements exactly the integrated proof and reconciliation the mission authorizes:

1. one new integrated scenario test file
   (`core-rs/crates/mqk-daemon/tests/scenario_autonomous_daily_phase_e_closure_01.rs`) proving the
   six integrated proofs below against the real, isolated port-5434 test database, the real
   production coordinator/finalizer/API seams, and fake/in-memory notifier instrumentation only;
2. this closure specification;
3. one new closure guard
   (`scripts/guards/validate_autonomous_daily_paper_operations_01e_phase_e_closure.ps1`);
4. narrow, non-semantic status reconciliation in the E2A/E2B/E3/E4 guards' own point-in-time status
   checks;
5. ledger and README/README_TECHNICAL reconciliation.

**No new production Rust behavior is introduced by E5.** Every production file this document
references was already accepted by E1–E4; E5 exercises it, it does not change it.

## 2. Integrated proof A — clean no-trade day

Test: `e5_proof_a_clean_no_trade_day_full_pipeline_and_replay`.

Proves one continuous durable path against a real operation with a real, immutable
`autonomous_daily_coverage_bound` event, a real validated single-run lineage, and every expected bar
completed with one confirmed `strategy_evaluated` flat evaluation:

```text
operation creation
  -> immutable coverage authority (E2A write_and_confirm_coverage_authority)
  -> validated run lineage (E2A fetch_and_validate_autonomous_daily_operation_run_lineage)
  -> every expected bar has exactly one completed dispatch claim
  -> every claim references exactly one confirmed flat/no-signal evaluation
  -> durable runtime stop (record_autonomous_runtime_stopped)
  -> coordinator finalization (handle_outcome_finalization -> classify_and_finalize_...)
  -> completed_no_trade / no_trade_strategy_evaluated_no_signal
  -> exactly one sys_autonomous_daily_operation_events row for the finalizing transition
  -> exactly one autonomous.daily_operation.outcome notification
  -> E4 single-route report: truth_state=active, outcome_class=no_trade,
     finalization_status=finalized, evidence_state=complete, non-null count fields
```

The test then proves a direct typed coordinator-tick call against the same already-terminal row
projects `OutcomeAlreadyFinalized` (read-only), then repeats the coordinator tick: a lifecycle-event
count taken immediately before and after the tick proves zero duplicate lifecycle event, and the
recorder's notification count proves zero duplicate notification. This tick-scoped check is
deliberately separate from the read-only-API snapshot immediately below it, because the coordinator
tick's own accepted compatibility read-surface write (`AutonomousSessionTruth` ->
`sys_autonomous_session_events`, orthogonal to this operation's E1–E4 durable lifecycle/outcome
truth) is unconditionally re-asserted by `log_coordinator_outcome` on every tick — including a
read-only `OutcomeAlreadyFinalized` replay — since its `detail` text embeds per-tick state and so
never compares equal to the prior tick's own detail. That write is expected and out of scope for
E1–E5; it is not itself an unrelated-identity API-route side effect. The API read itself is then
independently proven mutation-free via a full durable before/after snapshot (`state`,
`state_version`, lifecycle-event count, coverage-event count, raw/validated lineage counts,
lineage-scoped run/claim/evaluation/outbox/inbox counts, and the seven global evidence-table
totals) taken immediately before and after the GET call, proving the API call itself causes zero DB
mutation of any kind, and that it observes the same durable outcome.

## 3. Integrated proof B — activity day, full lineage across two runs

Test: `e5_proof_b_activity_day_full_lineage_across_two_runs`.

Builds one operation with a real two-`run_id` lineage (`run A -> recovery_retrying -> run B`,
mirroring the accepted E2A `h01_initial_and_recovery_run_lineage_read_and_validated` recovery
cycle): run A durably records one broker fill and one broker ack, then the operation recovers and
run B completes every expected bar's dispatch claim with a real flat evaluation, then stops.

```text
run A (fill + ack evidence) -> recovery -> run B (full bar coverage) -> stop -> finalization
  -> completed_with_activity / activity_fill_confirmed
```

The operation's mutable `run_id` column ends the day pointing at run B; the terminal classification
and the E4 report both still reflect run A's fill/ack evidence:

- `fill_count` = 1 (run A's fill, read via the full validated lineage, never the current `run_id`
  alone);
- `order_activity_count` >= 1 (run A's ack, per the accepted E4 definition: `oms_outbox` rows plus
  `ack`/`cancel_ack`/`replace_ack`/`reject` `oms_inbox` rows);
- `strategy_evaluation_count` spans the full lineage and is cross-checked directly against a raw DB
  count of run B's own evaluation rows (run A recorded none);
- exactly one terminal notification, zero on replay.

## 4. Integrated proof C — evidence blocker and recovery

Test: `e5_proof_c_evidence_blocker_notifies_once_and_recovers`.

Starts from a stopped, finalization-eligible, otherwise-clean operation whose one expected bar's
dispatch claim is corrupted (`status='failed'`, `evaluation_id=NULL`) after the fact — the durable
evidence a finalization attempt requires is now incomplete.

```text
coordinator tick -> evidence_degraded, state_reason_code=unknown_unresolved_dispatch_claim,
  outcome=NULL, finalized_at_utc=NULL, exactly one warning notification
repeat (unchanged blocker) -> zero duplicate lifecycle event, zero duplicate warning
repair only the test fixture's durable claim/evaluation row
evidence_degraded -> stopping (recovery edge; the same tick never completes directly)
next coordinator tick -> completed_no_trade, exactly one terminal notification
```

E4 reports `blocked_insufficient_evidence` / `evidence_state=degraded` / a bounded
`unknown_unresolved_dispatch_claim` blocker before repair, and `finalized` /
`evidence_state=complete` / `outcome_class=no_trade` after completion.

## 5. Integrated proof D — restart safety

Test: `e5_proof_d_restart_safety_stop_terminal_and_evidence_blocker`.

Every restart step constructs a brand-new `Arc<AppState>` with no in-memory continuity from the
step before it — the same restart-proof convention the accepted E3
`ci_09_10_restart_before_and_after_finalization_is_exactly_once` test already established for this
crate (a literal OS-level process restart is not reachable from inside one `cargo test` binary; a
fresh `AppState` is this codebase's accepted stand-in, since every fact the coordinator/finalizer
reasons from is re-read from durable Postgres state, never from `AppState`'s own construction
history).

```text
D1 restart after durable stop, before finalization:
   fixture built entirely via raw mqk_db calls (no AppState ever observed it) -> first AppState's
   first tick finalizes exactly once

D2 restart after terminal commit:
   a second, independent AppState's tick against the now-terminal row performs zero lifecycle-event
   writes and zero additional notifications

D3 restart after an evidence blocker:
   one AppState degrades a corrupted-claim operation to evidence_degraded (one warning) -> a second,
   independent AppState replays the unchanged blocker silently (zero event delta, zero notification
   delta) -> the claim is repaired -> a third, independent AppState recovers the operation through
   stopping (never completing directly) -> a fourth, independent AppState performs the later
   terminal tick
```

No test in this file uses `tokio::time::sleep` anywhere, for any purpose, including notification
observation. The loopback sink (`PeAlertRecorder`) stores each received payload and only then
publishes the new count on a `tokio::sync::watch` channel; `wait_for_alert_count` polls that
channel's own `changed()`/`borrow()` pair (never a fixed delay) to prove a notification count
deterministically, bounded by a `tokio::time::timeout` used strictly as deadlock/failure
protection — the timeout never makes an assertion pass. Every zero-new-notification (replay) proof
instead reads the recorder's count immediately after its coordinator tick, which is valid because
the audit in this patch confirmed `notify_run_status`/`notify_critical_alert` are `.await`ed
directly inside `run_durable_session_controller_tick`'s outcome match arm and never spawned onto a
separate task — so by the time a tick's own `.await` returns, any notification it would have sent
has already been fully delivered.

## 6. Integrated proof E — API read-only guarantee

Test: `e5_proof_e_api_read_only_guarantee`.

Built on the same two-run activity fixture as Proof B (`pe_two_run_activity_fixture`) rather than
the single-run clean fixture, so the read-only guarantee is exercised over a genuine multi-run
lineage, never trivially satisfied by an operation with only one run. Before taking the snapshot,
the test independently re-derives the raw lineage rows and validates them
(`fetch_autonomous_daily_operation_running_transitions_raw` +
`validate_autonomous_daily_operation_run_lineage`) and asserts the lineage contains exactly the two
fixture run IDs.

`PeSnapshot`'s scope authority is that same validated full run lineage — never a caller-supplied
single `run_id` — so every lineage-scoped count (`run`, dispatch-claim, strategy-evaluation, outbox,
inbox) sums across every run in the lineage. The snapshot additionally records global totals for
`runs`, `sys_autonomous_daily_bar_dispatches`, `strategy_signal_evaluations`, `oms_outbox`,
`oms_inbox`, `sys_autonomous_daily_operation_events`, and `sys_autonomous_session_events`, so a GET
route that created an unrelated row under a newly generated identity could not silently escape the
operation-scoped counts.

The test asserts the pre-route snapshot itself observes both lineage runs (`validated_lineage_run_count
== 2`) and carries run A's earlier fill/ack inbox evidence, then captures one full snapshot before
and after calling all five read routes in sequence against the same `AppState`:

```text
GET /api/v1/autonomous/daily-operation
GET /api/v1/autonomous/daily-operations
GET /api/v1/autonomous/readiness
GET /api/v1/autonomous/paper-status
GET /api/v1/system/preflight
```

Every delta, across every lineage-scoped and global field, is zero. This is consistent with, and
does not re-derive, the accepted E4 structural proof (`b21`/`b22`/`b23`/`b24` in
`scenario_autonomous_daily_operation_api_01.rs`) that the E4 route module never references a
classifier/finalizer/coordinator/mutating-DB-helper symbol at all — E5's proof is the dynamic,
whole-pipeline complement of that static proof.

## 7. Integrated proof F — fail-soft API truth

Test: `e5_proof_f_fail_soft_api_truth`.

Proves the accepted E4 fail-soft distinctions remain intact when exercised together in one file
alongside E5's own integrated fixtures:

```text
no operation                        -> not_found
no DB pool                          -> backend_unavailable
required downstream count-read fail -> query_failed (known operation row retained, null counts)
invalid/contradictory run lineage   -> active + evidence_state=unavailable + null counts
                                        (unknown_run_lineage_unavailable blocker)
exact malformed market_date         -> HTTP 400, truth_state=invalid_request,
                                        fixed bounded message, no raw-input echo
```

The frozen E4 response contract (`docs/specs/autonomous_daily_paper_operations_01e4_read_only_daily_operation_api.md`
§3–§5) is not altered by this proof.

## 8. Complete focused regression matrix

Run one named binary at a time (`--include-ignored --test-threads=1`, real isolated port-5434 test
database):

```text
scenario_autonomous_daily_phase_e_closure_01                 6/6   (new, this patch)
scenario_autonomous_daily_coverage_anchor_and_run_lineage_01  41/41
scenario_autonomous_daily_outcome_classifier_and_finalization_01  67/67
scenario_autonomous_daily_outcome_coordinator_integration_01  16/16
scenario_autonomous_daily_operation_api_01                    50/50 (19 non-DB + 31 DB-backed)
scenario_autonomous_daily_session_coordinator_01              48/48
scenario_autonomous_daily_phase_d_integration_01               8/8
scenario_autonomous_completed_bar_task_01                     49/49
scenario_daily_data_readiness_start_gate_01                   20/20
scenario_autonomous_daily_operation_store_01                  26/26
scenario_autonomous_daily_operation_lifecycle_01              36/36
scenario_autonomous_daily_operation_data_evidence_01            9/9
scenario_signal_evaluation_journal_auton_no_signal_obs_01       7/7
scenario_autonomous_readiness_auton_truth01                   18/18
scenario_autonomous_paper_status_summary_01                   21/21
scenario_daemon_routes                                        84/84
scenario_route_contract_rt01                                    2/2
scenario_gui_daemon_contract_gate                              23/23
```

Every exact test-binary name the mission listed exists verbatim in this repo at this HEAD — no
substitution was required for any entry above.

## 9. Known completed-bar-driver baseline

`scenario_autonomous_completed_bar_driver_01` is run and recorded honestly, not silently skipped:

```text
current-head totals: 47 passed, 9 failed
failing tests: DispatchClaimUnresolved{status:"failed"} instead of DispatchCompleted, in the same
  9 tests E2A's own audit already identified and confirmed pre-existing and unrelated
  (`scenario_autonomous_daily_coverage_anchor_and_run_lineage_01.md` §11: "47/56 pass against
  current HEAD, 3591064a, the original E2A commit, and its closure repair alike")
comparison against the accepted pre-E2A baseline (3591064a): identical 9 failures, identical 47
  passes — `git diff --stat` against 3591064a for this file remains empty; E5 touches no production
  seam in this file's driver path either
new Phase E failures: 0
```

## 10. Known limitations

- No pagination cursor/offset exists on the E4 routes — unchanged, frozen by the E1/E4 contracts.
- `scenario_autonomous_completed_bar_driver_01`'s 9 pre-existing failures remain open, tracked since
  before E2A; out of scope for E5 (§9 above).
- No unattended-soak evidence is produced by this patch. No GUI projection exists. No runbook
  correction is performed. All three remain Phase F's job.
- This document does not claim, and no test in this bundle proves, trading profitability or P&L
  accuracy — Phase E closure is a durable-outcome-truth and read-model-correctness proof only.
- Proof B's two-run lineage is built by direct `mqk_db` transition calls (mirroring the accepted
  E2A `h01` fixture), not by a live interrupted execution loop — consistent with every other
  recovery-lineage proof already accepted in this bundle (E2A §10, E3 §13's own documented
  limitation for the symmetric matching-runtime case).

## 11. Boundaries

```text
Phase F boundary:   GUI projection, runbook correction, and supervised soak-evidence preparation —
                     not started, not authorized by this patch.
Phase G boundary:   closure audit and final ledger reconciliation — not started, not authorized.
Bundle 4 boundary:  not started, not authorized.
Soak boundary:      the 10–20-session unattended autonomous paper soak has not started and is not
                     authorized by this patch.
Live-capital boundary: live trading is not ready and is not authorized by this patch.
```

## 12. E5 does not reopen E1–E4

No integrated proof in this patch exposed a production correctness defect. Every assertion in
`scenario_autonomous_daily_phase_e_closure_01.rs` passed against the accepted E1–E4 production code
unmodified. Per the mission's own binding condition, E1–E4 behavior is not reopened or redesigned by
this patch.

## 13. E5 deterministic proof and closure-guard repair (this patch)

This repair closes four proof defects in the original E5 patch. **No production Rust file, no
migration, and no GUI file is touched** — only the closure test file, this spec, the closure guard,
the ledger, and both READMEs.

1. **Deterministic notification observation.** The prior test used `tokio::time::sleep` fixed
   delays as the sole mechanism for letting the loopback sink's `.await`ed POST become observable
   before a notification-count assertion. Every `tokio::time::sleep` call is removed. The sink is
   now a `PeAlertRecorder`: it stores each received payload, then publishes the new count on a
   `tokio::sync::watch` channel — a deterministic completion signal, not a delay. `wait_for_alert_count`
   polls that channel (`changed()`/`borrow()`) until the expected count is observed, bounded by a
   `tokio::time::timeout` that exists purely as deadlock/failure protection and never makes an
   assertion pass. Every zero-new-notification (replay) proof instead reads the recorder's count
   immediately after its coordinator tick — valid because this patch's audit confirmed
   `notify_run_status`/`notify_critical_alert` are `.await`ed directly inside
   `run_durable_session_controller_tick`'s outcome match arm and never spawned, so nothing can
   arrive after the tick's own `.await` returns.
2. **Complete read-only snapshot.** `PeSnapshot`/`pe_snapshot` no longer accept a caller-supplied
   single `run_id`. They now derive the operation's full validated run lineage via
   `fetch_autonomous_daily_operation_running_transitions_raw` +
   `validate_autonomous_daily_operation_run_lineage` (the same pair
   `fetch_and_validate_autonomous_daily_operation_run_lineage` composes), fail the test outright if
   that lineage is contradictory, and sum every lineage-scoped count (run-table rows,
   strategy-evaluation rows, outbox rows, inbox rows) across every run in the lineage. Dispatch
   claims remain scoped to the operation directly (their native schema shape). The snapshot also
   now carries global totals for `runs`, `sys_autonomous_daily_bar_dispatches`,
   `strategy_signal_evaluations`, `oms_outbox`, `oms_inbox`,
   `sys_autonomous_daily_operation_events`, and `sys_autonomous_session_events`, closing the gap
   where a GET route could create an unrelated row under a newly generated identity and escape an
   operation-scoped-only snapshot. Proof E now runs against the two-run activity fixture (§6 above),
   asserting the validated lineage contains exactly two run IDs and that the snapshot's
   lineage-scoped counts include both runs before taking its before/after comparison.
3. **Committed patch-scope closure guard.** Checks `[23]`/`[24]` previously ran
   `git diff --name-only HEAD` / `--cached` only — commands that see nothing once E5's work is
   committed, silently letting a committed production/migration/GUI change escape the guard
   entirely. The guard now fixes the accepted E4 head
   (`11664945e90a582e6984f0eab66cf89690120769`) as the Phase E closure base, requires
   `git merge-base --is-ancestor` to hold, and inspects
   `git diff --name-only 11664945e90a582e6984f0eab66cf89690120769..HEAD` as the committed-range
   authority for production Rust (`core-rs/**/src/**.rs`), migrations
   (`core-rs/crates/mqk-db/migrations/**`), and GUI (`core-rs/mqk-gui/**`) files. The staged and
   unstaged working tree is still inspected separately (checks now split, still numbered
   `[23]`/`[24]`) so an in-progress, uncommitted violation is caught before it is committed. The
   guard reports the exact offending paths for every list it inspects.
4. **Closure-guard proof-quality strengthening.** New checks `[25]`–`[29]` require: no
   `tokio::time::sleep` anywhere in the closure test file; a deterministic alert-recorder/
   wait-for-count helper backed by a `tokio::sync::watch` channel; `pe_snapshot` deriving lineage
   via the raw-fetch/validate pair rather than a caller-supplied `run_id`; `PeSnapshot` carrying all
   seven global-total fields; and Proof E using the two-run activity fixture. Every existing E1–E4
   invariant check (`[1]`–`[22]`) is unchanged.

This repair does not reopen or redesign any E1–E4 behavior, and does not itself close Phase E —
independent ChatGPT/operator acceptance is still required.

```text
E5 deterministic proof repair: IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE
PHASE E:  IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE
PHASE F:  NOT STARTED
PHASE G:  NOT STARTED
BUNDLE 3: OPEN
BUNDLE 4: NOT STARTED
SOAK:     NOT STARTED
LIVE CAPITAL: NOT READY
```
