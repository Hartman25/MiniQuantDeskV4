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
| `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` | Not started (decision-only patch, not yet written) | Production-consumption-open (this *is* the boundary-crossing decision point) | Blocked on two of five `ASSET-CORE-01H` §5 prerequisites: (1) `BACKTEST-MULTIPLIER-MARGIN-01` closed — **satisfied**; (2) symbol/`instrument_id` translation layer — **satisfied** by `REGISTRY-V2-TRANSLATION-01A`-`01D` (pure fail-closed `RegistryV2SymbolTranslationIndex`, proven collision-free and round-trippable across the full 88-row equity universe, zero production callers); (3) Gate 0 / broker-submit routing-guard parity re-verification against `InstrumentRegistryV2::asset_class` — **now satisfied** by `REGISTRY-V2-GATE-PARITY-01A`-`01D` (pure fail-closed `registry_v2_gate_asset_class` helper, 20 regression tests proving Gate 0 and the routing guard reject the same asset classes whether keyed off `mqk_schemas::AssetClass` or `InstrumentRegistryV2::asset_class`, zero production callers, neither gate modified); (4) a live-network-verified non-equity market-data provider end-to-end into `md_bars` — open (requires live network calls, forbidden this session); (5) an explicit operator enablement decision for a named non-equity instrument — open. |

## 3. Next best patch

**Update (`REGISTRY-V2-TRANSLATION-01A`-`01D`, this session's later work):**
prerequisite #2 — the symbol/`instrument_id` translation layer between
`InstrumentRegistryV2` and the existing symbol-string-keyed tables
(`md_bars`, outbox rows, portfolio positions) — is now **satisfied**. A
pure, fail-closed `RegistryV2SymbolTranslationIndex` was built
(`core-rs/crates/mqk-md/src/instrument_registry_v2.rs`) and proven
collision-free and round-trippable across the full 88-row production
equity universe via a read-only CLI (`mqk md registry-v2-translation-check`);
see `docs/specs/registry_v2_translation_01d_closure_decision.md` for the
full closure decision. Zero production paths consume it.

**Update (`REGISTRY-V2-GATE-PARITY-01A`-`01D`, this session's later work):**
prerequisite #3 — Gate 0 / broker-submit routing-guard parity
re-verification against `InstrumentRegistryV2::asset_class` — is now
**satisfied**. A pure, fail-closed `registry_v2_gate_asset_class` helper was
built (`core-rs/crates/mqk-md/src/instrument_registry_v2.rs`) and 20
regression tests (`scenario_registry_v2_gate0_parity_01c.rs`,
`scenario_registry_v2_routing_guard_parity_01c.rs`) proved it classifies
every `InstrumentRegistryV2.asset_class` string identically to Gate 0's and
the routing guard's actual, already-tested behavior — equity allowed,
every other canonical class rejected, malformed/unknown input fails
closed, and `"rate"` (no `mqk_schemas::AssetClass` counterpart) is
unconstructable through the routing guard's closed enum. Neither gate was
modified; zero production paths consume the helper. See
`docs/specs/registry_v2_gate_parity_01d_closure_decision.md` for the full
closure decision.

**`REGISTRY-V2-LIVE-PROVIDER-PROOF-BOUNDARY-DECISION-01`** — prerequisite
#4's boundary decision — is now the next best value-per-risk patch:

- Prerequisites #1-#3 are now all satisfied; #4 (live-network-verified
  non-equity provider proof) and #5 (explicit operator enablement) are the
  only two remaining, and #4 must be settled first — enabling an instrument
  before its data source is live-network-proven would invert the
  checklist's own ordering.
- It is a decision/design patch (not code), naming the exact first
  non-equity provider and the exact network-call/operator-authorization
  boundary required before any live network call is made — consistent
  with this session's (and every prior session's) hard safety rule against
  making live network calls without an explicit boundary decision first.

**Do not** recommend `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` itself
next — two of its five prerequisites remain open; recommending it now
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
