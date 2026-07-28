# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 — source audit

Answers the 15 current-truth facts and 15 authority questions from the Bundle 7
mission, from exact current source, before any runtime wiring. Verified
against `main` at `f712cd916771e32ef69202c6b17c09ebd77fed06`.

## Current truth to preserve (15 facts)

1. **`NativeStrategyBootstrap` consumes only `fleet_ids[0]`.**
   `core-rs/crates/mqk-runtime/src/native_strategy.rs:99-141`
   (`NativeStrategyBootstrap::bootstrap`). Line 114-115: "Single-strategy Tier
   A policy: consume only the first fleet entry." `ids[0].clone()` is the only
   element ever read from `fleet_ids`.

2. **`StrategyHost::register` remains single-registration.**
   `core-rs/crates/mqk-strategy/src/host.rs:34-42`. Second call to `register`
   returns `Err(StrategyHostError::MultiStrategyNotAllowed)`
   (`core-rs/crates/mqk-strategy/src/types.rs:144`). Enforced today by
   `scenario_multi_strategy_rejection.rs` and
   `scenario_parallel_long_short_strategy_01.rs`.

3. **The daemon plugin registry captures one scalar `MQK_STRATEGY_SYMBOL`.**
   `core-rs/crates/mqk-runtime/src/native_strategy.rs:302-322`
   (`build_daemon_plugin_registry_and_symbol`). One env read
   (`std::env::var("MQK_STRATEGY_SYMBOL")`), passed to
   `mqk_strategy::engines::register_builtin_strategies(&mut registry,
   symbol.clone())` — every built-in factory closure captures the same single
   symbol value.

4. **Multi-symbol assignments already exist as `SymbolStrategyAssignment`.**
   `core-rs/crates/mqk-daemon/src/state/multi_symbol_config.rs:92-97`:
   `{ symbol, strategy_id, timeframe }`. Built by
   `build_multi_symbol_runtime_config_from_env_and_watchlist` (line 360),
   preferring an approved watchlist-v2 artifact and falling back to the legacy
   single-symbol env path.

5. **`tick_strategy_dispatch_multi_symbol_with_bar_facts` dispatches
   sequentially, returning exact evaluated bar facts.**
   Called at `core-rs/crates/mqk-daemon/src/state/loop_runner.rs:634-638`;
   the `for (assignment, mut bar_result, bar_facts) in dispatch_results` loop
   (line 692) processes one assignment at a time and every downstream decision
   carries the `bar_facts` captured at that exact dispatch (line 1027-1034,
   Authority-repair Phase A comment).

6. **Multi-symbol dispatch uses one global strategy host; non-construction
   symbols are dropped by `retain_targets_matching_symbol`.**
   `loop_runner.rs:695-709`: the comment is explicit — "the native strategy
   bootstrap's `StrategyHost` emits `TargetPosition.symbol` fixed at
   construction time from `MQK_STRATEGY_SYMBOL`, independent of which symbol's
   bar window was just dispatched... per-symbol strategy bootstrap not yet
   implemented." `AppState::retain_targets_matching_symbol` drops every target
   whose symbol doesn't match `assignment.symbol`.

7. **`MQK_DRY_RUN_STRATEGY_IDS` already evaluates secondary strategies
   read-only.**
   `core-rs/crates/mqk-daemon/src/state/dry_run_strategy.rs`. Module doc
   (lines 13-23): builds a *fresh* `PluginRegistry` + `StrategyHost` per
   evaluated strategy id, never shares a host, never touches
   `AppState`/`PgPool`/broker (structurally cannot enqueue or submit — no such
   handle in any function signature in this module). This is the exact
   "independent host per strategy" pattern Bundle 7's host pool reuses; the
   difference is Bundle 7 needs one host per **(symbol, strategy_id)**, not
   per strategy_id alone, and needs the result to actually dispatch (not stay
   diagnostic-only) in `paper_enforced`.

8. **Durable promotion registry allows PAPER orders only for unexpired
   `active_paper`.**
   `core-rs/crates/mqk-db/src/strategy_promotion.rs`. Identity is
   `(strategy_id, symbol, timeframe_secs)` (every query keys on this triple).
   `evaluate_promotion_tradability` (line 715-745): `paper_tradable=true` only
   when `new_state == "active_paper"`, `effective_at_utc <= now_utc`, and
   (`expires_at_utc.is_none()` or `expires_at_utc > now_utc`).

9. **Scanner review decisions carry the required fields.**
   `core-rs/crates/mqk-backtest/src/strategy_scan_review.rs:105-115`
   (`StrategyScanReviewDecision`): `symbol`, `timeframe`, `strategy_id`,
   `scanner_rank: Option<usize>`, `scanner_score: Option<f64>`,
   `review_state: StrategyScanReviewState`.

10. **`paper_candidate` is research evidence only, not authorization.**
    `strategy_scan_review.rs:16-19` (module doc) and
    `PAPER_CANDIDATE_WARNING` (line 353-354): "requires a later, separately
    authorized paper-promotion patch before any paper trading." Confirmed
    downstream: `strategy_promotions.rs` treats a matched `paper_candidate`
    row only as *evidence* attached to a promotion transition, never as the
    transition itself.

11. **Bundle 6 conflict policy runs once across the complete same-tick batch
    before Bundle 5 allocation.**
    `loop_runner.rs:1062-1070`: `conflict_outcome =
    runtime_strategy_conflict::gather_and_resolve(..., all_decisions, ...)`.

12. **Bundle 5 allocation runs before cap #6 and canonical submission.**
    `loop_runner.rs:1086-1096`: `allocation_outcome =
    runtime_opportunity_allocation::gather_and_apply(...,
    conflict_outcome.decisions, ...)` — immediately after Bundle 6, consuming
    its output. The `for decision in allocation_outcome.decisions` submission
    loop (line 1102+) is where cap #6
    (`max_new_orders_per_tick_reason`) and
    `submit_internal_strategy_decision` live.

13. **Watchlist-v2 supports up to five symbols, one `strategy_assignments`
    entry per symbol.**
    `core-rs/crates/mqk-daemon/src/watchlist_intake.rs:86`:
    `pub const MULTI_SYMBOL_HARD_CEILING: u64 = 5;`. `symbol_assignments_from_artifact`
    (`multi_symbol_config.rs:316-337`) maps each symbol to exactly one
    `strategy_assignments.get(symbol)` entry, failing closed
    (`MissingAssignment`) if absent.

14. **Accepted fallback is one legacy environment symbol when no approved
    watchlist-v2 artifact is configured.**
    `multi_symbol_config.rs:360-382`
    (`build_multi_symbol_runtime_config_from_env_and_watchlist`): any
    non-`LoadedApproved`/non-v2/unbuildable watchlist outcome falls through to
    `build_legacy_single_symbol_config` (`source =
    EnvSingleSymbolFallback`, always exactly one assignment,
    `max_concurrent_symbols = 1`).

15. **All existing paper promotion, readiness, freshness, risk, session,
    reconcile, OMS, outbox, and broker gates remain mandatory.** Confirmed
    structurally: none of the modules audited above (promotion, conflict,
    allocation, dispatch) call `outbox_enqueue`, a broker client, or bypass
    `submit_internal_strategy_decision` — the canonical 7-gate admission seam
    remains the only route to an outbox row anywhere in this call chain.

## Authority questions

**1. Exact durable query for all current promotion identities for one
symbol/timeframe, including the latest evidence-bearing transition needed to
reproduce ranking evidence?**
`mqk_db::strategy_promotion::fetch_current_promotion_state(pool, strategy_id,
symbol, timeframe_secs)` (`strategy_promotion.rs:548-572`) returns the latest
transition row for the exact identity (`order by effective_at_utc desc,
created_at_utc desc, transition_id desc limit 1`). Combined with
`resolve_evidence_lineage(pool, record)` (line 781-795) to walk back to the
transition that actually carries evidence.

**2. Can the existing DB query recover the evidence path/fingerprint for an
`active_paper` row whose latest transition itself carries no evidence?**
Yes. `resolve_evidence_lineage`: if `record.evidence_review_id` is `None`, it
follows `record.evidence_transition_id` (carried forward by the promotion
route on every non-evidence-bearing transition, e.g. `demoted -> active_paper`
would not occur, but `active_paper`'s own row may not be the original
evidence-bearing row after intermediate re-transitions) and fetches that row
via `fetch_promotion_transition_by_id`. Returns `Ok(None)` only if the chain
itself is broken — reported honestly, never substituted.

**3. What exact review artifact row is fingerprinted?**
`mqk_backtest::StrategyScanReviewDecision` — the row in
`{review_dir}/review_decisions.json` matching
`(strategy_id, symbol, timeframe_secs)` after `symbol` uppercasing and
`scanner_timeframe_label_to_secs(&d.timeframe)` conversion. See
`validate_paper_candidate_evidence`,
`core-rs/crates/mqk-daemon/src/routes/strategy_promotions.rs:529-613`.

**4. How is that fingerprint recomputed and compared?**
`serde_json::to_string(matched)` (canonical serde field order) → SHA-256 →
hex (`strategy_promotions.rs:600-604`). Compared byte-for-byte against the
`evidence_fingerprint` column stored on the promotion transition at
approval time. Bundle 7's read-side validator must reuse this exact
recompute-and-compare, not reimplement it.

**5. Can one review artifact contain multiple candidates for the same
`(symbol,timeframe)`?**
Yes — the ambiguity check in `validate_paper_candidate_evidence` filters on
the full `(strategy_id, symbol, timeframe_secs)` triple, not on
`(symbol, timeframe)` alone; `matches.len() > 1` only fires for a duplicate
**identity** (same strategy too). A review artifact routinely contains many
different `strategy_id` rows for the same `(symbol, timeframe)` — this is
exactly what makes cross-strategy ranking for one symbol possible, and is the
candidate universe Bundle 7 selects over.

**6. Which rank/score field is authoritative when one is missing?**
`scanner_score: Option<f64>` is the primary ranking key. A candidate reaching
`PaperCandidate` review state can never have `scanner_score = None` —
`evaluate_scan_review_decision`
(`core-rs/crates/mqk-backtest/src/strategy_scan_review.rs:169-172`) blocks any
candidate with a missing score before it can reach `PaperCandidate`. However,
`evaluate_scan_review_decision` does **not** guarantee `scanner_rank` is
`Some` for a `PaperCandidate` row (rank is assigned by
`crate::strategy_scanner::rank_scan_candidates` and is not itself gated as a
promotion requirement) — so Bundle 7's ranking contract must independently
require `scanner_rank.is_some()` whenever a tie-break on rank is needed, and
reject (not default) a tie with a missing rank, per the mission's "absent/zero
rank when a tie requires rank" rejection rule.

**7. What exact strategy registry/plugin check proves the built-in can be
instantiated for a particular symbol?**
`mqk_strategy::PluginRegistry::instantiate_verified(strategy_id)`, exactly as
called by `NativeStrategyBootstrap::bootstrap` (line 117) and
`evaluate_dry_run_strategy` (`dry_run_strategy.rs:208`). The registry itself
must be built per-symbol first via
`mqk_strategy::engines::register_builtin_strategies(&mut registry, symbol)` —
today only `build_daemon_plugin_registry_and_symbol` (env-driven, one global
symbol) and `evaluate_dry_run_strategy` (per-call, throwaway) do this; Bundle
7 needs a third, source-driven constructor
(`build_daemon_plugin_registry_for_symbol(symbol: &str)`), narrowly factored
out of the existing `register_builtin_strategies` call, preserving the
existing env-driven builder unchanged for `off`-mode back-compat.

**8. What exact start-gate/readiness surfaces currently assume one effective
runtime binding?**
`mqk_runtime::native_strategy::EffectiveRuntimeBinding` (lines 336-341) and
`bootstrap_with_effective_binding`/`effective_binding_from_bootstrap` (lines
354-382) — single `effective_runtime_strategy_id` /
`effective_runtime_target_symbol` / `effective_runtime_timeframe_secs`
fields. `core-rs/crates/mqk-daemon/src/daily_data_readiness.rs` and the
autonomous readiness/status routes consume this single binding. These must
gain an additive selected-pair-count/detail projection without removing the
existing single-binding fields (back-compat requirement).

**9. Which code owns the active bootstrap from run start to stop/halt?**
`AppState` in `core-rs/crates/mqk-daemon/src/state.rs` holds the
`NativeStrategyBootstrap` (constructed once at run start,
`Option<NativeStrategyBootstrap>` = `None` meaning no active run — per that
struct's own doc comment in `native_strategy.rs:78-84`). A new run-scoped
`SelectionHostPool` must be held alongside it in `AppState`, constructed and
dropped on the same run-start/stop-halt lifecycle.

**10. What production call sites can access the selected host pool?**
Only `loop_runner.rs`'s per-tick dispatch block (the same call site that
today calls `tick_strategy_dispatch_multi_symbol_with_bar_facts`) — no route
handler, CLI command, or test harness outside the execution loop may dispatch
through it, mirroring today's `NativeStrategyBootstrap` access pattern (never
touched by an HTTP route directly; only deposited-signal state is).

**11. What exact API/GUI patterns should be reused for read-only durable
truth?**
Bundle 6's pattern: `core-rs/crates/mqk-daemon/src/routes/strategy_conflict.rs`
(GET-only, explicit `truth_state` enum, bounded list/detail, UTF-8-safe
blockers via `conflict_evidence_validation.rs`) and the GUI's
`StrategyScannerScreen.tsx`/`screenSource.test.ts` fail-closed-parser
convention. Bundle 7's read validator should be structured the same way
Bundle 6's `conflict_evidence_validation.rs` is: one shared
validate-and-recompute module used by status/list/detail routes alike.

**12. Next free migration ID?**
`0058` — confirmed from `core-rs/crates/mqk-db/migrations/manifest.json`
(highest recorded entry is `0057_runtime_strategy_conflict_evidence_provenance.sql`)
and directory listing (`0057...sql` is the newest file; no `0058` exists yet).

**13. Which existing guards assert Bundle 7 has not started and must now be
converted?**
`scripts/guards/check_multi_strategy_conflict_policy_01.sh`, function body
around lines 784-788:
```
# 13. Bundle 7 (dynamic strategy-symbol selection) not started.
if git ls-files | grep -qiE 'dynamic.strategy.symbol|bundle.?7'; then
  fail "Bundle 7 (dynamic strategy-symbol selection) must not be started"
fi
ok "Bundle 7 not started"
```
This check will fail the moment Bundle 7 source files are added (it matches
`dynamic.strategy.symbol` in tracked paths) and must be replaced with a
positive ordering/authority check (e.g. "Bundle 7 selection module never
calls outbox/broker directly", "Bundle 6 conflict call precedes Bundle 5
allocation call", asserted against the same source instead of asserting
Bundle 7's absence).

**14. Can the bundle meet the contract without changing stable public
strategy traits or `StrategyHost`'s single-registration invariant?**
Yes. The host pool is a new run-scoped collection of ordinary
`StrategyHost::new(ShadowMode::Off)` instances (one per selected
`(symbol, strategy_id)` pair), each receiving exactly one
`register()` call — identical to how `evaluate_dry_run_strategy` already
builds an independent host per call today. No change to `mqk-strategy`'s
`Strategy` trait, `StrategyHost`, or `StrategySpec` is required.

**15. What bounded candidate count is possible from five symbols and the
configured strategy fleet?**
`5 (MULTI_SYMBOL_HARD_CEILING) × |union(MQK_STRATEGY_IDS, per-symbol
watchlist-assigned strategy)|`. The four built-in engines
(`swing_momentum`, `mean_reversion`, `volatility_breakout`,
`intraday_scalper`) bound the realistic fleet size today, so the practical
ceiling is on the order of `5 × 5 = 25` candidates per plan (5 symbols, up to
4 fleet strategies plus at most 1 additional watchlist-assigned strategy per
symbol not already in the fleet) — small enough for the pure selector and the
durable candidate table to bound trivially without a separate paging design.

## Ranking-evidence recoverability conclusion

Ranking evidence **is** durably recoverable and fingerprint-validatable from
current source truth: `fetch_current_promotion_state` +
`resolve_evidence_lineage` (DB) together with `validate_paper_candidate_evidence`'s
recompute-and-compare (review artifact) give an unambiguous, already-proven
path from `(strategy_id, symbol, timeframe_secs)` to a specific
`StrategyScanReviewDecision` row and its SHA-256 fingerprint. Nothing in this
bundle needs to invent a new evidence path — it reuses the existing promotion
route's validation exactly, applied to every candidate instead of one
per-transition candidate.

The scope constraint in this bundle is not evidence availability; it is the
sheer surface area of full `paper_enforced` runtime wiring (host pool
integration into `loop_runner.rs`'s live tick path), DB persistence, API, GUI,
and the full guard/validation suite, several of which touch code paths that
gate real (paper) order submission. See the closure doc
(`dynamic_strategy_symbol_selection_01f_closure.md`) for exactly what shipped
in this session versus what remains open.
