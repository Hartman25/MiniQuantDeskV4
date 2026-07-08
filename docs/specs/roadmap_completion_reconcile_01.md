# Roadmap Completion Reconcile — 01

Patch ID: `ROADMAP-COMPLETION-RECONCILE-01`

Docs-only. Reconciles the multi-asset roadmap
(`docs/audits/multi_asset_completion_audit.md`) after this session's
`BACKTEST-MULTIPLIER-MARGIN-01` audit/closure work
(`BACKTEST-MULTIPLIER-MARGIN-01-COMPLETION-AUDIT-01` →
`-SAFE-GAP-CLOSURE-01` → `-CLOSURE-OR-BOUNDARY-DECISION-01`). No code
changed by this document. Percentages below are carried from
`multi_asset_completion_audit.md`'s existing per-item table except where
this session found direct repo evidence of drift, in which case the
evidence is cited inline — this doc does not re-derive a fresh audit for
items outside this session's scope.

---

## 1. Status categories used below

- **Closed foundation** — the item's own stated scope (schema/model/validator/docs, not production consumption) is done.
- **Backtest-complete** — multiplier/margin economics work in the backtest lane only; live/paper accounting is explicitly untouched.
- **Production-consumption-open** — the foundation exists but no trading/execution/risk/OMS/ingestion path reads it as truth.
- **Missing execution/risk/strategy** — no execution, risk, or strategy code exists for this asset class at all.

## 2. Per-item status

| Item | Status | Category | Exact blocker |
|---|---|---|---|
| `ASSET-CORE-01` | `CLOSED_LOCAL / FOUNDATION-COMPLETE` | Closed foundation | None for its own scope. Production consumption is a separate, explicitly-named boundary (`ASSET-CORE-01H`) — no trading/execution/risk/OMS/ingestion path reads `InstrumentRegistryV2`; v1 `equities.json` remains sole trading truth. |
| `ASSET-CORE-02` (Multi-Asset Order Intent Model) | `PARTIAL` (~25%, unchanged this session — outside its scope) | Production-consumption-open | `OrderIntentV2`/`ExecutionIntentV2` exist but are explicitly unwired (per `multi_asset_completion_audit.md` §5); zero bracket/OCO logic. Not touched by this session — `mqk-execution/*` was in this session's forbidden-file list. |
| `ASSET-CORE-03` (Asset-Aware Risk Router) | `PARTIAL` (~35%, unchanged this session — outside its scope) | Production-consumption-open | Two tested fail-closed gates exist (Gate 0, routing guard); zero graduated per-class policy behind them. Not touched — `mqk-risk/*` was forbidden this session. |
| `ASSET-CORE-04` (Multi-Asset Portfolio Ledger) | `PARTIAL` — dual-axis: live ledger ~20% (unchanged), read-only economics scaffold (`04A`-`04F`) substantially more complete | Production-consumption-open | The live-path ledger itself (`mqk-portfolio::accounting.rs`) is still single-currency, FIFO-P&L-multiplier-naive, and `PositionSnapshot.net_qty` is a whole-unit `i64` — fractional crypto quantities are not constructible through any route. Separately, an additive, zero-live-caller economics model chain (`ASSET-CORE-04A` instrument economics model → `04B` registry-v2 bridge → `04C` multi-asset NAV aggregation → `04D` read-only status route → `04F` registry-v2 seam) is built, tested, and daemon-exposed, but none of it is wired into `mqk-portfolio`'s actual accounting path or any live/paper order flow. This session confirms (via the `BACKTEST-MULTIPLIER-MARGIN-01` closure decision, §5) that `mqk-portfolio` remains untouched by the backtest-economics lineage too — the two economics scaffolds (backtest and portfolio-status) are parallel and neither reaches live accounting. Not touched this session — `mqk-portfolio/*` was forbidden. |
| `ASSET-CORE-05` (Market Calendar & Session Provider) | `PARTIAL` (~35%, up from the ~30% on record before `ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED`) | Production-consumption-open | `MarketCalendarProvider` trait + fail-closed fallback + read-only session-profile diagnostics (`equity_us_regular`/`crypto_continuous`/`futures_globex`/`forex_24x5`, daemon route + GUI panel) exist and are tested, per the ledger's `ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED` entry. True per-instrument session routing, authoritative non-equity calendars, and any use of non-equity profiles in trading/admission remain unwired. Not touched this session. |
| `BACKTEST-MULTIPLIER-MARGIN-01` | `CLOSED_LOCAL / BACKTEST-COMPLETE` (closed this session) | Backtest-complete | None for backtest economics. Margin enforcement and a real non-equity production registry-v2 data source are explicitly deferred, separate items — not blockers to this label's own closed scope. See `docs/specs/backtest_multiplier_margin_01_closure_decision.md`. |
| `CRYPTO-REGISTRY-01` (Crypto asset registry) | `PARTIAL` (~28%, unchanged this session) | Production-consumption-open | Two disabled registry-v2 fixture rows (`BTC/USD`, `ETH/USD`) exist, validated, bridged through `ASSET-CORE-04B`; zero production registry-v2 callers anywhere. Not touched this session. |
| `CRYPTO-DATA-01` (24/7 market ingestion) | `PARTIAL` (~32%, unchanged this session) | Production-consumption-open | Local-CSV and DB-backed local-mark ingestion proven for `BTC/USD`/`ETH/USD`; a fixture-first Kraken OHLCV parser/adapter/CLI/sync/scheduler-readiness chain is built and read-only-status-surfaced, but the `kraken` provider remains disabled by default, no recurring ingestion or scheduler registration exists, and `sync-provider`/`ingest-provider` have no Kraken path. Not touched this session (would require live network calls, forbidden by this session's hard safety rules). |
| `CRYPTO-RISK-01` | `MISSING` (0%, unchanged) | Missing execution/risk/strategy | No spread-gate, no counterparty-risk model. Not touched — `mqk-risk/*` forbidden this session. |
| `CRYPTO-EXEC-01` | `MISSING` (0%, unchanged) | Missing execution/risk/strategy | Alpaca adapter never calls `/v2/crypto/*` (confirmed by direct source read, `mqk-broker-alpaca/src/lib.rs`). Not touched — `mqk-broker-*` forbidden this session. |
| `CRYPTO-STRAT-01` | `MISSING` (0%, unchanged) | Missing execution/risk/strategy | Depends on `CRYPTO-EXEC-01`; no strategy code exists. Not touched this session. |
| `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` | Not started (decision-only patch, not yet written) | Production-consumption-open (this *is* the boundary-crossing decision point) | Blocked on all five `ASSET-CORE-01H` §5 prerequisites: (1) `BACKTEST-MULTIPLIER-MARGIN-01` closed — **now satisfied by this session**; (2) symbol/`instrument_id` translation layer — open; (3) Gate 0 / broker-submit routing-guard parity re-verification against `InstrumentRegistryV2::asset_class` — open; (4) a live-network-verified non-equity market-data provider end-to-end into `md_bars` — open (requires live network calls, forbidden this session); (5) an explicit operator enablement decision for a named non-equity instrument — open. |

## 3. Next best patch

**`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01`'s prerequisite #2** —
**a symbol/`instrument_id` translation layer** between `InstrumentRegistryV2`
and the existing symbol-string-keyed tables (`md_bars`, outbox rows,
portfolio positions) — is the next best value-per-risk patch:

- It is a **pure, additive, backtest/tooling-adjacent** translation/lookup
  concern (v1 bare-ticker ↔ v2 `instrument_id`/pair-style `symbol`), provably
  scopeable without touching broker/risk/OMS/runtime behavior, matching the
  pattern every closed sub-slice in this roadmap has already followed.
- It directly satisfies prerequisite #2 named by `ASSET-CORE-01H` §5, moving
  `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` one item closer without
  crossing the production-consumption boundary itself.
- Prerequisite #3 (Gate 0 / routing-guard parity) is the next cheapest after
  that — it is a **regression-test-only** patch against code that already
  exists (`mqk_schemas::AssetClass` gates), requiring no new runtime
  behavior, only a parity proof.
- Prerequisites #4 (live non-equity provider verification) and #5 (explicit
  operator enablement) are **not** recommended next — #4 requires a live
  network call (forbidden under this session's hard safety rules and
  arguably every prior session's, since no prior patch in this entire
  roadmap has made one), and #5 is an operator decision that should not
  precede #2-#4 being settled.

**Do not** recommend `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` itself
next — four of its five prerequisites remain open; recommending it now
would be the exact "stale recommendation pointing to already-closed work"
pattern this document is required to avoid the opposite of (recommending
unready work as if it were ready).

Independent of the registry-v2 boundary entirely, `ASSET-CORE-05`'s
remaining per-instrument session-routing gap is the next-closest-to-done
independent item (~35%, cheapest structurally per the original audit's own
assessment), and remains available as a lower-risk parallel track that does
not touch the registry-v2 boundary at all.

---

## 4. What this reconciliation changed vs. what it left alone

**Changed (this session, with direct repo evidence):**
- `BACKTEST-MULTIPLIER-MARGIN-01`: `MISSING/0%` → `CLOSED_LOCAL / BACKTEST-COMPLETE` (Phase A/B/C this session).
- `ASSET-CORE-05`: percentage note updated (~30% → ~35%) citing the already-committed `ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED` entry, which predates this session but was not yet reflected in the roadmap table's percentage.
- `ASSET-CORE-04`: evidence column expanded to name the `04A`-`04F` additive economics scaffold explicitly (already-committed work, not previously distinguished in the roadmap table from the live-ledger gap it sits beside).

**Left alone (no new evidence gathered this session, forbidden files not touched):**
- `ASSET-CORE-02`, `ASSET-CORE-03` — percentages carried forward unchanged; `mqk-execution/*` and `mqk-risk/*` were outside this session's allowed file list.
- `CRYPTO-REGISTRY-01`, `CRYPTO-DATA-01`, `CRYPTO-RISK-01`, `CRYPTO-EXEC-01`, `CRYPTO-STRAT-01` — unchanged; closing any of these further requires either a live network call (forbidden) or broker/risk code changes (forbidden) this session.
- `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` — not started; correctly not recommended as the immediate next patch (§3).

No config flag was changed, no trading was enabled, no network or DB call
was made, and no broker/execution/risk/OMS/runtime/strategy/portfolio file
was touched by this reconciliation or by any patch in this session.
