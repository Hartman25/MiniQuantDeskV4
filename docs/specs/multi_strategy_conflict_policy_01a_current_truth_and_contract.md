# MULTI-STRATEGY-CONFLICT-POLICY-01 — source audit

Answers the 10 required audit questions from the exact current source, before
any runtime wiring. Verified against `main` at `a0852c2f6ffd2343c9b6740728abd5b7889bcb15`.

## 1. Where is the one current authoritative insertion point before Bundle 5?

`core-rs/crates/mqk-daemon/src/state/loop_runner.rs`, inside `spawn_execution_loop`'s
per-tick block. `all_decisions: Vec<PendingDecisionWithBarFacts>` is fully
built by the `for (assignment, mut bar_result, bar_facts) in dispatch_results`
loop (each symbol's decisions pushed via `all_decisions.extend(...)`), and the
very next statement after that loop closes is the single per-tick call:

```rust
let allocation_outcome = crate::runtime_opportunity_allocation::gather_and_apply(
    &state_arc, run_id, now_micros, market_date_today, dispatch_timeframe,
    all_decisions, &current_positions,
).await;
```

That is the one seam: `all_decisions` is complete and no submission has
happened yet. Bundle 6's `resolve_strategy_conflicts` call goes immediately
before this `gather_and_apply` call, consuming `all_decisions` and producing
the vector `gather_and_apply` receives instead.

## 2. What exact current ordering must remain unchanged?

Per-symbol loop (dispatch, symbol-mismatch guard, cap #2 clamp, dry-run
diagnostics, `bar_result_to_decisions`) → **[Bundle 6 inserts here]** →
`runtime_opportunity_allocation::gather_and_apply` (Bundle 5) → `for decision
in allocation_outcome.decisions` submission loop, which contains cap #6
(`max_new_orders_per_tick_reason`) immediately before
`submit_internal_strategy_decision`. Bundle 6 must not move Bundle 5, must
not move cap #6, and must not touch the submission loop itself — it only
narrows the vector Bundle 5 receives.

## 3. What current types can be reused without widening stable contracts?

`crate::runtime_opportunity_allocation::PendingDecisionWithBarFacts` (pairs
`InternalStrategyDecision` with `Option<EvaluatedBarFacts>`) is the exact
input/output shape Bundle 6 consumes and produces — no new decision-carrying
type is needed. `crate::decision::InternalStrategyDecision` and
`crate::state::EvaluatedBarFacts` are read-only inputs; Bundle 6 never
constructs a new `InternalStrategyDecision` (exact-original invariant), it
only selects which of the exact input structs (by move, not by
reconstruction) survive into the output vector.

## 4. Why must Bundle 6 not change `NativeStrategyBootstrap` in this bundle?

`NativeStrategyBootstrap::bootstrap` (`core-rs/crates/mqk-runtime/src/native_strategy.rs`)
consumes only `fleet_ids[0]` — "Single-strategy Tier A policy: consume only
the first fleet entry" — and `StrategyHost::register` (`mqk-strategy/src/host.rs`)
returns `StrategyHostError::MultiStrategyNotAllowed` on a second registration.
There is exactly one strategy host for the entire run, dispatched sequentially
across every configured symbol. `docs/design/native_multi_symbol_dispatch.md`
(§4.4, §5 "Honest open gap") confirms this is unmitigated beyond
`retain_targets_matching_symbol`: only the one symbol matching the host's
fixed-at-construction `MQK_STRATEGY_SYMBOL` target ever produces a non-empty
target list; every other configured symbol's targets are dropped before
`bar_result_to_decisions` ever runs. Building real multi-strategy competition
requires per-`(symbol, strategy_id)` host instantiation — that is Bundle 7's
dynamic strategy-symbol selection work, explicitly deferred. Changing
`NativeStrategyBootstrap` now would be scope creep into Bundle 7 and is
forbidden by the task's scope decision; Bundle 6 must be provably correct
against the *possibility* of multiple same-symbol candidates without
requiring the runtime to actually produce them today.

## 5. What exact candidate identity is stable across later loop ticks?

Per candidate: normalized `symbol` (trim, uppercase not required — compared
as-is since `InternalStrategyDecision.symbol` is already the canonical
ticker), `strategy_id`, `timeframe` (from `EvaluatedBarFacts.timeframe`, must
match the cycle timeframe), and the exact completed-bar identity
`bar_end_ts` (from `EvaluatedBarFacts`, mirroring Bundle 5's own
`compute_cycle_id` tuple `(symbol, strategy_id, bar_end_ts)`). `decision_id`
is excluded from identity — `bar_result_to_decisions` mints it from
`now_micros` (loop-tick wall clock), so two decisions from the same economic
cycle re-processed on a later tick have different `decision_id`s but the same
`(symbol, strategy_id, bar_end_ts, side, proposed_target_qty)` economic
content. Cycle-level identity (Bundle 6's own `cycle_id`) is the sorted set of
per-symbol-group resolved-candidate tuples, mirroring Bundle 5's Phase B
repair exactly (see ledger `RUNTIME-OPPORTUNITY-ALLOCATION-01-READINESS-AND-AUTHORITY-REPAIR-01`).

## 6. How does the policy preserve exits without permitting oversell?

Every candidate's `proposed_target_qty` is computed once
(`buy: current_qty + decision.qty`, `sell: current_qty - decision.qty`,
checked arithmetic). A candidate is risk-reducing iff
`proposed_target_qty < current_qty`. When one or more valid risk-reducing
candidates exist for a symbol, exactly one exact-original decision (the
smallest proposed target, canonical tie-break on ties) is passed through —
never combined, never summed. A single surviving sell decision's own `qty`
is untouched (exact-original invariant), so oversell protection is whatever
`bar_result_to_decisions`'s existing B5 short-sale guard already proved
upstream (delta must not exceed `current_qty`) — Bundle 6 adds no new
arithmetic to a sell's qty, it only chooses *which* one original sell survives
when more than one competes for the same symbol.

## 7. How will `off` and `shadow` modes prove zero economic behavior change?

`off` (default): `resolve_strategy_conflicts` returns the input
`Vec<PendingDecisionWithBarFacts>` unchanged, same order, no clone-and-rebuild
— proven by reference/pointer-stable unit tests (same `decision_id`s, same
order) and an integration test asserting the vector passed to
`gather_and_apply` is identical, by value, to the pre-Bundle-6 vector. `shadow`
returns the same unchanged vector to `gather_and_apply` and only additionally
computes (and best-effort persists) what *would* have happened — Bundle 5's
own `Shadow` mode is the direct precedent (`apply_runtime_opportunity_allocation`'s
`Shadow` arm: "Zero allocator-driven outbox changes: original buy decisions
pass through exactly as the strategy computed them"). Both are proven by the
same before/after-vector-equality test Bundle 5 used for its own off/shadow
proof.

## 8. Which API/GUI pattern should be reused for read-only evidence?

`core-rs/crates/mqk-daemon/src/routes/portfolio_allocation.rs` (three GET
routes: `/status`, `/plans`, `/plans/:plan_id`; closed `AllocationTruthState`
enum; `approved_for_live: false` hardcoded; `run_id` resolution via
`durable_portfolio::resolve_run`) and
`core-rs/mqk-gui/src/features/system/RuntimeOpportunityAllocationPanel.tsx`
(fail-closed parser in a sibling `.ts` file, `Panel` component, no button/
mutation, mounted in `SettingsScreen.tsx` behind a `systemStatusSections.ts`
feature-flag id) are the exact templates for Bundle 6's read-only conflict
routes and panel.

## 9. What existing tests/guards will break from the new insertion point?

`scripts/guards/check_runtime_opportunity_allocation_01.sh` check #11 ("Bundle
6 ... is not started -- no file path referencing it exists yet") fails as
soon as any Bundle 6 file is added. This check exists only because Bundle 6
had not started when Bundle 5 closed; now that Bundle 6 is explicitly
operator-authorized, this specific check is obsolete and must be corrected in
this bundle (narrow, in-scope: `scripts/guards/**` is an allowed area) —
replaced with a check that Bundle 6's own insertion-point/ordering invariants
hold, not that Bundle 6 doesn't exist. No Rust unit/integration test asserts
on `all_decisions`'s identity before `gather_and_apply` in a way that would
break from inserting a default-`off` (exact-passthrough) call — `off` mode
is behaviorally a no-op, so every existing Bundle 5 scenario test (8 named
DB-backed scenario files) is expected to remain green unmodified.

## 10. What exact files are in scope and why?

- `core-rs/crates/mqk-portfolio/src/conflict_policy.rs` (new) — the pure
  model (Phase A), mirroring `cycle.rs`'s zero-IO, zero-dependency shape;
  lives beside `allocator.rs`/`cycle.rs` since it is the same kind of object
  (pure pre-decision constraint layer) and the guard already asserts
  `mqk-portfolio` stays dependency-free.
- `core-rs/crates/mqk-portfolio/src/lib.rs` — export the new module's public
  types.
- `core-rs/crates/mqk-daemon/src/runtime_strategy_conflict_mode.rs` (new) —
  mode resolver + live hard lock, mirroring `runtime_opportunity_mode.rs`
  exactly (same closed-enum, same `effective_mode` shape).
- `core-rs/crates/mqk-daemon/src/runtime_strategy_conflict.rs` (new) —
  the impure `gather_and_resolve` glue (mirrors
  `runtime_opportunity_allocation.rs`'s split between pure `apply_*` and
  impure `gather_and_apply`), plus Phase G persistence glue.
- `core-rs/crates/mqk-daemon/src/state/loop_runner.rs` — one new call between
  `all_decisions` collection and `gather_and_apply` (Q1/Q2 above).
- `core-rs/crates/mqk-daemon/src/lib.rs` — register the two new modules.
- `core-rs/crates/mqk-daemon/src/routes.rs` — register three new GET routes.
- `core-rs/crates/mqk-daemon/src/routes/strategy_conflict.rs` (new) —
  read-only API (Phase D), mirrors `routes/portfolio_allocation.rs`.
- `core-rs/crates/mqk-db/migrations/0056_runtime_strategy_conflict_plans.sql`
  (new; `0056` is the next sequential id per `manifest.json`) +
  `manifest.json` update.
- `core-rs/crates/mqk-db/src/runtime_strategy_conflict.rs` (new) — durable
  evidence store, mirrors `mqk-db/src/runtime_opportunity_allocation.rs`.
- `core-rs/crates/mqk-db/src/lib.rs` — export the new module.
- `core-rs/mqk-gui/src/features/system/types/strategyConflict.ts`,
  `strategyConflict.ts` (parser), `StrategyConflictPolicyPanel.tsx` (new) +
  `api.ts` additions — mirror the allocation panel triad exactly.
- `core-rs/mqk-gui/src/features/system/systemStatusSections.ts` — add the new
  panel's section id.
- `core-rs/mqk-gui/src/features/settings/SettingsScreen.tsx` — mount the new
  panel.
- `scripts/guards/check_multi_strategy_conflict_policy_01.sh` (new) — Phase E
  structural guard, mirrors `check_runtime_opportunity_allocation_01.sh`.
- `scripts/guards/check_runtime_opportunity_allocation_01.sh` — narrow fix to
  check #11 (Q9 above).
- `.gitattributes` — add `*.sh text eol=lf` (Phase 0).
- `scripts/guards/check_migration_governance.sh` — narrow portability fix:
  falls back to `python` when `python3` is not resolvable (this Windows box's
  `python3` resolves to a Microsoft Store app-execution-alias stub, not a
  real interpreter; verified the guard's underlying manifest-vs-disk check
  passes when invoked with a real interpreter). Required so this guard,
  named explicitly in "Required Final Validation Commands", can actually run
  and pass on this box.
- `MiniQuantDesk_Master_Patch_Ledger_v2.md` — Bundle 5 acceptance checkpoint
  (Phase 0) + new Bundle 6 entry (Phase E), current-truth only.
