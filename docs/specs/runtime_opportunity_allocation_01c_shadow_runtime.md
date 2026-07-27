# RUNTIME-OPPORTUNITY-ALLOCATION-01 — Phase D/E/F: Cycle Model, Mode, Wiring

Status: implemented and tested.

## Pure allocation-cycle model (Phase D)

[core-rs/crates/mqk-portfolio/src/cycle.rs](../../core-rs/crates/mqk-portfolio/src/cycle.rs)

`compute_allocation_cycle(context, candidates, runtime_ceiling) ->
AllocationCycleResult` is zero-I/O, zero-dependency (mqk-portfolio remains a
dependency-free crate — `cycle_id`, `run_id`, `source_snapshot_id`, and every
other identity/timestamp is supplied by the caller, which owns the uuid/clock
plumbing this crate intentionally does not carry).

Rules (15 unit tests):

- Nonpositive `equity_micros` fails the **whole cycle** closed
  (`fail_closed_nonpositive_equity`) — every candidate refused.
- A duplicate symbol within one cycle's candidate set fails the **whole
  cycle** closed (`fail_closed_duplicate_symbol_in_cycle`) — never resolved
  by picking one arbitrarily (Phase A audit Q10).
- Nonpositive/missing price, invalid score, or a non-increasing target
  (`strategy_target_qty <= current_qty`) fails closed **only for that one
  candidate** — it does not reach the allocator and does not affect siblings.
- An allocator error (e.g. `runtime_ceiling == 0`, a caller bug) fails the
  whole cycle closed.
- `final_target_qty` is always in `[current_qty, strategy_target_qty]` —
  allocation can narrow a buy toward "no trade" but can never exceed
  strategy intent or flip a buy into a sell.
- Whole-share conversion floors (`(weight * equity_micros /
  price_micros).floor()`) — conservative, never rounds up.
- Output candidates are sorted by symbol — the result is bit-identical
  regardless of the input candidate slice's order, and identical across
  repeated calls with the same input (idempotent by construction — pure
  function, no internal mutable state).

## Runtime mode switch + live-lock (Phase E)

[core-rs/crates/mqk-daemon/src/runtime_opportunity_mode.rs](../../core-rs/crates/mqk-daemon/src/runtime_opportunity_mode.rs)

`MQK_RUNTIME_OPPORTUNITY_ALLOCATION_MODE` ∈ `{off, shadow, paper_enforced}`,
case-insensitive, trimmed. Absent/blank → `Off`, no invalid-configuration
flag. Any other non-matching value → `Off` **with** an
`invalid_configuration: Some(<value>)` flag the status API/GUI must surface
(never silently swallowed, never a panic).

`effective_mode(resolution, deployment_mode, broker_kind)` applies the hard
live-lock: any configured mode other than `Off` is forced all the way down
to `Off` (not `Shadow`) unless `deployment_mode == Paper && broker_kind ==
Some(Alpaca)`. Demoting even `Shadow` to `Off` outside the frozen paper+Alpaca
lane is deliberate — Shadow still reads the durable snapshot and opportunity
artifact, and Bundle 5 must have zero footprint of any kind outside that lane.
9 unit tests cover every mode × deployment/broker combination.

## Runtime wiring (Phase F)

[core-rs/crates/mqk-daemon/src/runtime_opportunity_allocation.rs](../../core-rs/crates/mqk-daemon/src/runtime_opportunity_allocation.rs)
+ [core-rs/crates/mqk-daemon/src/state/loop_runner.rs](../../core-rs/crates/mqk-daemon/src/state/loop_runner.rs)

`apply_runtime_opportunity_allocation` (pure) is the batching/apply core; 12
unit tests. `gather_and_apply` (async, the only impure code in the module) is
the single per-tick call site `loop_runner.rs` uses:

1. Resolves the effective mode. If `Off`, returns immediately — zero I/O,
   zero allocator call, decisions unchanged. This is the default and is the
   exact pre-Bundle-5 code path's observable behavior.
2. Otherwise, partitions the tick's decisions into buy (`side == "buy"`,
   already `LongOpen`-classified upstream by the existing
   `classify_order_intent`) and everything else. Non-buy decisions are
   **never touched** — sells, exits, flattens pass through unchanged in
   every mode.
3. If there are no buy decisions this tick, returns immediately (nothing to
   allocate) — `plan: None`.
4. Otherwise loads the watchlist + `runtime-opportunity-set-v1` artifact,
   resolves the latest durable `PaperPortfolioSnapshot` (must be
   `truth_state == "active"`, `currency == "USD"`, `source ==
   external_alpaca`, not older than 180s — mirroring
   `routes/durable_portfolio.rs`'s own staleness constant, duplicated rather
   than cross-module-exposed to avoid touching Bundle 4's route file), and
   fetches each buy-candidate symbol's latest completed-bar close price via
   the existing `fetch_recent_completed_bars_for_strategy` read path (same
   call the dry-run diagnostics already use).
5. Missing/invalid artifact or snapshot refuses **every buy** this cycle
   (`fail_closed_no_opportunity_authority` /
   `fail_closed_no_durable_snapshot`) without fabricating a score or
   allocation — sells are untouched.
6. A buy symbol not covered by the opportunity artifact is refused
   individually (`fail_closed_no_opportunity_score_for_symbol`) — it does
   not get a default score.
7. Otherwise: one `compute_allocation_cycle` call over the whole buy-candidate
   batch (never one call per symbol — proven by
   `one_allocator_call_per_tick_not_per_symbol`).
8. **Shadow**: the plan is computed and (best-effort) persisted, but the
   original buy decisions pass through to submission completely unchanged —
   zero allocator-driven outbox difference.
9. **PaperEnforced**: each buy decision's qty is replaced with
   `final_target_qty - current_qty` (a fresh `decision_id`, since the
   original embeds the old qty); a zero delta means the decision is dropped
   entirely (no submission, no trade this cycle) rather than submitted with
   qty=0.

### `loop_runner.rs` restructuring

The per-symbol dispatch loop changed from "derive and submit each symbol's
decisions immediately" to "derive every symbol's decisions, batch them, call
`gather_and_apply` once, then submit" — this is the minimum structural change
needed for one allocation call to see the whole same-cycle candidate set.

Everything else in the per-symbol loop is untouched at its original point:
cap #2 (`per_symbol_max_position_qty` clamp), the symbol-mismatch guard, raw
signal-qty recording, dry-run diagnostics, and per-symbol target-state
recording for non-decision outcomes all still run before decision derivation,
exactly as before.

**Cap #6 (`max_new_orders_per_tick`) relocation**: this cap counts *accepted*
submissions, which can only be known at submission time. Bundle 5's
"collect the whole cycle before submitting" requirement makes its old
position (an early per-symbol skip *before* decision derivation) impossible
to keep unchanged, so it moved to the submission pass — applied in the same
dispatch order, over the (possibly allocation-adjusted) decision list,
with the identical skip condition and `"max_new_orders_per_tick_reached"`
reason string. The cap's *effect* (limit total accepted orders per tick) is
unchanged; only *where in the pipeline* the check runs moved, and only
because Bundle 5 makes the old position structurally impossible to keep.

### Regression proof

Against the isolated port-5434 test DB: `scenario_multi_symbol_
{runtime_config,dispatch_loop,tick_order_cap,capital_caps,day_order_cap,
dispatch_summary}_01`, `scenario_daemon_routes`,
`scenario_gui_daemon_contract_gate`, `scenario_internal_strategy_decision`,
`scenario_native_strategy_b6_budget_gate`,
`scenario_durable_paper_portfolio_and_pnl_01`,
`scenario_durable_paper_portfolio_read_only_api_01`, and
`scenario_autonomous_daily_operation_api_01` all pass unchanged. Full
mqk-daemon lib suite: 424/426 (the 2 failures are pre-existing
`alpaca_ws_transport` test-DB migration-state drift on this shared test
database, unrelated to any file this bundle touches — no migration was added
until Phase G, which itself proves clean against the same DB).
