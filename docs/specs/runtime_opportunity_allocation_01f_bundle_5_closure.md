# RUNTIME-OPPORTUNITY-ALLOCATION-01 — Phase F(inal): Bundle 5 Closure

Disposition (as originally written by the 9-commit Phase A–H patch this
document indexes):

```
RUNTIME-OPPORTUNITY-ALLOCATION-01:
IMPLEMENTATION AND CLOSURE PROOF COMPLETE —
AWAITING CHATGPT AND OPERATOR ACCEPTANCE
```

**Superseded — read this first.** Independent source review of this exact
patch found three authority defects (exact strategy-bar price source,
wall-clock-based cycle identity, a duplicated/shallower snapshot-authority
check) plus two premarket script-guard failures, closed by
`RUNTIME-OPPORTUNITY-ALLOCATION-01-READINESS-AND-AUTHORITY-REPAIR-01` — see
the Master Patch Ledger entry of that name for the authoritative current
status and the repair's own test/guard evidence. **Bundle 5 was not accepted
at the disposition above**, and this document's evidence table below
(current as of the original 9 commits) is retained as a historical record of
what was proven at that point, not as the current closure state. Where this
document's phase narrative below describes price sourcing or cycle
identity, the "Dependency-cone expansions" note further down is the specific
claim the repair corrected — treat `01c`/`01d` (already updated) as the
current truth for those two topics, not this file.

This document is the single evidence index for the original Bundle 5 patch.
It does not repeat the per-phase design rationale already in `01a`–`01e`; it
records what was built, where, and how it was proven, so a future reader (or
the acceptance reviewer) does not have to reconstruct that from commit
messages alone.

## Source / branch

- Source main SHA: `5355c579` (`docs: accept bundle 4 and authorize
  supervised paper soak`), pushed to `origin/main` before this branch was
  cut (local main was 1 commit ahead of origin at session start; the user
  explicitly authorized pushing it first).
- Worktree: `C:\Users\Zacha\Desktop\MiniQuantDeskV4-bundle5`
- Branch: `bundle/5-runtime-opportunity-allocation-01`
- 9 commits, no merge into `main`, no push of this branch.

## Files changed, by phase

| Phase | Files |
|---|---|
| A | `docs/specs/runtime_opportunity_allocation_01a_current_truth_and_contract.md` |
| B | `research-py/src/mqk_research/scanner/runtime_opportunity_artifact.py`, `research-py/tests/test_runtime_opportunity_artifact.py`, `core-rs/crates/mqk-daemon/src/runtime_opportunity_artifact.rs` |
| C | `core-rs/crates/mqk-portfolio/src/allocator.rs` |
| D | `core-rs/crates/mqk-portfolio/src/cycle.rs`, `core-rs/crates/mqk-portfolio/src/lib.rs` |
| E | `core-rs/crates/mqk-daemon/src/runtime_opportunity_mode.rs` |
| F | `core-rs/crates/mqk-daemon/src/runtime_opportunity_allocation.rs`, `core-rs/crates/mqk-daemon/src/state/loop_runner.rs`, `core-rs/crates/mqk-daemon/src/lib.rs` |
| G | `core-rs/crates/mqk-db/migrations/0055_runtime_opportunity_allocation_plans.sql`, `core-rs/crates/mqk-db/migrations/manifest.json`, `core-rs/crates/mqk-db/src/runtime_opportunity_allocation.rs`, `core-rs/crates/mqk-db/src/lib.rs`, `core-rs/crates/mqk-db/tests/scenario_runtime_opportunity_allocation_store_01.rs` |
| H (API) | `core-rs/crates/mqk-daemon/src/routes/portfolio_allocation.rs`, `core-rs/crates/mqk-daemon/src/routes.rs`, `core-rs/crates/mqk-daemon/tests/scenario_runtime_opportunity_allocation_api_01.rs` |
| H (GUI) | `core-rs/mqk-gui/src/features/system/{RuntimeOpportunityAllocationPanel.tsx,runtimeOpportunityAllocation.ts,runtimeOpportunityAllocation.test.ts,types/allocation.ts,api.ts,systemStatusSections.ts,systemStatusSections.test.ts}`, `core-rs/mqk-gui/src/features/settings/SettingsScreen.tsx`, `core-rs/mqk-gui/package.json` |
| Guards/docs | `scripts/guards/check_runtime_opportunity_allocation_01.sh`, `docs/specs/runtime_opportunity_allocation_01{b,c,d,e,f}*.md`, `MiniQuantDesk_Master_Patch_Ledger_v2.md`, `README.md`, `README_TECHNICAL.md` |

**Dependency-cone expansions beyond the task's starting list**, each because
a direct caller/type/test required it (all reported at the point of
discovery, none silent):

- `core-rs/crates/mqk-db/src/paper_portfolio.rs` (read functions only, not
  modified) — needed to resolve the active durable snapshot.
- `core-rs/crates/mqk-daemon/src/routes/durable_portfolio.rs` (`resolve_run`/
  `RunResolution`, reused not modified) — needed for Phase H's `?run_id=`
  resolution to match the existing convention exactly rather than
  reimplementing it.
- `core-rs/crates/mqk-strategy/src/types.rs` (`StrategyBarResult`, read
  only) — confirmed no completed-bar price is exposed on the primary
  dispatch result, which is why the original Phase F fetched price via a
  second `fetch_recent_completed_bars_for_strategy` call rather than
  carrying the exact evaluated bar forward. **Corrected by
  RUNTIME-OPPORTUNITY-ALLOCATION-01-READINESS-AND-AUTHORITY-REPAIR-01
  (Phase A)**: this second fetch was exactly the authority defect the
  repair closed — a newer bar could land in the DB between strategy
  evaluation and allocation, silently pricing a decision off a bar the
  strategy never saw. The repair widened `state.rs`'s dispatch seam to
  return the exact `EvaluatedBarFacts` (symbol/strategy_id/timeframe/
  bar_end_ts/close_micros) alongside each `StrategyBarResult`, so the price
  used for allocation is always the exact bar the strategy actually
  evaluated — never a second, independently-timed fetch.
- `core-rs/mqk-gui/src/features/system/durablePortfolio.ts` (read only) —
  template for the GUI fail-closed parsing pattern (`api.ts`,
  `systemStatusSections.ts`, `InstrumentRegistryV2SourcePanel.tsx` likewise
  read as templates, not modified beyond the necessary wiring edits already
  listed above).

**Unexpected files encountered, not part of this patch**: `smoke_logs/` and
`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` (stale, untracked leftovers
in the primary `main` worktree, found during the initial state check;
neither touched).

## Test/proof summary

| Lane | Result |
|---|---|
| Allocator (mqk-portfolio) | 24 new + 78 existing = 102 pass |
| Cycle model (mqk-portfolio) | 15 new; 117 total pass |
| Artifact — Python | 30 new; 986 total research-py pass |
| Artifact — Rust | 25 new pass |
| Runtime mode | 9 new pass |
| Runtime batching/apply | 12 new pass |
| Plan→DB-row conversion | 8 new pass |
| Durable store (DB-backed, port 5434) | 3 new pass |
| API (7 in-process + 6 DB-backed) | 13 new pass |
| GUI parser | 16 new; 922 total GUI suite pass |
| Regression: multi-symbol ×6, daemon routes, GUI contract gate, internal decision, budget gate, durable portfolio ×2, autonomous daily ops API | all pass unchanged (DB-backed, port 5434) |
| Full mqk-daemon lib suite | 424/426 (2 pre-existing, unrelated `alpaca_ws_transport` migration-drift failures — see below) |
| `cargo clippy -D warnings` (mqk-portfolio, mqk-db, mqk-daemon lib) | clean |
| `cargo fmt` | applied to every touched file; unrelated pre-existing drift in sibling files reverted, not committed |
| `npm run build` (tsc + vite) | clean |
| `check_migration_governance.sh` | pass |
| `check_runtime_opportunity_allocation_01.sh` | pass (11/11) |

**The 2 unrelated failures**: `state::alpaca_ws_transport::tests::
brk00r05b_s5_db_backed_restart_repair_sets_recovery_truth` and
`ws_truth_oa01_db_gap_cursor_persisted_after_disconnect` fail with "migration
6 was previously applied but has been modified" — a pre-existing drift on
the shared port-5434 test database (used across many concurrent
worktrees/branches on this machine), not caused by this bundle: no file this
bundle touches is a migration below `0055`, and this bundle's own migration
applies and is exercised cleanly (Phase G's tests, and every other DB-backed
test in this closure, ran against the identical database).

## Authority / scope discipline (self-check against the task's constraints)

- AI consumed: **NO** (guard-verified: no AI/ML framework or provider string
  in any Bundle 5 file).
- Order authority changed beyond a deterministic pre-buy constraint: **NO**
  — `submit_internal_strategy_decision` is called unchanged; only the batch
  of decisions reaching it can be narrowed.
- Risk/portfolio/P&L/broker-adapter authority changed: **NO** — no file in
  `mqk-risk`, `mqk-execution`'s gateway, `mqk-broker-alpaca`, or the durable
  portfolio/accounting write path was touched.
- Bundle 6 started: **NO** (guard-verified).
- Live capital enabled: **NO** — the live hard-lock is unit-tested from both
  the `LiveCapital`/`LiveShadow` and non-Alpaca-adapter directions.
- Primary `main` worktree / operating paper DB / Alpaca credentials / real
  daemon: untouched, unaccessed, unloaded, not started, respectively — all
  work happened in the separate worktree against the isolated port-5434 test
  database only.

## What remains for a future bundle (explicitly out of scope here)

- Live-in-browser verification of the mounted GUI panel (blocked by this
  session's own soak-isolation rule against starting the real daemon).
- Bundle 6 (multi-strategy conflict policy), Bundle 7 (dynamic
  strategy-symbol selection), Bundle 8 (watchdog/Discord/alerts expansion),
  Bundle 9 (autonomous paper soak/readiness closure) — all explicitly
  deferred, none started.
- Cap #5 (`aggregate_gross_exposure_cap_usd`) remains a design-only sketch
  in `docs/design/native_multi_symbol_dispatch.md`, unrelated to and
  untouched by Bundle 5.
