# RUNTIME-OPPORTUNITY-ALLOCATION-01 — Phase A: Current Truth and Contract

Status: audit only. No runtime behavior is changed by this document.
Scope: answers the 10 required audit questions from source, cites exact
files/functions, and freezes the Bundle 5 semantics that Phases B-H build on.

## Method

Read in full (not excerpted) unless noted:

- `core-rs/crates/mqk-portfolio/src/allocator.rs` (624 lines)
- `core-rs/crates/mqk-portfolio/src/constraints.rs` (912 lines)
- `core-rs/crates/mqk-portfolio/src/lib.rs` (93 lines)
- `core-rs/crates/mqk-daemon/src/decision.rs` (872 lines)
- `core-rs/crates/mqk-daemon/src/watchlist_intake.rs` (first 270 of 640 lines;
  remainder is validation-error plumbing not load-bearing for this audit)
- `core-rs/crates/mqk-daemon/src/state/multi_symbol_config.rs` (423 lines)
- `core-rs/crates/mqk-daemon/src/state/loop_runner.rs` (lines 1-1060 of 1342;
  the per-symbol dispatch block, lines 620-1060, read in full)
- `core-rs/crates/mqk-daemon/src/state/snapshot.rs` (lines 560-890 of 1606;
  the durable-snapshot persist path)
- `core-rs/crates/mqk-daemon/src/state/env.rs` (285 lines — closed-mode env
  parsing style reference for Phase E)
- `research-py/src/mqk_research/scanner/scoring.py` (314 lines)
- `research-py/src/mqk_research/scanner/candidates.py` (318 lines)
- `research-py/src/mqk_research/scanner/selector.py` (308 lines)
- `research-py/src/mqk_research/scanner/watchlist_promotion.py` (478 lines)
- `docs/design/native_multi_symbol_dispatch.md` (headers/caps sections; the
  13-cap table in §6, and §4/§5 architecture sections)
- `core-rs/crates/mqk-db/migrations/` directory listing (through `0054`)
- `core-rs/crates/mqk-db/src/paper_portfolio.rs` (function signatures for
  snapshot fetch/persist)

Per-symbol target state (`per_symbol_target_state.rs` named in the task) does
not exist at that path; the equivalent module is
`core-rs/crates/mqk-daemon/src/state/per_symbol_bar_window.rs` plus
`build_per_symbol_target_state`/`record_per_symbol_target_state` calls inside
`loop_runner.rs`. `docs/audits/multi_asset_completion_audit.md` and
`docs/specs/market_scanner_backtest_candidate_pipeline.md` were grepped for
section headers rather than read line-by-line; nothing in their headers
contradicts the findings below, and neither is a source of runtime score
truth (both are equity/ETF-focused status audits, not scanner internals).

## Q1 — Where is the current scanner `total_score` produced?

`research-py/src/mqk_research/scanner/scoring.py::score_candidate()`
(lines 182-231). It is a Python `float`, computed as a weighted linear
combination of clamped `[0,1]` component scores minus risk/cost penalties,
then `round(_clamp01(raw), 6)` (line 220). This value flows into
`build_scored_scanner_candidate()` → `candidates.py::build_scanner_candidate()`
(`candidates.py:238-314`) as the `total_score` field of a `scanner-candidate-v1`
JSONL record (`candidates.py:24,55,147,194`). No Rust code computes or
re-derives `total_score`; it is Python-only, JSON-float, unscaled `[0,1]`.

## Q2 — Does the approved watchlist artifact preserve a numeric score and its lineage today?

**No, not on the path the daemon actually consumes.**

- `selector.py::build_ranked_candidate_export()` (lines 204-238) and
  `build_watchlist_artifact()` (lines 241-281) *do* copy `total_score` into a
  `ranked_candidates[]` array inside the `watchlist-v1` JSON file written to
  disk (`WatchlistArtifact.ranked_candidates`, `selector.py:121,137,276`).
  There is no `candidate_artifact_id`/hash field anywhere in this chain —
  `RankedCandidate.source_candidate_artifact` (`selector.py:71,88,230`) is
  always `None` in the current builder.
- `watchlist_promotion.py::PromotionDecision` /
  `apply_watchlist_promotion()` (lines 132-151, 427-465) — the gate that
  actually produces the *approved* artifact — carries only `passed`,
  `failure_reasons`, `approved_symbols`, `strategy_assignments`, `notes`.
  **No score field exists anywhere in `PromotionDecision` or the promoted
  artifact it writes.**
- Daemon-side, `watchlist_intake.rs::LoadedWatchlistArtifact` (lines 104-122)
  — the struct the runtime actually trusts — carries `schema_version`,
  `symbols`, `top_symbol`, `strategy_assignments`, `max_symbols_to_trade`,
  `max_concurrent_positions`, `approved_for_autonomous_paper`. **No score
  field, no candidate identity/hash field.** Even where the raw JSON on disk
  happens to contain `ranked_candidates[].total_score` (v1 selector output),
  `evaluate_watchlist_intake` never parses or preserves it.

Conclusion: the runtime has no source-proven score today. Per the task's own
instruction ("If the approved artifact does not preserve scanner score
lineage, add a narrow, immutable runtime-opportunity artifact as Phase B
rather than synthesizing scores in Rust"), **Phase B is required** — a new,
separate `runtime-opportunity-set-v1` artifact is the only source-proven path
to a score the allocator may use. Bundle 5 must not read `total_score` off
today's watchlist artifact, because the daemon-trusted `LoadedWatchlistArtifact`
type does not carry it and the score has no candidate-identity/hash lineage
even where it appears in the raw v1 JSON.

## Q3 — Which exact completed-bar price is used for each strategy evaluation?

`loop_runner.rs` dispatches via `state_arc.tick_strategy_dispatch_multi_symbol(&multi_symbol_assignments)`
(line 631), which internally loads the most recent **completed** bar window
per symbol/timeframe from `md_bars` (see `fetch_recent_completed_bars_for_strategy`,
also used directly at `loop_runner.rs:926` for the dry-run path). The
strategy's `TargetPosition` targets (`bar_result.intents.output.targets`,
`loop_runner.rs:669-911`) are evaluated against that bar window; the "exact
evaluation price" for Bundle 5 purposes is the `close_micros` of the latest
completed bar in that same window (`BarStub::new(b.end_ts, b.is_complete,
b.close_micros, b.volume)`, `loop_runner.rs:938-944`). Current positions used
for delta computation come from a **single shared execution-snapshot read**
per tick (`loop_runner.rs:639-648`, explicitly documented as "one shared
snapshot read covers every symbol dispatched this tick — no torn-snapshot
race across symbols"). Bundle 5's pure cycle model must use this same
per-symbol latest-completed-bar `close_micros` as its evaluation price, and
must fail closed (refuse new/increasing buys) if no completed bar exists for
a symbol in the candidate set.

## Q4 — Which exact durable portfolio NAV/equity state is authoritative for the allocation cycle?

The Bundle 4 durable snapshot store: `mqk_db::PaperPortfolioSnapshot` rows,
persisted via `insert_or_confirm_paper_portfolio_snapshot`
(`state/snapshot.rs:863-879`) and read via
`fetch_latest_paper_portfolio_snapshot_for_run` /
`fetch_latest_paper_portfolio_snapshot` (`mqk-db/src/paper_portfolio.rs:354,410`).
A row is authoritative only when: `source == PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA`,
`currency == "USD"` (`snapshot.rs:784-790`), `truth_state == "active"`
(`snapshot.rs:873`), and it carries a real `run_id` (`snapshot.rs:776-779`,
"a run_id-less snapshot can never be resolved by any run-scoped route").
`equity_micros` is parsed from the broker's decimal equity string
(`snapshot.rs:792-797`) — never fabricated, never defaulted. The
`routes/durable_portfolio.rs` summary route additionally classifies staleness
(`DURABLE_SNAPSHOT_STALE_SECS`, line ~297) and provenance
(`classify_portfolio_provenance`, line ~415), producing a closed
`truth_state` vocabulary (`active`, `stale`, `snapshot_unavailable`,
`invalid_snapshot`, `db_unavailable`, `query_failed`, `not_found`,
`unsupported_source`). Bundle 5's pure cycle model must require
`truth_state == "active"` (not stale, not any other value) on the snapshot it
reads and must fail closed for new/increasing buys otherwise — it must never
call these DB fetch functions itself (pure, no I/O); the caller (runtime
wiring, Phase F) supplies the already-fetched, already-classified snapshot.

## Q5 — How are current positions represented?

As `BTreeMap<String, i64>` (symbol → signed net quantity), built once per
tick from the execution snapshot cache (`loop_runner.rs:639-648`):
`s.portfolio.positions.iter().map(|p| (p.symbol.clone(), p.net_qty))`. A
symbol absent from the map is treated as flat (`current_positions.get(&t.symbol).copied().unwrap_or(0)`,
e.g. `decision.rs:294`, `loop_runner.rs:848`). This is the same
representation the allocation-cycle model should accept as its "current
positions" input — no new representation is needed.

## Q6 — Where do new/increasing buys, reducing sells, exits, and flatten orders diverge?

They do not diverge structurally today — `bar_result_to_decisions`
(`decision.rs:275-340`) computes one signed `delta = target.qty - current`
per symbol and classifies it via `crate::capital_policy::classify_order_intent(current, delta)`
(`decision.rs:304`) into `OrderIntent` variants: `LongOpen`, `BuyToCover`,
`BuyToFlat`, `BuyBeyondShortToLong` → `side="buy"`; `SellToClose`,
`SellToFlat` → `side="sell"`; `ShortOpen`, `SellBeyondLongToShort`, `NoOp` →
dropped (B5 guard / no-op, `decision.rs:314-317`). This classification
already exists and is exactly the seam Bundle 5 must key off: **buy-side
decisions from `LongOpen`/`BuyBeyondShortToLong` (new or increasing long
exposure) are the only ones eligible for opportunity-allocation competition.**
`BuyToCover`/`BuyToFlat` (covering a short back toward/through flat) cannot
occur in this long-only runtime (shorts are already excluded by the B5 guard
upstream), so in practice the buy-side competition set is `LongOpen`-only.
All sell-side decisions (`SellToClose`, `SellToFlat`) must pass through
unaltered — Bundle 5 must not touch them. Flatten/operator-halt orders are
generated by an entirely separate path (not `bar_result_to_decisions`) and
are untouched by this bundle by construction (Bundle 5 only wires into the
`bar_result_to_decisions` → `submit_internal_strategy_decision` loop, per the
task's Phase F instructions).

## Q7 — Which current caps already constrain per-symbol position quantity, concurrent positions, per-tick orders, and strategy budgets?

Per `docs/design/native_multi_symbol_dispatch.md` §6 (13-cap table) and
direct source confirmation:

| Cap | Mechanism | Source |
|---|---|---|
| #1 `max_concurrent_symbols` | `MultiSymbolRuntimeConfig.max_concurrent_symbols`, from watchlist `max_symbols_to_trade` | `multi_symbol_config.rs:127,306` |
| #2 `per_symbol_max_position_qty` | `AppState::clamp_targets_to_per_symbol_position_cap` | `loop_runner.rs:758-812` |
| #3 `per_symbol_max_notional_usd` | `capital_policy::evaluate_per_symbol_notional_cap_from_env` (Gate 1g) | `decision.rs:495-546` |
| #4 `per_symbol_day_order_count_limit` | `state.symbol_day_order_limit_exceeded` (Gate 1f) | `decision.rs:385-403` |
| #6 `max_new_orders_per_tick` | per-tick counter, skips remaining symbols once reached | `loop_runner.rs:665-706` |
| account-wide budget | `capital_policy::evaluate_strategy_budget_from_env` (Gate 1e) | `decision.rs:405-474` |
| account-wide day-signal | `state.day_signal_limit_exceeded` (Gate 1) | `decision.rs:365-379` |
| sector exposure | `evaluate_sector_risk_gate` (Gate 1h) | `decision.rs:548-618`, `constraints.rs:517-611` |

Cap #5 (`aggregate_gross_exposure_cap_usd`) is documented as a design sketch
(`native_multi_symbol_dispatch.md:1251-1263`), distinct from the existing
account-wide `max_portfolio_notional_usd`; it is **not required or touched by
Bundle 5**. Bundle 5's allocator sits *before* all of these gates in the
governing order (per the task's "Governing Authority Rule") and can only
narrow the candidate buy set further — every cap above still runs
downstream, unchanged, on whatever the allocator passes through.

## Q8 — Which runtime paths can create an outbox row?

`mqk_db::outbox_enqueue` (`mqk-db/src/orders.rs:199`) is the only insert path,
called from exactly two call sites in the daemon: (1)
`decision.rs:842` inside `submit_internal_strategy_decision` (Gate 7, the
internal/native-strategy path Bundle 5 wires into), and (2) the external
signal HTTP route (`routes/strategy.rs`, `POST /api/v1/strategy/signal`,
sharing the same gate helpers via `capital_policy`/`promotion_gate` but a
separate call site — out of scope for Bundle 5, which only touches the
internal/native path per the task). Bundle 5 must not add a second outbox
write path or call `outbox_enqueue` directly; all Bundle-5-influenced
decisions must still flow through `submit_internal_strategy_decision`
unchanged.

## Q9 — How is one completed-bar cycle identified idempotently?

There is no single explicit "cycle_id" today. The natural cycle identity for
one tick's multi-symbol dispatch is the tuple `(run_id, now_micros)` computed
once per tick (`loop_runner.rs:634`, "allow: loop-context wall-clock for
decision_id") and shared across every symbol's `dispatch_results` entry
processed in that `for (assignment, mut bar_result) in dispatch_results`
loop (`loop_runner.rs:669`). `decision_id` itself is a UUIDv5 of
`"{run_id}:{strategy_id}:{symbol}:{side}:{qty}:{now_micros}"`
(`decision.rs:318-326`), which is idempotent per decision but not a
cycle-level identity. Bundle 5's pure cycle model (Phase D) must mint its own
deterministic `cycle_id` — a UUIDv5 derived from stable inputs available at
the same call site (`run_id`, the shared `now_micros`, and the sorted set of
symbols/timeframes dispatched this tick) so that re-running the same
logical cycle (e.g. after a crash/restart mid-tick) produces the same
`cycle_id` and durable evidence write is idempotent (`ON CONFLICT DO
NOTHING`/upsert-by-id, matching the outbox's own idempotency pattern).

## Q10 — Can one strategy emit more than one target for the same symbol?

Structurally, yes, and nothing upstream of `bar_result_to_decisions` prevents
it: `AppState::retain_targets_matching_symbol` (`loop_runner.rs:719-722`)
filters `bar_result.intents.output.targets` to targets whose symbol matches
the *dispatched* assignment, but does not deduplicate multiple targets that
already share that same symbol. `bar_result_to_decisions` then iterates
`result.intents.output.targets` unconditionally (`decision.rs:285-289`) — if
two targets exist for the same symbol, two `InternalStrategyDecision`s would
be produced and independently submitted. In practice this has not been
observed because native strategies are documented (design doc §4.1) as
emitting at most one target per assigned symbol per bar, but it is not
enforced. **Bundle 5's allocation-cycle model must treat this as a
fail-closed input-validation case**: a duplicate symbol within one cycle's
candidate set is rejected (mirrors the allocator's own duplicate-symbol
rejection requirement), not silently deduplicated by last-write-wins.

## Frozen Bundle 5 semantics (binding for Phases B-H)

1. **Score source**: only `runtime-opportunity-set-v1` (Phase B, new
   artifact). Never the raw scanner/watchlist JSON `total_score` field, and
   never list order, symbol name, or target quantity.
2. **Evaluation price**: per-symbol latest completed-bar `close_micros` from
   the same window the strategy was dispatched against this tick (Q3).
3. **NAV/equity authority**: the single durable `PaperPortfolioSnapshot` row
   with `truth_state == "active"`, `source == external_alpaca`,
   `currency == "USD"`, real `run_id` (Q4). Passed in by the caller — the
   pure cycle model never queries the DB itself.
4. **Current positions**: `BTreeMap<String, i64>` from the shared per-tick
   execution-snapshot read (Q5) — reused as-is.
5. **Competition set**: only `LongOpen`-classified (new/increasing long)
   decisions from `bar_result_to_decisions` enter allocation. Sell/exit/
   flatten decisions bypass allocation entirely and are submitted unchanged
   (Q6).
6. **Downstream caps unchanged**: allocation output still passes through
   every existing cap in Q7's table, in the existing order, via the existing
   `submit_internal_strategy_decision` seam (Q8) — Bundle 5 adds one narrowing
   step before that seam, nothing after it.
7. **Cycle identity**: a new deterministic `cycle_id` (UUIDv5 of
   `run_id` + shared `now_micros` + sorted dispatched-symbol set), minted in
   Phase D, distinct from but derived from the same inputs as `decision_id`
   (Q9).
8. **Duplicate-symbol-in-cycle**: fail closed, reject the cycle's
   new/increasing-buy competition for that symbol rather than pick one
   arbitrarily (Q10).
