# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 — closure (PARTIAL)

Status: **PARTIAL**. This session lands the source audit and the pure
selection model only. It does not wire a host pool into the live tick loop,
does not add a migration or durable evidence, does not add an API route or
GUI panel, does not rewrite the Bundle 6 "Bundle 7 not started" guard, and
does not attempt the full validation suite (DB tests, GUI tests, PowerShell
guards, premarket run) the mission requires for `FINAL: PASS`.

## Why PARTIAL, not COMPLETE or BLOCKED

Ranking evidence *is* durably recoverable and fingerprint-validatable from
current source truth — see
`dynamic_strategy_symbol_selection_01a_current_truth_and_contract.md`'s
"Ranking-evidence recoverability conclusion". Nothing here is `BLOCKED` on a
missing capability, an unavailable evidence path, or a design that would
require weakening `StrategyHost`, live routing, or P&L/signal-based ranking.

The constraint is scope: the full mission spans nine commit-sized units of
work (pure model; run-scoped host pool wired into `loop_runner.rs`'s live
per-tick dispatch; an additive `0058` migration plus durable
plan/candidate/selection tables; three new GET-only API routes with a
shared read-side recomputation validator; a GUI panel; a guard-script
rewrite with mutation-negative fixtures; PowerShell premarket/refresh
integration; and full validation ending in
`Invoke-PaperPremarketValidation.ps1` reporting `FINAL: PASS`). Several of
those units touch the exact code path that gates real (paper) order
submission (`loop_runner.rs`) or assert live-capital safety (the guard
script, the API live lock). Landing them without the ability to exercise
each one against a running paper DB and the full guard/regression suite in
this session would produce exactly the kind of half-finished,
optimistic-but-unproven implementation `CLAUDE.md` and this bundle's own
stop conditions rule out ("Return PARTIAL... rather than guessing").

## What shipped

1. `docs/specs/dynamic_strategy_symbol_selection_01a_current_truth_and_contract.md`
   — Phase 0 audit. Confirms all 15 current-truth facts and answers all 15
   authority questions against exact source locations (file:line citations),
   including the exact durable promotion + evidence-fingerprint recovery
   path.
2. `core-rs/crates/mqk-portfolio/src/dynamic_selection.rs` (+ `lib.rs`
   wiring) — pure, zero-dependency selection model:
   - `DynamicSelectionMode` (closed `off`/`shadow`/`paper_enforced`
     vocabulary, `parse`/`as_str`).
   - `SelectionCandidateEvidence` / `SelectionCandidateInput` — every gate
     the mission requires (promotion query success, `active_paper`,
     effective/expiry, evidence-lineage resolution, `paper_candidate`
     review state, fingerprint match, plugin instantiability, timeframe
     match, data readiness, present score) as caller-supplied booleans this
     module gates on in a fixed, documented order — first failing reason
     wins.
   - `compute_dynamic_selection_plan` — ranks by canonical
     `scanner_score` micros (descending), then `scanner_rank` (ascending,
     refusing a tie a missing/non-positive rank cannot resolve), then
     watchlist-assignment preference, then canonical `strategy_id`
     ascending. Every eligible symbol appears in the output even with zero
     candidates (no silent omission). Duplicate `(symbol, strategy_id)`
     identities collapse when evidence is identical (idempotent replay) and
     refuse both rows when evidence diverges.
   - 33 unit tests: every refusal reason, the full tie-break ladder, input-
     order and eligible-symbol-order independence, multi-symbol
     independence, and plan-level helper methods (`selected_pairs`,
     `selected_count`, `refused_count`, etc).
   - Zero callers. `off`/`shadow` economic-passthrough proof and
     `paper_enforced` host-pool proof are not applicable yet because nothing
     calls this module from the runtime — that proof belongs to the not-yet-
     landed wiring commit.

Both commits pass `cargo build`/`cargo test`/`cargo clippy -- -D warnings`
for `mqk-portfolio`, and `mqk-daemon` still builds cleanly against the
additive `lib.rs` exports.

## What remains open (named, with exact next steps)

- **Run-scoped host pool** (`SelectionHostKey`, one `StrategyHost` per
  selected `(symbol, strategy_id)` pair, held in `AppState` alongside
  `NativeStrategyBootstrap`) and its wiring into
  `core-rs/crates/mqk-daemon/src/state/loop_runner.rs`'s per-tick dispatch,
  preserving the exact `dispatch → symbol-mismatch guard → cap #2 →
  Bundle 6 → Bundle 5 → cap #6 → submission` order. Needs a
  `build_daemon_plugin_registry_for_symbol(symbol)` seam factored out of
  `mqk_strategy::engines::register_builtin_strategies` (see audit Q7).
- **Migration `0058`** (confirmed free) plus
  `sys_dynamic_selection_plans` / `..._candidates` / `..._selections` tables,
  additive only, no `DEFAULT now()`/`gen_random_uuid()`, caller-supplied
  `plan_id`/timestamps throughout (mirrors migrations `0055`-`0057`).
- **Daemon-side plan-identity minting**: the pure model exposes every
  result-affecting fact; the daemon layer (which already depends on `uuid`,
  unlike this zero-dependency crate) must derive the UUIDv5 `plan_id` from a
  canonical, length-prefixed serialization of those facts, and a read-side
  validator must recompute it identically — the same "caller mints, reader
  recomputes" pattern already established in `runtime_strategy_conflict`'s
  `compute_conflict_cycle_id`.
- **API**: `GET /api/v1/strategy/selection/status|plans|plans/:plan_id`,
  reusing Bundle 6's `conflict_evidence_validation.rs`-style shared
  validator and UTF-8-safe bounded-blocker pattern.
- **GUI**: read-only Dynamic Strategy Selection panel (no new route logic
  beyond wiring to the above).
- **Guard rewrite**: `scripts/guards/check_dynamic_strategy_symbol_selection_01.sh`
  (new), plus replacing
  `scripts/guards/check_multi_strategy_conflict_policy_01.sh` lines 784-788
  ("Bundle 7 not started") with a positive Bundle 7 ordering/authority
  check — required before any Bundle 7 source lands permanently, since the
  existing check will otherwise fail CI the moment these files are pushed.
- **Ops/docs**: `docs/design/native_multi_symbol_dispatch.md`,
  `docs/runbooks/autonomous_paper_ops.md`,
  `docs/runbooks/intraday_market_data_refresh.md`, `.env.local.example`,
  and `MiniQuantDesk_Master_Patch_Ledger_v2.md` are intentionally **not**
  updated in this session — they would otherwise document a mode contract,
  env var, and GUI panel that do not exist in committed code yet, which is
  exactly the "claim closure beyond the evidence" failure mode `CLAUDE.md`
  rules out.
- **Full validation suite**: DB-backed selection evidence tests on port
  5434, API malformed-evidence tests, full GUI build/tests, all PowerShell
  guards, the Bundle 7 guard/self-test, and the final
  `Invoke-PaperPremarketValidation.ps1` run — none of these have a subject
  to test yet (no migration, no route, no GUI panel, no guard script exist
  in this session's commits).

## Recommended next session scope

Land the host pool + migration + daemon wiring as one focused unit (mission
commits 3-5), proven against isolated port 5434 with the existing
`off`/`shadow` economic-passthrough tests before any `paper_enforced`
dispatch test is attempted — that is the highest-risk remaining surface
(it touches live per-tick dispatch) and should not be combined with the
lower-risk API/GUI/docs/guard work in the same sitting.
