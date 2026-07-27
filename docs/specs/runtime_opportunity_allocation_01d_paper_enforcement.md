# RUNTIME-OPPORTUNITY-ALLOCATION-01 — Phase G: Durable Evidence + Enforcement Proof

Status: implemented and tested.

## Durable evidence store

Migration:
[core-rs/crates/mqk-db/migrations/0055_runtime_opportunity_allocation_plans.sql](../../core-rs/crates/mqk-db/migrations/0055_runtime_opportunity_allocation_plans.sql)

Two additive tables, no existing table modified:

- `sys_runtime_opportunity_allocation_plans` — one row per allocation cycle.
  `plan_id` **is** the deterministic `cycle_id`. **Repaired** (see
  RUNTIME-OPPORTUNITY-ALLOCATION-01-READINESS-AND-AUTHORITY-REPAIR-01, Phase
  B): originally UUIDv5 of `run_id` + the loop-tick wall clock
  (`now_micros`) + the sorted dispatched-symbol set — that included the
  wall clock, so reprocessing the exact same completed-bar economic cycle
  on a later tick minted a *different* id, and durable-insert idempotency
  was not actually bound to the economic cycle. Now UUIDv5 of `run_id` +
  `market_date` + `timeframe` + the opportunity artifact id + the sorted
  `(symbol, strategy_id, exact completed-bar end-timestamp)` tuple set of
  the candidates with proven bar facts — no wall clock, no insertion order,
  no random id. Reprocessing the same run/artifact/bar candidate set on a
  later tick now yields the identical `cycle_id`/`plan_id`, so re-persisting
  the same logical economic cycle (e.g. after a crash/restart mid-tick, or a
  later tick re-observing the same completed bar) is a proven no-op, not a
  duplicate.
- `sys_runtime_opportunity_allocation_candidates` — child rows, one per
  new/increasing-buy candidate considered. Sell/reduce/exit/flatten
  decisions never enter this model and are never rows here.

**Not portfolio/P&L/order truth**: `equity_micros` and `source_snapshot_id`
are copies of the already-durable `sys_paper_portfolio_snapshots` row the
cycle read — this table adds no second source of NAV truth. Scores/weights
are stored as scaled integers (`x1,000,000`), never a binary float column.

[core-rs/crates/mqk-db/src/runtime_opportunity_allocation.rs](../../core-rs/crates/mqk-db/src/runtime_opportunity_allocation.rs)
provides `insert_runtime_opportunity_allocation_plan` (idempotent: existence
check + rollback-on-duplicate inside one transaction covering plan+candidates
together), `fetch_runtime_opportunity_allocation_plan`, and
`fetch_recent_runtime_opportunity_allocation_plans`. Wired into
`gather_and_apply` as a **best-effort** persist — a write failure is logged
and never blocks or fails the tick that produced the plan (this is evidence,
not authoritative truth; the tick must proceed regardless of whether its
evidence row landed).

3 DB-backed tests (round-trip, idempotent replay, recent-plans
ordering/run-scoping) pass against the isolated port-5434 test DB. Migration
governance guard (`scripts/guards/check_migration_governance.sh`) passes:
manifest matches the authoritative SQL chain.

## `paper_enforced` enforcement proof

Enforcement is only reachable when `effective_mode(...) ==
PaperEnforced`, which itself requires `deployment_mode == Paper &&
broker_kind == Some(Alpaca)` (the live-lock; Phase E). Given that:

1. **Buy narrowing**: `paper_enforced_clamps_buy_to_allocator_output` proves
   a strategy target of 10,000 shares gets clamped to whatever the
   allocator's 20%-single-position cap actually funds on the test equity —
   strictly less than the strategy's ask, never more.
2. **Zero-capital drop**: `paper_enforced_drops_decision_when_no_capital_
   available` proves a lower-scored candidate that the allocator's
   `max_positions` ceiling excludes is **not submitted at all** (no qty=0
   order), rather than being silently allowed through.
3. **Sells always pass**: `sell_decisions_always_pass_through_in_shadow_and_
   enforced` proves a sell decision is bit-identical to its pre-allocation
   form in both `Shadow` and `PaperEnforced` — the allocator has no path to
   touch a risk-reducing/exit order.
4. **Missing authority refuses buys, not sells**:
   `missing_opportunity_artifact_refuses_all_buys_but_not_sells` proves a
   sell survives even when the opportunity artifact is entirely absent and
   every buy for that cycle is refused.
5. **Live hard lock**: `runtime_opportunity_mode.rs`'s
   `live_capital_forces_off_even_when_paper_enforced_configured` and
   `non_alpaca_adapter_forces_off_even_in_paper_mode` prove the env var
   cannot enable any allocator influence outside the frozen paper+Alpaca
   lane, regardless of what is configured.

## False-truth review (Phase G/repair-cycle 3)

Each claim below was independently attempted to be falsified against the
actual test suite, not asserted from the implementation alone:

| Claim | Falsification attempt | Result |
|---|---|---|
| Allocator scores are source-proven | Tried to make the allocator accept a score not traceable to `scanner_total_score` | Blocked — the artifact validator rejects any `score_source != "scanner_total_score"` before the daemon ever builds a candidate; the daemon has no other score input path |
| Candidate order cannot determine capital | Ran identical candidates in forward and reversed order through both the allocator and the cycle model | `output_deterministic_across_candidate_input_order`, `different_candidate_input_order_produces_identical_plan` — bit-identical results |
| Duplicate symbols cannot overwrite silently | Constructed a 2-candidate set with the same symbol twice, in both the allocator and the cycle model | Both fail closed (`DuplicateSymbol` / `fail_closed_duplicate_symbol_in_cycle`) rather than picking one |
| Stale artifacts cannot influence targets | Set `expires_at_utc` before the evaluation `now` | `rejects_expired_artifact` — artifact treated as absent, all buys refused |
| Malformed NAV/price cannot become zero/default | Passed `equity_micros = 0` and `evaluation_price_micros = 0` | Both fail closed with an explicit reason (`fail_closed_nonpositive_equity`, `nonpositive_or_missing_price`) — no candidate silently gets a zero-cost allocation |
| One symbol cannot consume capital through per-symbol allocator calls | Confirmed via source read (`gather_and_apply` calls `compute_allocation_cycle` exactly once per tick) plus `one_allocator_call_per_tick_not_per_symbol` | Confirmed — one call, whole batch |
| Allocation cannot increase strategy intent | Tried a candidate whose allocator-implied share count exceeds the strategy's own target | `allocation_never_exceeds_strategy_target` — clamped to `strategy_target_qty`, never above |
| Hold cannot become buy | Constructed `strategy_target_qty <= current_qty` inputs | `hold_cannot_become_buy`, `reducing_target_does_not_enter_competition` — excluded before the allocator, `final_target_qty == current_qty` |
| Allocation cannot block exits/flatten | Confirmed sells never enter `apply_runtime_opportunity_allocation`'s buy partition, and flatten/operator-halt paths never call `bar_result_to_decisions` at all (separate code path, per Phase A Q6) | Confirmed by source read + `sell_decisions_always_pass_through_in_shadow_and_enforced` |
| Live mode cannot enable influence | Configured `paper_enforced` under `LiveCapital`/`LiveShadow`/non-Alpaca | `effective_mode` forces `Off` in every case |
| Shadow cannot change outbox behavior | Ran Shadow mode with a clamping-eligible candidate | `shadow_mode_produces_plan_but_leaves_buy_qty_unchanged` — submitted qty unchanged |
| Allocation tables cannot masquerade as portfolio/P&L truth | Read the migration and route code | `equity_micros`/`source_snapshot_id` are copies, never independently computed; no route in `portfolio_allocation.rs` writes any row |
| GUI cannot render malformed allocation as active | Fed the parser deliberately malformed bodies (unrecognized truth_state, `approved_for_live: true`, mode/influence mismatch, active-with-null-plan, non-active-smuggling-a-plan) | All 9 malformed-input GUI parser tests downgrade to the unavailable sentinel |

No confirmed defects were found in this review; nothing required repair.
