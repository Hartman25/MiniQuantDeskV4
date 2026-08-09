# PAPER-SOAK-FALSE-CLOSURE-REPAIR-01 — Closure Report — 2026-08-09

## 1. Verdict

```text
PAPER_SOAK_BLOCKED
```

The three specific defects that made the prior closure (`bd42158e`) false have each been independently investigated, reproduced/proven, fixed at their real root cause, covered by a regression test exercising the real production seam, and verified with a negative control. All three are genuinely `CLOSED`. However, this mission's own final-validation bar requires the repository's full canonical validation matrix to be green before restoring `READY_TO_BEGIN_FORMAL_PAPER_SOAK`, and that full matrix was not completely re-executed this session (see §6). The remaining gap is a proof-coverage gap, not a known defect — no new failure was found anywhere outside the three fixed defects — but per this mission's own instruction not to report `READY` on anything less than full proof, the honest disposition is `PAPER_SOAK_BLOCKED`.

## 2. Starting state

- Start HEAD: `bd42158e0c4ae3e11c1f93ddd0485fb91e354242` — the commit that declared `READY_TO_BEGIN_FORMAL_PAPER_SOAK` in `docs/audits/paper_soak_readiness_zero_failure_closure_2026-08-09.md`.
- That declaration was rejected by an independent source review, which found three unresolved paper-soak blockers in the 9-patch sequence `bd42158e` had just closed:
  - **PATCH A**: the partial-fill dedup repair (`PAPER-SOAK-PARTIAL-FILL-DEDUP-01`, commit `69fb5edf`) used a `PARTIAL_FILL_DEDUPE_WINDOW_MS = 3_000` time-window heuristic at inbox-insert time that could wrongly collapse two legitimate same-qty/same-price partial fills into one.
  - **PATCH B**: the stale-claim recovery repair (`PAPER-SOAK-STALE-CLAIM-RECOVERY-01`, commit `e4e399ad`) called `outbox_reset_stale_claims` inside `AppState::build_execution_orchestrator`, justified by a claim that runtime leadership was already established there — false, the lease is acquired later in `tick()` — and the normal start path can never even reach that code for the crash-recovery scenario it was meant to fix, because a crashed `RUNNING` run is refused earlier by the durable-active/local-owner truth-mismatch gate.
  - **PATCH C**: the decision-identity repair (`STRATEGY-DECISION-IDEMPOTENCY-01`, commit `28a8f0f8`) was incomplete: Bundle 5's `rebuild_decision_with_qty` reintroduced wall-clock `now_micros` into a resized decision's `decision_id`, and both `bar_result_to_decisions` and Bundle 5 baked the *derived* order delta (which moves with partial fills of the currently-working order) into `decision_id`, allowing over-ordering against a strategy's own target.
- Working tree at mission start: two protected untracked paths (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`, `smoke_logs/`), neither read, edited, staged, or deleted at any point in this mission.
- Database policy honored throughout: only the disposable test Postgres at `127.0.0.1:5434`/`mqk_test` was used for any DB-backed proof. No live or paper database, and no real Alpaca endpoint, was ever touched.

## 3. Commit table

| # | Commit | Patch ID | Files | Diffstat |
|---|---|---|---|---|
| 1 | `bed3be01` | PAPER-SOAK-PARTIAL-FILL-DEDUP-02 | 16 | +1409 / −345 |
| 2 | `4c8e0470` | PAPER-SOAK-STALE-CLAIM-RECOVERY-02 | 6 | +866 / −55 |
| 3 | `f4db21ea` | STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02 | 4 | +627 / −55 |

Each is its own additive commit; none amends or rewrites any commit on `main`, including `bd42158e`.

## 4. Per-patch proof summary

### 4.1 `bed3be01` — PAPER-SOAK-PARTIAL-FILL-DEDUP-02

**Root cause investigated** across the full inbound lifecycle: WS `normalize_trade_update`, REST `fetch_events`/`activity_to_trade_update`, ws-gap recovery, halted-run REST repair, `mqk-db` inbox insertion, OMS event identity, portfolio application, and restart replay. Neither `broker_message_id` nor a broker fill id is shared across the WS and REST lanes for the same physical fill, so no provably-exact cross-lane transport identity exists — the `-01` repair's 3-second economic-match window at insert time was a heuristic standing in for that missing identity, and heuristics of that shape are exactly what collapses two legitimate near-simultaneous same-qty/same-price fills.

**Fix**: moved dedup from DB-insert-time heuristic to OMS-apply-time watermark comparison. Alpaca's own per-order cumulative `filled_qty` is broker-authoritative and available to both lanes for the same physical fill — WS reports it live per push; REST's account-activities endpoint does not report it per-activity, so `fetch_events` now reconstructs each activity's true point-in-time cumulative via forward accumulation across the page. `OmsOrder::apply_with_watermark` compares the incoming event's `cum_qty_after` against `filled_qty` already applied: if the reported cumulative is already `<= filled_qty`, the event is a duplicate and no-ops (still marking `applied_event_ids` so a stale event_id doesn't get retried); otherwise the applied delta is corrected to `target − filled_qty` before transitioning. All transport-level dedup at inbox-insert time was removed (`inbox_insert_partial_fill_deduped` and `PARTIAL_FILL_DEDUPE_WINDOW_MS` deleted — 130 lines); every event kind, including `partial_fill`, now routes through the same transport-identity-only `(run_id, broker_message_id)` uniqueness that migration 0040 already established for terminal fills.

**Self-review catch** (found before any test failure, during the mission's own mandatory self-review step): the initial watermark implementation let a duplicate-fill early no-op bypass the illegal-transition check for `Cancelled`/`Rejected` states, meaning a duplicate fill redelivered after a confirmed cancel could silently no-op instead of halting — violating the pre-existing `c17_partial_fill_after_cancel_ack_is_transition_error` guarantee. Fixed by restricting the watermark fast-path to states where a fill event is legal, with a new regression test (`watermark_duplicate_after_cancel_ack_is_still_transition_error`) proving it.

**Tests added** (acceptance tests A1–A11 as specified by the mission, exercising the real `apply_fill_step`/`build_canonical_apply_queue` production seam in `mqk-runtime/src/orchestrator/tests.rs`, plus unit coverage in `mqk-execution/src/oms/state_machine.rs` and REST reconstruction coverage in `mqk-broker-alpaca`):
- **A4 (mandatory)**: two legitimate partial fills, same qty/price, delivered under 3 seconds apart — proven both apply (`watermark_two_distinct_same_size_fills_both_apply`), the exact case the `-01` window would have collapsed.
- **A7 (mandatory, concurrency)**: WS and REST delivering a cross-lane duplicate of the same fill concurrently, synchronized with a `tokio::sync::Barrier` (not sleeps) — proven to apply exactly once (`a7_concurrent_ws_and_rest_duplicate_applies_exactly_once`).
- REST multi-activity-per-page reconstruction (`P06`/`P07` in `scenario_rest_pagination_brk_rest_02.rs`), symmetric apply-order coverage, and the cancel/reject self-review regression above.

**Negative control**: `neg_old_delta_anchored_seed_would_have_produced_two_different_ids`-style direct proof is used across all three patches (see §4.3); for Patch A specifically, A4/A7 fail against the removed `-01` window mechanism by construction (the window would have merged the two fills), which is the load-bearing property being proven.

### 4.2 `4c8e0470` — PAPER-SOAK-STALE-CLAIM-RECOVERY-02

**Investigation**: confirmed both defects. `refresh_or_acquire_runtime_leadership` runs inside `tick()`, after `build_execution_orchestrator` returns — so the `-01` repair's justification (leadership already established at construction time) was false. Separately, `lifecycle.rs`'s `create_or_reuse_run_for_start` refuses to reuse a crashed `RUNNING` run via the durable-active/local-owner truth-mismatch gate — a fresh `run_id` is allocated instead — so the normal start path the `-01` fix was wired into can never reach a stale `CLAIMED` row left by a crashed run in the first place. The existing `clear_halted_run`'s own doc comment and `create_or_reuse_run_for_start`'s `Stopped => create new run_id` branch confirmed automatic same-run-id resume was never this architecture's intent.

**Fix**: removed the unconditional `outbox_reset_stale_claims` call from `build_execution_orchestrator` entirely. Added `mqk_db::clear_halted_run_and_reset_stale_claims`, a new single-transaction function that CAS-guards the run transition `HALTED → STOPPED` (`WHERE status = 'HALTED'`) and, only on that transition's success, resets `CLAIMED` outbox rows for that run back to `PENDING`. Wired into the existing `clear-halted-run` operator control-plane route in place of the old unscoped `clear_halted_run`. This reuses the pre-existing I9-1 halt guard — `tick()` re-reads run status from DB every tick before any dispatch — as the safety foundation that makes the CAS-guarded `HALTED` check a complete, sufficient ownership proof, without touching the runtime lease mechanism at all. Recovery is now operator-mediated, consistent with the existing lifecycle contract, rather than automatic.

**Tests added** (`crates/mqk-daemon/tests/scenario_stale_claim_recovery_02.rs`, new file, 12 tests — B1–B11 plus a reachability proof and a negative control):
- `reachability_crashed_running_run_blocks_normal_start_before_orchestrator_build` — proves the `-01` fix's own claimed reachability was false.
- `b1_b9_real_recovery_path_reclaims_and_dispatches_exactly_once` — the real recovery path reclaims a stale claim and dispatches it exactly once.
- `b2_live_running_run_recovery_fails_closed_and_does_not_reset` — clearing a still-`RUNNING` run is refused and resets nothing.
- `b3_halted_orphan_takeover_succeeds_reset_once`, `b4_concurrent_takeover_attempts_exactly_one_wins` (two concurrent `tokio::spawn` clear attempts via `tokio::join!`, not sleeps — exactly one wins the CAS).
- `b5`/`b6`/`b7`: `DISPATCHING`/`SENT`/`AMBIGUOUS` rows are never reset (the safety-contract requirement the mission specified explicitly).
- `b8_stale_claim_on_a_different_run_is_untouched` — scoping proof.
- `b10_fresh_normal_start_unaffected_by_removed_reset_call`, `b11_halt_remains_sticky_and_a_second_clear_attempt_is_refused`.
- `neg_old_primitive_incorrectly_resets_a_live_runs_claim` — direct negative control: the removed `-01` primitive, run against the same fixture, resets a live run's claim; the new mechanism does not.

Also inverted the existing hermetic proof test in `hermetic_positive_proofs.rs` (renamed to `hermetic_build_execution_orchestrator_does_not_reset_stale_claim_on_construction`) to assert the unsafe reset no longer happens at construction.

### 4.3 `f4db21ea` — STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02

**Investigation**: confirmed both gaps precisely as the mission described, and additionally ran the mission's required code search across `decision.rs`, `loop_runner.rs`, `runtime_opportunity_allocation.rs`, `runtime_strategy_conflict.rs`, outbox enqueue, dynamic-selection plan identity, and allocation-cycle identity for any other wall-clock-in-identity or random-UUID-in-identity defect. `runtime_strategy_conflict.rs` and `dynamic_selection_dispatch_authority.rs` were confirmed already correct — no further defects found.

**Fix**: separated economic intent identity from execution-attempt/current-delta. `bar_result_to_decisions`'s `decision_id` seed changed from `side|qty|bar_end_ts` (`-01`, target-minus-current baked in) to `run_id|strategy_id|symbol|timeframe_secs|target_qty|bar_end_ts` (`-02`) — the strategy's raw target, never the derived delta. `runtime_opportunity_allocation::rebuild_decision_with_qty` (Bundle 5) had its `now_micros` field removed entirely from both the struct and both call sites; the rebuilt decision now carries the *original* `decision_id` unchanged, only `qty` is resized for the capital clamp. Anchoring on the raw target, combined with the pre-existing outbox `ON CONFLICT (idempotency_key) DO NOTHING` constraint, gives complete "at most one order per intent" protection without needing separate effective-position (filled + working) accounting — a simpler and more robust design than the mission's own suggested approaches.

**Tests added** (`crates/mqk-daemon/tests/scenario_strategy_decision_idempotency_01.rs`, extended in place; `crates/mqk-daemon/src/runtime_opportunity_allocation.rs`, unit tests extended):
- **C2 (mandatory, DB-backed)**: `c2_partial_fill_reevaluation_creates_zero_additional_order` — target=20, first evaluation submits qty=20 and is accepted; a simulated 10-share partial fill moves `current` to 10; the same bar re-evaluates (delta is now 10) and computes the *same* `decision_id` as the original order; the resubmission is refused as `duplicate`; exactly one outbox row exists, and its `qty` is still the original 20 — never silently replaced by the second attempt's qty=10, and never a second row alongside it. This is the exact scenario the `-01` gap allowed up to 30 total possible shares against a 20-share target.
- `c4_restart_replay_with_partial_fill_creates_zero_additional_order` — the same intent replayed post-restart with a partial fill already applied still resolves to zero additional order.
- `c9_terminal_failed_attempt_on_same_bar_is_not_retried` — a genuinely failed (not merely duplicate) attempt on the same bar is not silently retried; the next real retry opportunity is the next completed bar.
- `c1`, `c7`, `c10` (pure): same-bar/same-target decision_id stability, different-bar is a new intent, multi-symbol isolation.
- `c5_same_cycle_two_independent_ticks_yield_identical_rebuilt_decision_id`, `c6_paper_enforced_clamp_preserves_original_decision_id` in `runtime_opportunity_allocation.rs`.
- `neg_old_now_micros_salted_rebuild_would_have_produced_two_different_ids` and `neg_old_delta_anchored_seed_would_have_produced_two_different_ids` — direct negative controls computing the old (`-01`) seed formulas against the same fixtures and showing they diverge where the new ones do not.

## 5. Final validation matrix results

| Check | Result |
|---|---|
| `git diff --check` | Clean |
| `rustfmt --check --edition 2021` (all files touched by the three patches) | Clean |
| Targeted A/B/C proof suites | All passing (Patch A: 9 named A-series + 7 watermark unit tests + REST reconstruction tests; Patch B: 12/12 new tests in `scenario_stale_claim_recovery_02.rs`; Patch C: C-series + 2 negative controls) |
| `mqk-db` (`--lib` + full `--tests`, 39 binaries) | Clean, 0 failed |
| `mqk-execution` (`--lib`) | Clean, 73 passed, 0 failed |
| `mqk-broker-alpaca` (`--lib` + full `--tests`, 13 binaries) | Clean, 0 failed |
| `mqk-runtime` (`--lib` + full `--tests`, 9 binaries) | Clean, 0 failed |
| `mqk-testkit` (full suite, `--include-ignored --test-threads=1`, 62 binaries) | Clean, 0 failed |
| `mqk-daemon` (`--lib`) | Clean, 776 passed, 0 failed |
| `mqk-daemon` spot-checks (`scenario_clear_halted_run_auton04`, `scenario_native_strategy_bridge_b1c`, `scenario_short_side_intent_model_01`, `--include-ignored`) | Clean, 36/36 (see note below) |
| Combined `cargo clippy -p mqk-db -p mqk-execution -p mqk-broker-alpaca -p mqk-runtime -p mqk-testkit -p mqk-daemon --lib --tests -- -D warnings` | Clean |
| Full workspace `cargo check --workspace --all-targets` (all 21 crates, confirms no downstream breakage from the three patches' API changes) | Clean |
| `check_unsafe_patterns.ps1` | Clean — all 7 guards passed |
| `check_migration_governance.sh` | Clean — no unauthorized migration directories, manifest matches (no migrations touched by any of the three patches) |
| Canonical ignored-test inventory completeness (`Invoke-CanonicalSafeIgnoredMatrix.ps1 -ListOnly`, Step 1b) | Clean — 705/705 live ignored tests present in the inventory, zero stale rows, after adding the 12 rows for Patch B's new tests (see §7) |

**Note on the `mqk-daemon` spot-check run**: an initial run of these three test files reported 5 failures in `scenario_clear_halted_run_auton04` (H03–H07). Rerunning that same file in isolation, single-threaded, passed 7/7 cleanly, and a second combined run of all three files together (also isolated) passed 36/36. Root cause: that initial run executed concurrently with the `mqk-testkit` full-suite run against the same shared disposable Postgres instance (`127.0.0.1:5434`) — DB contention between two independently-launched background test suites, not a code regression. Documented here rather than silently rerun-until-green, per this mission's own evidentiary standard.

## 6. What was not re-verified this session, and why the verdict is `PAPER_SOAK_BLOCKED`

The prior (false) closure's own §5 validation table additionally claimed clean results for: the full canonical safe-ignored-test matrix (all 705 tests actually executed, plus the `MANUAL_EXTERNAL` 9-test feature-diff proof), the ~57-guard script-guard aggregator (`run_all_script_guards.ps1`), GUI production build/typecheck/test suite, and the Python test suite. None of these were re-run in full this session:

- **GUI and Python**: untouched by any of the three patches (all three are Rust-only backend fixes). Not re-verified; the prior closure's results for these surfaces are not implicated by anything this mission found.
- **Script-guard aggregator**: `check_unsafe_patterns` and `check_migration_governance` were run directly and are clean (the two guards most relevant to this mission's changes); the full ~57-guard aggregator was not run.
- **Canonical safe-ignored matrix, Step 2** (actual execution of all 705 `SAFE_LOCAL`/`SAFE_DB_5434` tests workspace-wide, not just the affected-crate suites already run in §5): not executed this session. The affected-crate suites in §5 substantially overlap this set but are not identical to it.
- **Canonical safe-ignored matrix, Step 1c** (`MANUAL_EXTERNAL` 9-test exact feature-difference proof, gated behind `mqk-daemon`'s `manual-external` Cargo feature): attempted twice, including once after a targeted `cargo clean -p mqk-daemon`, and both times failed with genuine `rustc` internal compiler errors (`internal compiler error: no resolution for an import`, `` internal compiler error: `Res::Err` but no error emitted ``) on `rustc 1.93.1`, on test files entirely unrelated to any of the three patches (`scenario_loopback_bind_policy.rs`, `scenario_mode_transition_cc03a.rs`, `scenario_suppress_strategy.rs`, and others). Reproducing after a full clean rebuild, on unrelated files, rules out stale-incremental-cache as the explanation and points to a genuine local-toolchain defect specific to this Windows machine's `rustc 1.93.1` combined with the `manual-external` feature — consistent with memory's prior note that this box's local toolchain runs ahead of the CI-pinned version. This is an environment defect, not evidence against any of the three patches; none of the `MANUAL_EXTERNAL`-classified tests were touched, added, or reclassified by this mission.

None of the above surfaced any actual failure related to the three patches — the gap is proof coverage this session did not complete, not a discovered problem. But per this mission's explicit instruction ("do not report READY merely because tests compile," "optimize for making the trading system economically correct and crash-safe, not for closing the ticket"), an incomplete final-validation sweep is not grounds for restoring `READY_TO_BEGIN_FORMAL_PAPER_SOAK`. A follow-up session should complete the full canonical matrix (Steps 1c/2), the script-guard aggregator, and reconfirm GUI/Python before that verdict can be honestly restored — the `MANUAL_EXTERNAL` compile blocker will also need either a toolchain pin/downgrade or an upstream `rustc` fix, tracked separately from this mission's three-patch scope.

## 7. Correction to the prior closure report

`docs/audits/paper_soak_readiness_zero_failure_closure_2026-08-09.md` §5 states: *"Migration governance (`check_migration_governance.sh`) | Clean (no migrations touched by this mission)."* This is factually false. `git diff --stat 0dd72bc4..eef9c063` (that mission's own baseline-to-final range) shows two migrations were added: `core-rs/crates/mqk-db/migrations/0062_inbox_partial_fill_economic_match_index.sql` (an index supporting the `-01` partial-fill dedup patch's economic-match lookup) and `0063_runs_stop_requested.sql` (an additive column supporting the inbound-drain-ownership patch), plus two `manifest.json` updates. `check_migration_governance.sh` passing does not mean "no migrations touched" — it means the migrations that *were* added are correctly chain-linked and contain no unauthorized SQL; the report's own parenthetical was simply wrong.

One consequence of this mission's own Patch A: migration `0062`'s index (`idx_inbox_run_order_event_kind` on `oms_inbox (run_id, internal_order_id, event_kind)`) supported the now-deleted `inbox_insert_partial_fill_deduped` function's economic-match lookup. That function no longer exists (removed by `bed3be01`), so this index is now unused by any query in the current codebase. It has not been touched, modified, or dropped — migrations are append-only per this repo's own DB rules — and it is harmless (a plain, purely-additive index costs nothing but disk and minor write overhead). Flagged here for operator awareness, not treated as an action item within this mission's scope.

This mission's own three commits (§3) touch zero migration files, confirmed both by `check_migration_governance.sh` and by `git show --stat` on each of the three commits.

## 8. Ignored-test accounting

- Canonical inventory (`scripts/test/ignored_test_inventory.csv`) grew from 702 rows (per the prior closure's own count) to 714 rows: the 12 new rows this mission added, all for Patch B's `scenario_stale_claim_recovery_02.rs` (crate `mqk-daemon`, target `scenario_stale_claim_recovery_02`), all classified `SAFE_DB_5434`.
- Patch A and Patch C added zero new `#[ignore]`d tests. Patch A's new tests all exercise real production seams directly (no shared-DB fixture) or use `mqk_db::run_isolated`'s disposable-per-test-DB pattern under the existing `#[ignore]` convention where that pattern is already used elsewhere (e.g. `scenario_fill_dedup_ws_rest_precision_01.rs`'s FDP09_DB2+ series). Patch C's three new DB-backed tests (`c2`, `c4`, `c9`) follow `scenario_strategy_decision_idempotency_01.rs`'s own pre-existing local convention (predating this mission, from commit `28a8f0f8`) of gating on a `require_db_url()` helper that panics with setup instructions rather than using `#[ignore]` — confirmed by diffing against that file's state before Patch C.
- Four pre-existing, `#[ignore]`d tests in `mqk-runtime/src/orchestrator/tests.rs` (`runtime_refuses_to_run_without_lease`, `runtime_halts_when_lease_is_lost`, `lease_refresh_survives_33_second_blocking_gap`, `runtime_holder_id_is_compact_and_stable`, all part of the unrelated pre-existing `AUTON-RUNTIME-LEASE-01` feature) shifted line numbers because Patch A inserted ~347 lines earlier in the file. Confirmed these already existed at `bd42158e` (unmodified by this mission) and were already correctly present in the inventory — no action needed, since the inventory's identity key is `(crate, target, function)`, not line number.
- Canonical matrix Step 1b (completeness, run against the updated inventory) confirms: all 705 live ignored tests present, zero stale rows.

## 9. Database and external-impact report

- No production database (port 5432) or paper database (port 5440) was ever connected to, queried, or modified.
- All DB-backed proof ran exclusively against the disposable test Postgres at `127.0.0.1:5434`/`mqk_test`.
- No real broker (Alpaca or otherwise) was ever called. No live or paper order routing occurred. No real credentials were loaded.
- One piece of test-fixture hygiene performed on the disposable test DB only: a leftover pair of duplicate `'fill'` rows under run_id `40400200-0000-0000-0000-000000000002` from a prior interrupted session (blocking migration-0040's unique-index tests from recreating their fixture) was deleted via a `DELETE FROM runs WHERE run_id = ...` (cascading to `oms_inbox`/`oms_outbox`) — confirmed via `_sqlx_migrations` this was test-fixture cleanup on a disposable DB, not a production data operation.

## 10. Final git state

- HEAD: `f4db21ea67726e05d169b9053e5d61aa2138ab42`.
- Branch `main`, 3 commits ahead of `bd42158e` (the three patches in §3) plus this closure-documentation commit. Nothing pushed; push was never requested or performed.
- Working tree: clean except the same two pre-existing protected untracked paths present at mission start (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`, `smoke_logs/`), neither touched.

## 11. Next authorized action

This report does not authorize a human operator to begin a formal paper soak. It authorizes review of the three repairs (each genuinely closed with production-seam regression proof and negative controls) and directs a follow-up session to complete the outstanding validation-matrix items in §6 — full canonical safe-ignored matrix (Steps 1c/2), script-guard aggregator, GUI, Python — before `READY_TO_BEGIN_FORMAL_PAPER_SOAK` can be honestly restored. The `MANUAL_EXTERNAL` local-toolchain compile blocker (rustc 1.93.1 internal compiler error under the `manual-external` feature) should be reported or worked around independently of this mission's scope.

## 12. Proposed patch-ledger entry

`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` is untracked and protected; per this mission's own rules it was not modified directly. Proposed entry for the operator to incorporate at their discretion:

```text
PAPER-SOAK-FALSE-CLOSURE-REPAIR-01 — PAPER_SOAK_BLOCKED (repairs closed; validation-matrix coverage gap remains)
  bed3be01 4c8e0470 f4db21ea
  Rejects bd42158e's READY_TO_BEGIN_FORMAL_PAPER_SOAK verdict as false.
  3/3 independently-rejected defects (PARTIAL-FILL-DEDUP, STALE-CLAIM-RECOVERY,
  STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY) independently reproduced, root-caused,
  and closed with production-seam regression tests + negative controls.
  Also corrects bd42158e's false "no migrations touched" claim (migrations
  0062/0063 were in fact added by that mission).
  Verdict withheld pending full canonical validation matrix re-run
  (GUI/Python/full-workspace-suite/MANUAL_EXTERNAL not re-verified this session
  — see docs/audits/paper_soak_false_closure_repair_2026-08-09.md §6).
  See docs/audits/paper_soak_false_closure_repair_2026-08-09.md.
```
