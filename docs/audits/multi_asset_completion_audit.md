# MULTI-ASSET-COMPLETION-AUDIT-01

Audit-only patch. No production code, config, DB, or trading-path changes. Branch `main`, HEAD at audit time `40b3331` ("backtest: run jobs from md bars"). All findings are grounded in the current committed repo; where a finding came from a delegated research pass, the underlying file:line evidence was independently spot-verified by direct file reads before being included here.

---

## 1. Executive Summary

MiniQuantDesk V4 trades **one asset class today: US equities**, via a single broker (Alpaca, paper). That equity path is deep, tested, and operationally mature (hundreds of closed patches; see [`MiniQuantDesk_Master_Patch_Ledger_v2.md`](../../MiniQuantDesk_Master_Patch_Ledger_v2.md)). This audit does not re-litigate that maturity — it was explicitly out of scope.

The headline finding is better than a cold read of the mission brief would predict: **the repo already has a real, tested, fail-closed multi-asset admission boundary.** A forward-compatible `AssetClass` enum (`Equity/Option/Future/Crypto/Forex`) exists in `mqk-schemas`, and two independent gates — one at strategy-signal admission ("Gate 0", commit `653730f`), one at broker dispatch (`MULTI-ASSET-ROUTING-GUARD-01`, commit `ff2ae59`) — reject every non-equity order before it can reach a broker. Both gates are unit/integration-tested (20 tests total across `scenario_asset_class_scope_b8.rs` and `scenario_asset_class_guard_multi_asset_routing_guard_01.rs`). A machine-readable capability matrix (`GET /api/v1/system/metadata`, `ASSET-CAPABILITY-MATRIX-01`, commit `424f0de`) honestly reports every asset class as `enabled: false` except equities. A dedicated planning document, [`docs/specs/experimental/multi_asset_scaffold_01.md`](../specs/experimental/multi_asset_scaffold_01.md) (commit `0b622a5`), already lays out per-asset-class requirements for crypto/futures/options/forex in detail consistent with — and in places more detailed than — this audit's own findings.

What does **not** exist is anything past that boundary. No asset class beyond equities has a real instrument registry entry, market-data provider, risk model, portfolio accounting treatment, paper broker, or strategy. The `Instrument`/`ContractSpec`/`OrderSpec` types that *could* carry futures/options contract metadata (strike, expiry, multiplier, tick size) are defined in `mqk-schemas` but are **dead code outside their own crate** — zero callers anywhere in `mqk-execution`, `mqk-runtime`, `mqk-portfolio`, `mqk-risk`, `mqk-backtest`, or the GUI. Portfolio accounting and the risk engine are hardcoded to whole-share, single-currency, no-margin, multiplier-implicitly-1 semantics; neither has ever seen a non-equity instrument. The market calendar has a clean pluggable trait (`MarketCalendarProvider`) but only NYSE-shaped session states. ETFs already live in the equity instrument registry (30+ tickers: SPY, QQQ, TLT, GLD, XLK, etc.) and trade correctly today, but with **zero** sector, correlation, or rotation metadata — the entire value-add the user's mission asks for in the ETF phase is unbuilt.

**This audit's main course-correction on the user's own request**: building futures, options, crypto, and forex execution capability one-by-one, as the user's Phase 1–6 ordering implies, would repeat the "bolt it on as a one-off" pattern the repo's own ledger (§11, `DATA-MULTI-ASSET-MODEL-01`) already flagged as the wrong direction. The highest-leverage near-term work is finishing the **shared foundation** (unified instrument registry, multiplier-aware backtest P&L, asset-aware portfolio ledger, generalized calendar) and picking off **already-half-built, low-risk wins** (wiring dead `SectorConstraint` code, tagging existing ETF tickers) before any new asset class gets a broker connection. See §7/§8 for the reordered sequence and rationale.

---

## 2. Current Repo Baseline

- Branch: `main`. HEAD: `40b3331`. Tracked working tree: clean (`git diff --name-only` empty at audit start).
- Untracked at audit start: `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` (stray local draft, **not** git-tracked, **not** the canonical ledger — see §4) and `smoke_logs/` (generated artifacts). Neither was touched.
- Canonical ledger: [`MiniQuantDesk_Master_Patch_Ledger_v2.md`](../../MiniQuantDesk_Master_Patch_Ledger_v2.md) — git-tracked, last updated by commit `67bbc18`. A second tracked ledger, `ACTIVE_PATCH_LEDGER_20260425.md`, is an older (April 2026) snapshot superseded by the `_v2.md` file; this audit updates only `_v2.md`.
- No daemon was started. No provider/broker network calls were made. No DB migrations, broker code, or live-routing code were touched.

### Multi-asset-relevant code that already exists (pre-dates this audit)

| Component | Location | Commit |
|---|---|---|
| `AssetClass` enum (Equity/Option/Future/Crypto/Forex) + `Instrument`/`ContractSpec`/`OrderSpec`/`QtyMicros` | [`mqk-schemas/src/lib.rs:86-220`](../../core-rs/crates/mqk-schemas/src/lib.rs) | predates this audit |
| Gate 0 — signal-admission asset-class allowlist + `asset_class_scope: "equity_only"` on `/api/v1/system/status` | `mqk-daemon` signal validation; tested in [`scenario_asset_class_scope_b8.rs`](../../core-rs/crates/mqk-daemon/tests/scenario_asset_class_scope_b8.rs) (12 tests, AS-01..AS-12) | `653730f` |
| `MULTI-ASSET-ROUTING-GUARD-01` — broker-submit asset-class gate (`GateRefusal::AssetClassDisabled`) | [`mqk-execution/src/gateway.rs:375-381`](../../core-rs/crates/mqk-execution/src/gateway.rs); tested in [`scenario_asset_class_guard_multi_asset_routing_guard_01.rs`](../../core-rs/crates/mqk-execution/tests/scenario_asset_class_guard_multi_asset_routing_guard_01.rs) (8 tests, G01-G08) | `ff2ae59` |
| `DISABLED-ASSET-GATE-TESTS-01` — outbox-payload-level rejection of disabled asset classes | routing layer | `6fe1697` |
| `ASSET-CAPABILITY-MATRIX-01` — `GET /api/v1/system/metadata` static per-asset-class matrix (`enabled`/`paper_ready`/`live_ready`/`broker_adapter`) | [`mqk-daemon/src/routes/system.rs:1101-1170`](../../core-rs/crates/mqk-daemon/src/routes/system.rs); contract-gated in `scenario_gui_daemon_contract_gate.rs` | `424f0de` |
| `RESEARCH-NON-EQ-01` — `OrderIntentV2`/`ExecutionIntentV2`/`equity_instrument()` V2 scaffold, explicitly documented as not wired into MAIN | [`mqk-execution/src/types.rs:95-178`](../../core-rs/crates/mqk-execution/src/types.rs), [`lib.rs:25-34`](../../core-rs/crates/mqk-execution/src/lib.rs) | predates this audit |
| `MULTI-ASSET-SCAFFOLD-01` — planning doc for crypto/futures/options/forex promotion lanes | [`docs/specs/experimental/multi_asset_scaffold_01.md`](../specs/experimental/multi_asset_scaffold_01.md) | `0b622a5` |
| `MarketCalendarProvider` trait + `ExchangeSourcedCalendarProvider`/`NyseWeekdaysProvider`/`FixedWindowOverrideProvider` (fail-closed `Unknown` session state) | [`mqk-daemon/src/state/market_calendar.rs`](../../core-rs/crates/mqk-daemon/src/state/market_calendar.rs) | predates this audit |
| `SectorConstraint` + `check_sector_limits()` — written, zero callers | [`mqk-portfolio/src/constraints.rs:203-277`](../../core-rs/crates/mqk-portfolio/src/constraints.rs) | predates this audit |

**Important finding — the scaffold doc is now stale on its own status table.** `multi_asset_scaffold_01.md` lists `ASSET-CAPABILITY-MATRIX-01`, `MULTI-ASSET-ROUTING-GUARD-01`, and `DISABLED-ASSET-GATE-TESTS-01` under "Future Patch Lane IDs (not yet created)" — but all three have since shipped (commits above, all ancestors of current `HEAD`, confirmed via `git merge-base --is-ancestor`). This is not a defect in the doc (it predates those patches), but it means a reader of that doc today would underestimate what already exists. See §13 for the recommended fix (a trivial docs-only patch).

**Architecture-debt finding — two non-identical asset-class enums exist.** `mqk_schemas::AssetClass` (`Equity, Option, Future, Crypto, Forex`) and `mqk_md::provider::ProviderAssetClass` (`Equity, Etf, Crypto, Futures, Options, Forex`) both exist, with different variant names (`Option` vs `Options`, `Future` vs `Futures`) and different variant sets (only `ProviderAssetClass` has `Etf`). No conversion or shared source between them today. This should be collapsed or given an explicit, exhaustiveness-tested mapping as part of `ASSET-CORE-01` (§9), or the two enums will drift further as more code is written against each.

---

## 3. Asset-Class Completion Percentages

Conservative, evidence-weighted (instrument model, data ingestion, backtest, risk, portfolio accounting, execution/paper broker, strategy, GUI/operator visibility, tests — per the rubric in §0 of the mission brief). Equities is reported for context only, per instruction not to re-audit it deeply.

| Asset Class | Completion | Basis |
|---|---|---|
| **Stocks (equities)** | **~85%** | Not re-audited in depth (out of scope by design). Consistent with prior repo audit (`FULL-REPO-COMPLETION-AUDIT-01`, memory). Full execution/risk/reconcile/backtest/GUI chain, hundreds of closed patches. |
| **ETFs** | **35%** | All major tracked ETFs (SPY, QQQ, IWM, XLK, XLF, XLE, TLT, IEF, GLD, etc. — 30+ tickers) already trade correctly through the equity pipeline (data, backtest, execution, GUI all "just work" because nothing distinguishes an ETF from a stock). **0%** on the actual ETF-phase deliverables: no `asset_class:"etf"` tag, no sector/category metadata, no correlation control, no rotation ranking, no risk-on/risk-off allocator. |
| **Futures** | **3%** | `ContractSpec::Future{root,expiry,multiplier,tick_size_micros}` shape exists in `mqk-schemas`, unused. Zero instrument data, zero contract spec table, zero continuous-contract/roll logic, zero margin model, zero broker adapter, zero backtest multiplier support, zero strategy awareness. Repo-wide grep for ES/MES/NQ/MNQ/RTY/CL/GC/ZN/ZB/6E/Globex/CME returns zero hits outside the planning doc. |
| **Options** | **5%** | `ContractSpec::Option{underlying,expiry,strike,right,multiplier}` shape exists, unused. Zero chain ingestion, zero Greeks, zero assignment/expiration handling, zero spread model, zero options-specific risk gates. Alpaca adapter calls only `/v2/orders`, `/v2/positions`, `/v2/account`, `/v2/account/activities` — never an options endpoint. |
| **Crypto** | **8%** | `AssetClass::Crypto` + `ContractSpec::Crypto` unit variant exist. `config/providers/providers.json` *declares* TwelveData supports `crypto` (metadata only — `implementation_status: "implemented_equity_provider"`, i.e. not actually implemented for crypto). `QtyMicros` fixed-point type was designed with fractional crypto quantities in mind but has no real caller. Zero 24/7 session model, zero pair registry, zero adapter, zero paper broker. |
| **Forex** | **2%** | `AssetClass::Forex` variant exists and is rejected at both gates. Provider config lists `forex` as a TwelveData/AlphaVantage/Polygon/yfinance capability — all unverified candidates, none wired. No pip/lot model, no leverage model, no session model, no macro-event calendar beyond the existing equity-only earnings/event-risk blackout. |
| **Fixed Income / Rates** | **5%** | TLT/IEF/SHY already trade as plain equities (same 0% distinction problem as ETFs generally). Zero yield-curve ingestion, zero duration model, zero rate-shock control, zero FOMC/CPI macro-event handling (existing event-risk blackout is earnings/symbol-scoped only). Zero ZN/ZB/ZT futures instruments of any kind. |

---

## 4. Architecture Gap Matrix

Against the mission's "Architectural Principles To Audit Against."

### Unified Instrument Model

| Field | Status | Evidence |
|---|---|---|
| `asset_class` | PARTIAL | `mqk_schemas::Instrument.asset_class` exists; unused outside schema crate. Two divergent enum definitions (see §2). |
| `exchange` / venue | PARTIAL | `Instrument.venue: Option<String>`; equities.json has `venue` per ticker (e.g. `"NASDAQ"`). Not validated against a real venue/calendar registry. |
| `currency` | PARTIAL | Field exists on `Instrument` and in config; never used for cross-currency conversion or exposure (single-currency USD assumption throughout). |
| `tick_size` | PARTIAL | `ContractSpec::Future.tick_size_micros` field shape exists; never populated, never read. |
| `lot_size` | MISSING | No field anywhere. |
| `contract_multiplier` | PARTIAL | `ContractSpec::Future.multiplier` / `ContractSpec::Option.multiplier` field shapes exist; never populated, never applied to any P&L calculation (confirmed: `mqk-portfolio/src/accounting.rs` fill math is always `qty * price_delta`, multiplier implicitly 1). |
| `session_calendar` | MISSING | No field on `Instrument`; calendar is a process-wide singleton concept (`MarketCalendarProvider`), not per-instrument. |
| `settlement_type` | MISSING | No field, no concept anywhere. |
| `margin_requirements` | MISSING | No field, no model anywhere. |
| `expiration` | PARTIAL | `ContractSpec::Option.expiry_yyyymmdd` / `ContractSpec::Future.expiry_yyyymm` field shapes exist; never populated, no expiry-event handling exists. |

### Unified Order Model

| Type | Status | Evidence |
|---|---|---|
| Market / Limit / Stop / Stop-Limit | PARTIAL | `mqk_schemas::OrderType{Market,Limit,Stop,StopLimit}` exists but unused; the *production* type is `mqk-execution::types::Side` + a free-form `order_type: String` consumed by the Alpaca adapter. Functionally equivalent coverage today (Alpaca path supports market/limit/stop/stop-limit), just not modeled as a shared enum. |
| Bracket | MISSING | No bracket-order concept anywhere in the codebase. |
| OCO | MISSING | No OCO concept anywhere in the codebase. |
| Asset-specific extensions (multi-leg, MOC/MOO, etc.) | MISSING | Nothing beyond the four basic types above. |

### Unified Portfolio Model

| Element | Status | Evidence |
|---|---|---|
| Cash balances | EXISTS (equity-only) | `PortfolioState.cash_micros: i64` — single balance, implicit USD. |
| Margin balances | MISSING | No field, no model. |
| Buying power | MISSING | Not modeled; risk/execution use raw cash. |
| Realized P/L | EXISTS (equity-only) | FIFO lot accounting in `mqk-portfolio/src/accounting.rs`; multiplier-naive. |
| Unrealized P/L | EXISTS (equity-only) | Per-lot mark-to-market in `metrics.rs`; multiplier-naive. |
| Cross-asset exposure | MISSING | Exposure aggregation is per-symbol notional only; no asset-class grouping. |
| Currency exposure | MISSING | Not tracked anywhere. |
| Contract-multiplier adjustments | MISSING | Confirmed absent from both backtest and live P&L paths. |
| NAV calculation | EXISTS (equity-only) | `cash + Σ(qty * mark)`, i128-overflow-guarded; assumes multiplier=1 for every position. |

### Unified Risk Framework

| Element | Status | Evidence |
|---|---|---|
| Portfolio-level risk (drawdown, daily loss) | EXISTS (equity-only) | `mqk-risk/src/engine.rs` — peak-equity drawdown + day-start daily-loss limits, deterministic `day_id` rollover. |
| Symbol risk | MISSING | No per-symbol limits beyond capital allocation sizing. |
| Sector risk | PARTIAL (dead code) | `SectorConstraint` + `check_sector_limits()` written, zero callers anywhere. |
| Margin risk | MISSING | No margin model to gate against. |
| Exposure limits | EXISTS (equity-only) | Max gross exposure check in `metrics.rs`; no asset-class-specific caps. |
| Drawdown limits | EXISTS (equity-only) | See above. |
| Daily loss limits | EXISTS (equity-only) | See above. |
| Session restrictions | PARTIAL | Calendar/session truth exists (`MarketCalendarProvider`) but is not consulted by the risk engine's gate evaluation itself. |
| Asset-specific risk gates | PARTIAL (admission-only) | The only asset-aware risk behavior today is binary reject-if-not-equity at Gate 0 and at broker submit. No graduated, per-class policy exists behind that gate. |

---

## 5. Patch-by-Patch Status Table

Difficulty: S (days) / M (1-2 weeks) / L (3-6 weeks) / XL (6+ weeks, likely multi-patch). "Order" is the global recommended sequence position from §7/§8 (DONE items are foundation, not sequenced).

### Already CLOSED (foundation, pre-dates this audit)

| Patch ID | Status | Completion | Evidence | Notes |
|---|---|---|---|---|
| `ASSET-CLASS-SCOPE-DECLARATION-01` ("B8") | CLOSED | 100% (of its own narrow scope) | `653730f`; 12 tests | Gate 0 signal-admission allowlist + honest `asset_class_scope` status field. |
| `MULTI-ASSET-ROUTING-GUARD-01` | CLOSED | 100% | `ff2ae59`; 8 tests | Broker-submit asset-class reject gate. |
| `DISABLED-ASSET-GATE-TESTS-01` | CLOSED | 100% | `6fe1697` | Outbox-payload-level rejection proof. |
| `ASSET-CAPABILITY-MATRIX-01` | CLOSED | 100% (backend) | `424f0de`; contract-gated | Backend done + tested; **not yet rendered in the GUI** (no TS consumer found) — would need a small follow-up to reach 90%+. |
| `MULTI-ASSET-SCAFFOLD-01` | CLOSED (docs) | 100% (as a planning doc) | `0b622a5` | Status table (§2) refreshed by `LEDGER-MULTI-ASSET-RECONCILE-01` (closed, §17). |
| `RESEARCH-NON-EQ-01` | CLOSED/PARKED | n/a (intentionally inert) | `mqk-execution/src/types.rs:95-178` | Deliberate, documented, isolated scaffold. Correct as-is; do not wire without a scope-reviewed patch (per its own comment). |

### Phase 0 — Core Multi-Asset Foundation

| Patch ID | Status | Completion | Difficulty | Dependencies | Evidence | Order | Notes |
|---|---|---|---|---|---|---|---|
| `ASSET-CORE-01` Unified Instrument Registry v2 | PARTIAL | 25% | L | none | `mqk-schemas::Instrument`/`ContractSpec` isolated; `equities.json` single-asset-class, no sector/lot/margin fields | 4 | Should also resolve the two-enum split (§2). |
| `ASSET-CORE-02` Multi-Asset Order Intent Model | `CLOSED_LOCAL / MODEL-COMPLETE` (own model scope only; see §90) | ~35% (up from 25%) | L | `ASSET-CORE-01` | `OrderIntentV2`/`ExecutionIntentV2` exist, explicitly unwired (`RESEARCH-NON-EQ-01`); bracket/OCO is now structurally modeled (`BracketLegs`) as always-non-routable (`ASSET-CORE-02-03-COMPLETION-SWEEP-01-COMBINED`) | 7 | Wiring this is itself a scope-reviewed patch per its own code comment. |
| `ASSET-CORE-03` Asset-Aware Risk Router | `CLOSED_LOCAL / POLICY-BOUNDARY-COMPLETE` (own policy-boundary scope only; see §90) | ~45% (up from 35%) | L | `ASSET-CORE-01`, `ASSET-CORE-04` | Two tested fail-closed gates exist (Gate 0, routing guard) plus a graduated per-class static policy model (`mqk_execution::asset_risk_policy`, corrects prior "zero graduated per-class policy" note), now operator-surfaced read-only via `GET /api/v1/system/asset-risk-policy` | 8 | Most mature of the five — true live enforcement still needs `ASSET-CORE-04`. |
| `ASSET-CORE-04` Multi-Asset Portfolio Ledger | MISSING (effectively, live path) / PARTIAL (read-only economics scaffold) | 20% (live ledger, unchanged) — see §78 for the separate `04A`-`04F` scaffold | XL | none | `cash_micros` single-currency; FIFO P&L multiplier-naive; `PositionSnapshot.net_qty` whole-unit `i64` (no fractional crypto qty); no margin/NAV-by-asset-class in the live path. A parallel, zero-live-caller economics model+bridge+aggregation+status chain (`04A`-`04F`) exists and is tested but does not touch `mqk-portfolio::accounting.rs` | 14 | Highest-risk patch in this entire roadmap — touches live capital accounting invariants. Needs its own scenario-test proof standard per `audit_repo_truth_rules.md`. |
| `ASSET-CORE-05` Market Calendar & Session Provider | PARTIAL | ~38% (up from 35%; see §89) | M | none | `MarketCalendarProvider` trait + 3 providers exist and are fail-closed/pluggable; `MarketSessionState` enum is NYSE-vocabulary-coupled (no 24/7, no Globex RTH/ETH, no FX session windows); `ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED` added read-only session-profile diagnostics (4 profiles, daemon route + GUI panel); `ASSET-CORE-05-PER-INSTRUMENT-SESSION-ROUTING-01-COMBINED` since closed the composed-router/"rate"-gap/parity-test model-and-parity scope, still model-only/unwired into trading | 5 | Cheaper than it looks — the hard part (trait, fail-closed contract, fallback) is already built. |

### Phase 1 — Futures Trading Engine

| Patch ID | Status | Completion | Difficulty | Dependencies | Evidence | Order | Notes |
|---|---|---|---|---|---|---|---|
| `FUTURES-REGISTRY-01` Contract spec database | MISSING | 5% | M | `ASSET-CORE-01` | Schema shape only; zero contract data | 22 | |
| `FUTURES-DATA-01` Continuous futures builder | MISSING | 0% | L | `FUTURES-REGISTRY-01` | Zero roll/stitch logic anywhere | 23 | |
| `FUTURES-DATA-02` Intraday futures ingestion | MISSING | 0% | M | `FUTURES-REGISTRY-01`, `ASSET-CORE-05` | No adapter, no Globex session awareness | 24 | |
| `FUTURES-RISK-01` Futures-specific risk controls | MISSING | 0% | L | `ASSET-CORE-03`, `ASSET-CORE-04` | No margin-per-contract model | 25 | |
| `FUTURES-EXEC-01` Paper futures broker | MISSING | 0% | XL | `ASSET-CORE-04`, `FUTURES-REGISTRY-01`, `FUTURES-RISK-01`, `BROKER-IBKR-04` | Alpaca has no futures API; new broker required | 28 | |
| `FUTURES-STRAT-01` Trend-following engine | MISSING | 0% | M | `FUTURES-EXEC-01` | Existing `swing_momentum`/`volatility_breakout` equity engines could be retargeted once instrument model supports futures | 29 | |
| `FUTURES-STRAT-02` Mean-reversion engine | MISSING | 0% | M | `FUTURES-EXEC-01` | Existing `mean_reversion.rs` equity engine is a reuse candidate | 30 | |

### Phase 2 — Options Trading Engine

| Patch ID | Status | Completion | Difficulty | Dependencies | Evidence | Order | Notes |
|---|---|---|---|---|---|---|---|
| `OPTIONS-CONTRACT-01` Options contract registry | MISSING | 10% | M | `ASSET-CORE-01` | `ContractSpec::Option` shape exists, most fully specified of the unused schema variants | 31 | |
| `OPTIONS-CHAIN-01` Options chain ingestion | MISSING | 0% | L | `OPTIONS-CONTRACT-01`, `PROVIDER-SWAP-CONTRACT-01` | No chain/Greeks/IV/OI data anywhere; current pipeline is OHLCV-bars-only | 32 | |
| `OPTIONS-RISK-01` Options risk permissions | MISSING | 0% | L | `ASSET-CORE-03`, `ASSET-CORE-04`, `OPTIONS-CHAIN-01` | No Greeks-based sizing, no tier/level gating | 33 | |
| `OPTIONS-BACKTEST-01` Assignment simulator | MISSING | 0% | L | `BACKTEST-MULTIPLIER-MARGIN-01`, `OPTIONS-CHAIN-01` | No expiry/exercise/assignment handling anywhere | 35 | |
| `OPTIONS-WHEEL-01` Boring Wheel Scanner | MISSING | 0% | M | `OPTIONS-CHAIN-01`, `OPTIONS-BACKTEST-01` | Read-only research scanner — lower risk than execution engines | 36 | |
| `OPTIONS-WHEEL-02` Paper Wheel Engine | MISSING | 0% | L | `OPTIONS-RISK-01`, `BROKER-ALPACA-OPTIONS-01`, `OPTIONS-WHEEL-01` | Requires short-option margin enforcement first | 37 | |
| `OPTIONS-SPREADS-01` Defined-risk spread engine | MISSING | 0% | XL | `OPTIONS-RISK-01`, `BROKER-ALPACA-OPTIONS-01`, `OPTIONS-WHEEL-02` | Multi-leg orders; no OCO/multi-leg model exists at all yet | 38 | |

### Phase 3 — Crypto Engine

| Patch ID | Status | Completion | Difficulty | Dependencies | Evidence | Order | Notes |
|---|---|---|---|---|---|---|---|
| `CRYPTO-REGISTRY-01` Crypto asset registry | PARTIAL | 28% | S | `ASSET-CORE-01` | `CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01` added one real disabled `BTC/USD` registry-v2 fixture (`config/instruments/instruments_v2.crypto_local_marks.example.json`), validated, bridged through `ASSET-CORE-04B`; `CRYPTO-DATA-01G-ETHUSD-LOCAL-CSV-MARKS-01-COMBINED` added a second disabled `ETH/USD` row beside it, proving the registry-v2 fixture lane is not hardcoded to one symbol; still zero production registry-v2 callers anywhere | 15 | Cheapest non-equity asset class to stand up — spot only, no margin, no expiry. |
| `CRYPTO-DATA-01` 24/7 market ingestion | PARTIAL | 32% | M | `CRYPTO-REGISTRY-01`, `ASSET-CORE-05`, `PROVIDER-SWAP-CONTRACT-01` | `CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01` proved a real (non-fixture-economics) `BTC/USD` mark from a committed local CSV reaches the unmodified `ASSET-CORE-04A`/`04B`/`04C` model chain; `CRYPTO-DATA-01B-DB-BACKED-LOCAL-MARK-PERSISTENCE-01` extended that to a real DB-backed `md_bars` persistence + readback proof (no migration); `CRYPTO-DATA-01G-ETHUSD-LOCAL-CSV-MARKS-01-COMBINED` proved the same fixture/parser/model-chain/provider-metadata lane for a second symbol, `ETH/USD`; `CRYPTO-DATA-01U-V-W-KRAKEN-OHLCV-ADAPTER-PARSER-CLI-BUNDLE-01-COMBINED` added a fixture-first Kraken OHLCV parser + disabled `HistoricalProvider` adapter/factory arm + read-only CLI dry-run, with completion semantics and a documented/tested volume-scaling convention; `CRYPTO-DATA-01X-Y-KRAKEN-INGEST-PROVIDER-DB-PROOF-BUNDLE-01-COMBINED` added a fixture-first `mqk md kraken-ohlc-ingest` command and DB-backed proof that completed Kraken bars reach `md_bars` with truthful provider metadata, forming-row exclusion, and scaled-volume readback; `CRYPTO-DATA-01Z-AA-KRAKEN-INCREMENTAL-SYNC-DB-PROOF-BUNDLE-01-COMBINED` added a fixture-first `mqk md kraken-ohlc-sync` command proving pre-write inspection of existing `md_bars` state and presence-based missing/existing bar classification; `CRYPTO-DATA-01AB-AC-KRAKEN-CONTENT-DIFF-SYNC-BUNDLE-01-COMBINED` closed the content-diff gap `01Z-AA` left open with a new `mqk-db` read helper (`fetch_md_bars_for_provider_sync_keys`) and comparison helper (`provider_bar_matches_existing`), so `kraken-ohlc-sync` now truly skips unchanged rows (no write-helper call) while inserting missing and updating changed rows, proven with true idempotency and a `--no-update-existing` changed-vs-unchanged distinction; `CRYPTO-DATA-01AD-KRAKEN-SYNC-EVIDENCE-STATUS-ROUTE-01` added a read-only `GET /api/v1/market-data/kraken-ohlc/status` daemon route exposing that evidence (fail-closed safety checks, epoch-based file selection across the ingest/sync prefixes); `CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01` added a read-only "Kraken OHLCV sync status" GUI panel consuming that route (no GUI action can trigger a provider/CLI/scheduler call); `kraken` provider still disabled by default, no recurring ingestion, no scheduler, `sync-provider`/`ingest-provider` still have no Kraken path | 16 | |
| `CRYPTO-RISK-01` Crypto-specific risk controls | MISSING | 0% | M | `ASSET-CORE-03`, `ASSET-CORE-04` | No spread-gate, no counterparty-risk model | 17 | |
| `CRYPTO-EXEC-01` Crypto paper broker | MISSING | 0% | L | `ASSET-CORE-04`, `CRYPTO-DATA-01`, `CRYPTO-RISK-01` | Alpaca adapter never calls `/v2/crypto/*` (confirmed by direct read of `mqk-broker-alpaca/src/lib.rs`) | 18 | |
| `CRYPTO-STRAT-01` BTC/ETH trend engine | MISSING | 0% | M | `CRYPTO-EXEC-01` | | 19 | |

### Phase 4 — Forex Engine

| Patch ID | Status | Completion | Difficulty | Dependencies | Evidence | Order | Notes |
|---|---|---|---|---|---|---|---|
| `FX-FUTURES-01` Currency futures registry | MISSING | 5% | M | `ASSET-CORE-01` | `AssetClass::Forex` exists; no pip/lot/leverage model | 40 | |
| `FX-DATA-01` Currency futures ingestion | MISSING | 5% | M | `FX-FUTURES-01`, `ASSET-CORE-05`, `PROVIDER-SWAP-CONTRACT-01` | Provider metadata lists forex as unverified candidate capability only | 41 | |
| `FX-RISK-01` Macro event risk controls | MISSING | 0% | M | `ASSET-CORE-03`, `ASSET-CORE-04` | Existing event-risk blackout is equity-earnings-scoped only, no FOMC/CPI/NFP concept | 42 | |
| `FX-STRAT-01` Dollar trend engine | MISSING | 0% | M | `FX-DATA-01`, `FX-RISK-01` | | 43 | |

### Phase 5 — ETF & Sector Rotation Engine

| Patch ID | Status | Completion | Difficulty | Dependencies | Evidence | Order | Notes |
|---|---|---|---|---|---|---|---|
| `ETF-REGISTRY-01` ETF universe | PARTIAL | 30% | S | none | 30+ ETF tickers already in `equities.json`; zero `asset_class:"etf"` tag or sector field | 3 | Cheapest real win in the entire roadmap — pure config/tagging change. |
| `ETF-RANKER-01` Sector rotation ranking system | MISSING | 0% | M | `ETF-REGISTRY-01`, `ETF-RISK-01`, `MULTI-ASSET-ALLOCATOR-01` | No ranking/rotation code anywhere | 10 | |
| `ETF-STRAT-01` Risk-on/Risk-off allocator | MISSING | 0% | M | `ETF-RANKER-01`, `MULTI-ASSET-ALLOCATOR-01` | | 11 | |
| `ETF-RISK-01` Correlation exposure controls | CLOSED (`ETF-RISK-CLOSURE-01` + `ETF-RISK-EXTERNAL-SIGNAL-GATE-01`) | 100% | S | `PORTFOLIO-LIVE-WEIGHTS-01` | Live, bps-based `evaluate_sector_risk` wired pre-outbox on both the internal decision path (Gate 1h, `decision.rs`) and the external signal path (Gate 1i, `routes/strategy.rs`), sharing one mechanism via `capital_policy::sector_risk_gate`; default-off via `MQK_SECTOR_EXPOSURE_LIMITS_BPS` on both; see §16 closure note | 2 | Closed once the live mark/NAV/weight dependency (`PORTFOLIO-LIVE-WEIGHTS-01`) existed; external-path gap closed by a dedicated follow-up patch. |

### Phase 6 — Rates & Fixed Income

| Patch ID | Status | Completion | Difficulty | Dependencies | Evidence | Order | Notes |
|---|---|---|---|---|---|---|---|
| `RATES-REGISTRY-01` Treasury products | PARTIAL | 10% | S | `ASSET-CORE-01` | TLT/IEF/SHY already trade as plain equities; no ZN/ZB/ZT, no duration metadata | 44 | |
| `RATES-DATA-01` Yield curve ingestion | MISSING | 0% | L | `RATES-REGISTRY-01` | Zero yield-curve code anywhere | 45 | |
| `RATES-STRAT-01` Duration trend strategy | MISSING | 0% | M | `RATES-DATA-01`, `RATES-RISK-01` | | 47 | |
| `RATES-RISK-01` Rate shock controls | MISSING | 0% | M | `ASSET-CORE-03`, `ASSET-CORE-04` | No FOMC/CPI/rate-shock concept anywhere (grep: zero matches) | 46 | |

### Phase 7 — Broker Expansion Layer

| Patch ID | Status | Completion | Difficulty | Dependencies | Evidence | Order | Notes |
|---|---|---|---|---|---|---|---|
| `BROKER-IBKR-01` IBKR adapter skeleton | MISSING | 0% | M | none | Zero IBKR/ConId/TWS references anywhere in repo (exhaustive grep); `BrokerAdapter` trait is clean enough that this is genuinely M not L/XL | 20 | Trait seam is ready *now* — cheap relative to value once futures/forex are in scope. |
| `BROKER-IBKR-02` Contract discovery / ConId mapping | MISSING | 0% | M | `BROKER-IBKR-01` | | 21 | |
| `BROKER-IBKR-03` Read-only paper synchronization | MISSING | 0% | M | `BROKER-IBKR-01`, `BROKER-IBKR-02` | Current `broker_baseline.rs` snapshot model is tightly coupled to `AlpacaBrokerAdapter` instantiation, not broker-agnostic | 26 | |
| `BROKER-IBKR-04` Paper order routing | MISSING | 0% | L | `BROKER-IBKR-01..03` | | 27 | |
| `BROKER-ALPACA-OPTIONS-01` Alpaca options support | MISSING | 0% | L | `OPTIONS-CONTRACT-01` | Adapter only calls equity endpoints (`/v2/orders`, `/v2/positions`, `/v2/account*`) — confirmed by direct source read | 34 | |
| `BROKER-ALPACA-CRYPTO-01` Alpaca crypto support | MISSING | 0% | M | `CRYPTO-REGISTRY-01` | Same — zero `/v2/crypto/*` calls | 39 | |

### Additional foundation patches identified by this audit

| Patch ID | Status | Completion | Difficulty | Dependencies | Evidence | Order | Notes |
|---|---|---|---|---|---|---|---|
| `BACKTEST-MULTIPLIER-MARGIN-01` | `CLOSED_LOCAL / BACKTEST-COMPLETE` (backtest economics only; see §77) | 100% (backtest economics scope); margin enforcement and production registry-v2 data source explicitly out of scope | L | `ASSET-CORE-01` | Twelve `CLOSED_LOCAL` sub-slices deliver a pure multiplier-aware economics model, engine wiring, economics-sensitive `run_id`, `metrics.json`/`report.md`/`manifest.json` economics (all entry points including `csv-sweep`), CLI (`csv`/`db`/`csv-sweep`) flags, daemon job request economics, GUI controls, and a registry-v2 economics-suggestion seam; `mqk-portfolio/src/accounting.rs` (live/paper fill math) remains untouched by design — `mqk-backtest` is not a dependency of `mqk-runtime`. See `docs/specs/backtest_multiplier_margin_01_closure_decision.md`. | 6 | Prerequisite #1 for `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` is now satisfied; the other four prerequisites remain open. |
| `MULTI-ASSET-ALLOCATOR-01` | PARTIAL | 50% (as a generic allocator) / 0% (as multi-asset) | M | `ASSET-CORE-01` | `mqk-portfolio/src/allocator.rs` is a complete, tested, deterministic weight-normalization allocator — but has zero callers in `mqk-runtime`/`mqk-daemon` and zero asset-class awareness in its candidate type | 9 | Wiring + extending, not building from scratch. |
| `MULTI-STRATEGY-CONFLICT-POLICY-01` | MISSING | 0% | M | none | Today symbol→strategy is a structural 1:1 mapping, so conflicts are avoided by construction rather than resolved; zero conflict-detection code exists | 12 | Required before any "rank strategies per instrument" capability. |
| `PROVIDER-SWAP-CONTRACT-01` | PARTIAL | 40% | M | none | Already substantially covered by the prior `DATA-INGESTION-COVERAGE-AUDIT-01` audit: `MarketDataProvider` trait is capability-aware and asset-class-tagged (`ProviderAssetClass`), but the factory (`build_market_data_provider_from_config`) hardcodes match arms per provider — declaring a provider "crypto-capable" in JSON does not make it usable | 13 | Lower marginal audit value — already characterized; mainly needs the enum-unification work from `ASSET-CORE-01`. |
| `LEDGER-MULTI-ASSET-RECONCILE-01` | CLOSED | 100% | S | none | Ledger §11's five labels reconciled and mapped to this roadmap's Phase 0–4 patch IDs (preserved, not deleted); `multi_asset_scaffold_01.md`'s stale status table (§2) refreshed; see ledger §19 closure note and §17 below | 1 | Pure docs hygiene — zero code risk. Closed this patch. |

---

## 6. Dependency Graph

```text
ASSET-CORE-01 (Instrument Registry v2; unify the two AssetClass enums)
 ├─> FUTURES-REGISTRY-01 ─> FUTURES-DATA-01/02 ─> FUTURES-RISK-01 ─> FUTURES-EXEC-01 ─> FUTURES-STRAT-01/02
 ├─> OPTIONS-CONTRACT-01 ─> OPTIONS-CHAIN-01 ─> OPTIONS-RISK-01 ─┬─> OPTIONS-WHEEL-02 ─> OPTIONS-SPREADS-01
 │                                          OPTIONS-BACKTEST-01 ┴─> OPTIONS-WHEEL-01
 ├─> CRYPTO-REGISTRY-01 ─> CRYPTO-DATA-01 ─> CRYPTO-RISK-01 ─> CRYPTO-EXEC-01 ─> CRYPTO-STRAT-01
 ├─> FX-FUTURES-01 ─> FX-DATA-01 ─> FX-RISK-01 ─> FX-STRAT-01
 ├─> RATES-REGISTRY-01 ─> RATES-DATA-01 ─> RATES-STRAT-01
 │                        RATES-RISK-01 ──────────┘
 └─> ETF-REGISTRY-01 ─> ETF-RANKER-01 ─> ETF-STRAT-01
                          ETF-RISK-01 ──┘

ASSET-CORE-04 (Portfolio Ledger: margin/multiplier/currency/NAV)
 ├─> BACKTEST-MULTIPLIER-MARGIN-01  (also needs ASSET-CORE-01)
 ├─> ASSET-CORE-03 (real per-asset risk policy; also needs ASSET-CORE-01)
 ├─> FUTURES-RISK-01, OPTIONS-RISK-01, CRYPTO-RISK-01, FX-RISK-01, RATES-RISK-01
 └─> *-EXEC-01 / *-PAPER-01 patches for every non-equity class

ASSET-CORE-05 (Calendar/Session generalization)
 ├─> CRYPTO-DATA-01 (24/7)
 ├─> FUTURES-DATA-02 (Globex RTH/ETH)
 └─> FX-DATA-01 (24/5 London/NY/Tokyo/Sydney)

PROVIDER-SWAP-CONTRACT-01 ─> CRYPTO-DATA-01, FX-DATA-01, OPTIONS-CHAIN-01

MULTI-ASSET-ALLOCATOR-01 ─┬─> ETF-RANKER-01 ─> ETF-STRAT-01
                          └─> MULTI-STRATEGY-CONFLICT-POLICY-01 (orchestration ranking goal)

BROKER-IBKR-01 ─> BROKER-IBKR-02 ─> BROKER-IBKR-03 ─> BROKER-IBKR-04 ─> FUTURES-EXEC-01 (one viable path)
BROKER-ALPACA-OPTIONS-01 ─> OPTIONS-WHEEL-02, OPTIONS-SPREADS-01  (alternative path for options specifically)
BROKER-ALPACA-CRYPTO-01 ──> CRYPTO-EXEC-01  (alternative/supplement to a from-scratch crypto adapter)

ETF-RISK-01, LEDGER-MULTI-ASSET-RECONCILE-01: no dependencies — buildable today, in isolation, with zero risk to the equity path.
```

---

## 7. 6–12 Month Build Sequence

Two-month blocks; each block assumes the prior block's patches are `CLOSED` (per `audit_repo_truth_rules.md` closure standard — committed code + passing tests, not "mostly done").

**Months 1–2 — Foundation, zero-risk wins, and de-risking docs**
`LEDGER-MULTI-ASSET-RECONCILE-01` → `ETF-RISK-01` → `ETF-REGISTRY-01` → `ASSET-CORE-01` (incl. enum unification) → `ASSET-CORE-05`.

**Months 2–4 — Finish the foundation**
`BACKTEST-MULTIPLIER-MARGIN-01` → `ASSET-CORE-02` → `ASSET-CORE-03` → `MULTI-ASSET-ALLOCATOR-01` → `ETF-RANKER-01` → `ETF-STRAT-01`.

**Months 4–5 — Orchestration groundwork**
`MULTI-STRATEGY-CONFLICT-POLICY-01` → `PROVIDER-SWAP-CONTRACT-01`.

**Months 5–6 — `ASSET-CORE-04` (the big one)**
Multi-Asset Portfolio Ledger. Sequenced alone in its own block deliberately: it is the highest-difficulty, highest-blast-radius patch in this roadmap (touches live capital accounting) and every subsequent asset-class risk/execution patch depends on it. Should not be parallelized with anything that also touches `mqk-portfolio`.

**Months 6–8 — Crypto lane (first live non-equity asset class)**
`CRYPTO-REGISTRY-01` → `CRYPTO-DATA-01` → `CRYPTO-RISK-01` → `CRYPTO-EXEC-01` (paper) → `CRYPTO-STRAT-01`. Crypto first because it is structurally the simplest non-equity class: spot only, no margin, no expiry, 24/7 (single new calendar variant, not a multi-session model).

**Months 8–10 — Broker expansion + Futures groundwork (parallel tracks)**
Track A: `BROKER-IBKR-01` → `BROKER-IBKR-02`. Track B: `FUTURES-REGISTRY-01` → `FUTURES-DATA-01`/`FUTURES-DATA-02`. These two tracks are independent and can run in parallel.

**Months 10–12 — Futures risk/execution + Options groundwork (parallel tracks)**
Track A: `FUTURES-RISK-01` → `BROKER-IBKR-03`/`04` → `FUTURES-EXEC-01` (paper). Track B: `OPTIONS-CONTRACT-01` → `OPTIONS-CHAIN-01`. Forex and Rates remain at the planning-doc stage (analogous to today's `multi_asset_scaffold_01.md`) through month 12 — deliberately not started in parallel with futures/options/crypto execution work, consistent with one-asset-class-at-a-time risk discipline.

---

## 8. Top 20 Highest-Value Patches

In recommended build order (full ordering and rationale in §5/§7). This list intentionally diverges from the mission brief's candidate list in a few places — rationale follows the table.

| # | Patch ID | Why it's top-20 |
|---|---|---|
| 1 | `LEDGER-MULTI-ASSET-RECONCILE-01` | Zero-risk docs fix; removes stale-status confusion immediately. |
| 2 | `ETF-RISK-01` | Code already written, zero callers — cheapest real risk-management win in the repo. |
| 3 | `ETF-REGISTRY-01` | Pure config tagging of tickers already in the registry. |
| 4 | `ASSET-CORE-01` | Every other asset class depends on this; also fixes the two-enum split. |
| 5 | `ASSET-CORE-05` | Trait/fail-closed contract already built; "just" needs new session variants. |
| 6 | `BACKTEST-MULTIPLIER-MARGIN-01` | Hard prerequisite for any futures/options backtest credibility. |
| 7 | `ASSET-CORE-02` | Unwiring the already-built `OrderIntentV2` scaffold. |
| 8 | `ASSET-CORE-03` | Highest existing investment (two tested gates) — finish what's started. |
| 9 | `MULTI-ASSET-ALLOCATOR-01` | Wiring an already-complete, tested allocator module, not building one. |
| 10 | `ETF-RANKER-01` | First real "rank instruments" capability the mission asks for. |
| 11 | `ETF-STRAT-01` | Natural payoff of 2/9/10. |
| 12 | `MULTI-STRATEGY-CONFLICT-POLICY-01` | Required before any multi-strategy-per-symbol ranking is safe. |
| 13 | `PROVIDER-SWAP-CONTRACT-01` | Unlocks every non-equity `*-DATA-01` patch. |
| 14 | `ASSET-CORE-04` | Biggest gap in the repo; everything execution-side for non-equity depends on it. |
| 15 | `CRYPTO-REGISTRY-01` | Cheapest non-equity asset class to start. |
| 16 | `CRYPTO-DATA-01` | First real non-equity data pipeline. |
| 17 | `CRYPTO-RISK-01` | First real non-equity risk model. |
| 18 | `CRYPTO-EXEC-01` | First real non-equity paper broker — the actual milestone the whole mission is aimed at. |
| 19 | `CRYPTO-STRAT-01` | Closes the first full non-equity vertical slice. |
| 20 | `BROKER-IBKR-01` | Trait seam is ready now; needed soon for futures/forex; cheap while the seam is fresh. |

**Why this differs from the brief's candidate list**: the brief's candidates (`FUTURES-REGISTRY-01`, `FUTURES-RISK-01`, `OPTIONS-CONTRACT-01`, `OPTIONS-CHAIN-01`, `OPTIONS-RISK-01`, `BROKER-IBKR-02`) are all real and all needed — they land at positions 21–33 (§5), just outside this top-20. Futures and options both carry irreducible complexity (margin models, Greeks, assignment, roll logic) that is safer to tackle *after* the foundation is solid and *after* one non-equity class (crypto) has proven the asset-aware execution path end-to-end on the simplest possible instrument. Sequencing crypto before futures/options is a leverage decision, not a difficulty dodge: it's the cheapest way to prove the whole pipeline (registry → data → risk → paper broker → strategy) works for *any* non-equity class before spending XL-difficulty effort on margin/Greeks-heavy classes.

---

## 9. Asset-Class Roadmaps

**ETFs**: `ETF-RISK-01` → `ETF-REGISTRY-01` → `ETF-RANKER-01` (depends on `MULTI-ASSET-ALLOCATOR-01`) → `ETF-STRAT-01`. Lowest-risk roadmap in this document — no new broker, no new data pipeline, no new instrument type. Pure metadata + risk-engine + allocator wiring on top of an already-working execution path.

**Crypto**: `CRYPTO-REGISTRY-01` → `CRYPTO-DATA-01` → `CRYPTO-RISK-01` → `CRYPTO-EXEC-01` → `CRYPTO-STRAT-01`. Recommended first live non-equity vertical (§7/§8 rationale). `BROKER-ALPACA-CRYPTO-01` is a viable shortcut for `CRYPTO-EXEC-01` if Alpaca's crypto API proves adequate — should be evaluated before building a from-scratch crypto adapter.

**Futures**: `FUTURES-REGISTRY-01` → `FUTURES-DATA-01`/`02` → `FUTURES-RISK-01` (margin model — do not skip) → `BROKER-IBKR-01..04` or another futures-capable adapter → `FUTURES-EXEC-01` → `FUTURES-STRAT-01`/`02`. Existing equity strategy engines (`swing_momentum`, `volatility_breakout`, `mean_reversion`) are credible retargeting candidates once the instrument model supports futures — this should reduce `FUTURES-STRAT-01`/`02` effort below a from-scratch build.

**Options**: `OPTIONS-CONTRACT-01` → `OPTIONS-CHAIN-01` (new data pipeline — current OHLCV-only ingestion cannot serve this) → `OPTIONS-RISK-01` → `BROKER-ALPACA-OPTIONS-01` → `OPTIONS-BACKTEST-01` → `OPTIONS-WHEEL-01` (read-only scanner, low risk) → `OPTIONS-WHEEL-02` (paper engine, needs short-margin enforcement first) → `OPTIONS-SPREADS-01` (multi-leg, needs an OCO/combo-order model that does not exist yet at any layer). Long options should be paper-proven well before any short/undefined-risk strategy, per the existing scaffold doc's own promotion-gate philosophy.

**Forex**: `FX-FUTURES-01` → `FX-DATA-01` → `FX-RISK-01` (leverage + macro-event model) → `FX-STRAT-01`. Lowest near-term priority of the four new asset classes — no current adapter candidate is wired even at the metadata level beyond unverified provider declarations, and US retail FX carries its own regulatory track (NFA) the repo has not begun to address.

**Fixed Income / Rates**: `RATES-REGISTRY-01` (start with TLT/IEF/SHY/ZN/ZB via the futures-and-ETF-first approach the brief itself recommends) → `RATES-DATA-01` (yield curve — genuinely new infrastructure, no analog elsewhere in the repo) → `RATES-RISK-01` → `RATES-STRAT-01`. Should ride behind both `ETF-REGISTRY-01`/`ETF-RISK-01` (for the ETF leg) and `FUTURES-REGISTRY-01` (for the ZN/ZB leg) rather than being built as a third, separate instrument pipeline.

---

## 10. Broker Expansion Roadmap

**Alpaca today**: equity-only. Confirmed by direct read of `mqk-broker-alpaca/src/lib.rs` — every call is to `/v2/orders*`, `/v2/account`, `/v2/positions`, or `/v2/account/activities/FILL`. Zero `/v2/crypto/*` or `/v2/options/*` calls exist. Price/quantity formatting is hardcoded to 2-decimal/whole-share equity conventions with explicit TODOs deferring fractional/crypto/option precision.

**Trait seam quality**: `BrokerAdapter` (in `mqk-execution::order_router`) is genuinely broker-agnostic — `submit_order`/`cancel_order`/`replace_order`/`fetch_events`, all broker-neutral request/response types. A second adapter implementing this trait would need **no changes to `mqk-execution`**. The leakage is one layer up: `mqk-daemon`'s broker builder hardcodes `AlpacaBrokerAdapter` instantiation and Alpaca-specific env-var names, and the read-only position/account sync (`broker_baseline.rs`) is coupled to having a live adapter instance rather than being a standalone "watch this broker" capability.

**Recommended path**: `BROKER-IBKR-01` (skeleton implementing `BrokerAdapter`, isolated crate) → `BROKER-IBKR-02` (ConId mapping) → `BROKER-IBKR-03` (decouple read-only sync from order-routing instantiation — this also pays off for any future broker) → `BROKER-IBKR-04` (paper order routing, gated behind the same kind of promotion review the scaffold doc already specifies for asset classes). For options and crypto specifically, evaluate `BROKER-ALPACA-OPTIONS-01`/`BROKER-ALPACA-CRYPTO-01` first — Alpaca already has both APIs in the broader market, and reusing the existing adapter/credential plumbing is likely cheaper than IBKR for those two classes specifically, even though IBKR is the more natural fit for futures and forex.

---

## 11. Multi-Strategy / Multi-Asset Orchestration Roadmap

**Current state** (verified): single-strategy-per-symbol dispatch is fully wired and mature (multi-symbol config, watchlist-driven assignment). A genuine, tested, default-off **dry-run secondary strategy** mechanism already exists (`MQK_DRY_RUN_STRATEGY_IDS` → per-symbol shadow evaluation with no order-submission capability, surfaced at `GET /api/v1/strategy/dry-run/status`) — this is a real, underused asset for safely experimenting with new strategies/asset classes before committing capital. A backtest-only strategy-lab ranking tool (`rank_strategy_lab_evaluations`) exists but has **zero callers** from live/paper code — it is a research report generator, not a live decision input. The portfolio allocator (`mqk-portfolio/src/allocator.rs`) is complete and tested but also has **zero callers** from `mqk-runtime`/`mqk-daemon`.

**Gap vs. the mission's orchestration target** ("rank symbols, asset classes, and strategies; pair best strategy with best instrument; portfolio-level position sizing"): every individual building block named in that target either doesn't exist (conflict policy) or exists-but-isn't-connected (allocator, strategy-lab ranking). None of this requires inventing new concepts — it requires wiring three already-built, already-tested pieces together and adding one new piece (conflict policy).

**Recommended path**: `MULTI-ASSET-ALLOCATOR-01` (give the allocator runtime callers + asset-class-aware candidate input) → `MULTI-STRATEGY-CONFLICT-POLICY-01` (new — define what happens when two strategies target the same symbol) → extend the dry-run mechanism so secondary strategies can carry a ranking score consumable by the allocator → only then attempt true cross-asset-class ranking (which additionally depends on `ASSET-CORE-01`/`04` existing, since you cannot rank a crypto candidate against an equity candidate without a shared instrument/portfolio model).

---

## 12. Risks and Non-Negotiables

These restate and extend the existing `multi_asset_scaffold_01.md` "Hard Boundaries" / "What Must NOT Change" tables, which remain valid and should continue to govern all future asset-class work:

- All non-equity asset types remain **disabled by default**; no patch in this roadmap should flip a default-on flag for any class before its full promotion gate (per the scaffold doc) is met.
- `ASSET-CORE-04` (Portfolio Ledger) is the highest-blast-radius patch in this entire roadmap — it touches live capital accounting invariants directly. Per `CLAUDE.md` and `audit_repo_truth_rules.md`, it requires its own scenario-test proof standard before any `CLOSED` claim, and should not be bundled with any other patch.
- The two-enum split (`mqk_schemas::AssetClass` vs `mqk_md::provider::ProviderAssetClass`) is architecture debt that will compound the longer it's left unresolved — every new asset class written against the wrong enum makes unification harder later. Resolve as part of `ASSET-CORE-01`, not deferred further.
- `ASSET-CAPABILITY-MATRIX-01` is backend-complete but **not GUI-visible**. Per `gui_rules.md` ("no friendly defaults that hide unproven state"), any future GUI work that touches the system-status/metadata screens should surface this matrix rather than leaving operators to `curl` it.
- The existing scaffold doc's promotion-gate philosophy (adapter unit tests → paper sandbox → N-day evidence review → operator sign-off, scaled by asset-class risk — 30 days for crypto/options/forex, 60 days for futures) should be preserved verbatim for every asset class this roadmap eventually reaches. This audit does not relax it anywhere.
- Equity `MAIN` path: zero changes recommended or implied by this roadmap to the current paper+Alpaca equity execution, risk, reconcile, or broker code.
- Ledger §11 (`DATA-MULTI-ASSET-MODEL-01` and the four `DATA-INGEST-*-PLAN-01` items) should be explicitly reconciled against this audit's Phase 0–4 patches (see `LEDGER-MULTI-ASSET-RECONCILE-01`) so the repo does not end up tracking the same work under two different patch-ID schemes.

---

## 13. Recommended Next Patch

**Superseded by events — see §17 for the current closure note.** At audit time, this section recommended `ETF-RISK-01` first (or `LEDGER-MULTI-ASSET-RECONCILE-01` as a docs-first alternative), then `ASSET-CORE-01`. Since then: `ETF-RISK-01` closed (`ETF-RISK-CLOSURE-01` + `ETF-RISK-EXTERNAL-SIGNAL-GATE-01`, see §16) and `LEDGER-MULTI-ASSET-RECONCILE-01` closed (this patch, see §17). The current recommended next patch is:

**`ASSET-CORE-01`** (Unified Instrument Registry v2 — also resolves the `mqk_schemas::AssetClass` vs `mqk_md::provider::ProviderAssetClass` two-enum split, §2).

Rationale (unchanged from the original recommendation): every other asset class in this roadmap depends on it; nothing in Phases 1–6 should start in earnest until the instrument model is unified, per this audit's central recommendation in §1.

**Original recommendation (preserved for history):** at audit time, **`ETF-RISK-01`** (wire the existing, already-written `SectorConstraint`/`check_sector_limits()` code in `mqk-portfolio/src/constraints.rs` into the live risk engine) was recommended first — smallest possible diff, zero new architecture, zero touch to broker/execution/live-routing code, delivered a real risk-management capability squarely inside the mission's own Phase 5 ask, with no preceding patch required. A close second was **`LEDGER-MULTI-ASSET-RECONCILE-01`** as a docs-first alternative. Both are now closed.

---

## 14. Validation / Search Evidence

**Repo-state commands run** (per mission instruction, not routine `git status`):
```
git branch --show-current        → main
git log --oneline -45             → HEAD 40b3331, clean history
git diff --name-only              → (empty — no dirty tracked files)
git ls-files --others --exclude-standard → MiniQuantDesk_Master_Patch_Ledger_v2_updated.md, smoke_logs/* (untouched)
```

**Searches performed** (direct, plus four parallel read-only research passes covering the full ripgrep pattern from the mission brief across `core-rs/crates`, `core-rs/mqk-gui/src`, `config/`, `docs/`, `scripts/`):
- `mqk_schemas::(AssetClass|Instrument|ContractSpec|OrderSpec|QtyMicros|Position|OptionRight|OrderSide|OrderType)` cross-crate usage → only `AssetClass` used outside its own crate.
- `asset_class_scope|equity_only|Gate 0` → confirmed B8 patch chain end-to-end (Rust + GUI TS).
- `IBKR|Interactive Brokers|ConId|TWS|IB Gateway|ib_insync|ibapi` → zero matches outside the planning doc.
- `ES|MES|NQ|MNQ|RTY|M2K|CL|MCL|GC|MGC|ZN|ZB|6E|M6E|Globex|CME` (futures symbols) → zero matches anywhere in code/config/docs.
- `strike|expiry|greeks|implied_vol|assignment|covered_call|cash_secured|wheel|OCC` → zero functional matches (schema field names only).
- `treasury|yield|duration|rate_shock|FOMC|CPI` → zero functional matches.
- Direct reads: `mqk-schemas/src/lib.rs`, `mqk-execution/src/{order_router,gateway,types,lib}.rs`, `mqk-daemon/src/routes/system.rs` (capability matrix), `mqk-daemon/src/state/market_calendar.rs`, `mqk-portfolio/src/types.rs`, `mqk-broker-alpaca/src/lib.rs` (REST endpoint list), `config/instruments/equities.json` (88 entries, all `asset_class:"equity"`), `config/providers/providers.json`, `docs/specs/experimental/multi_asset_scaffold_01.md` (full read), `docs/audits/data_ingestion_coverage_audit.md` (style/cross-reference), `MiniQuantDesk_Master_Patch_Ledger_v2.md` §§6/11/18.
- `git log` provenance checks confirming `653730f`/`424f0de`/`ff2ae59`/`6fe1697`/`0b622a5` are all ancestors of current `HEAD`.

No `cargo build`/`cargo test`/full workspace suite was run (not required for a static docs audit; this audit makes no code-correctness claims that would require it).

`git diff --check` run after writing this document and the ledger update: see §15 commit record.

---

## 15. Safety Confirmation

- Docs only: this patch adds [`docs/audits/multi_asset_completion_audit.md`](multi_asset_completion_audit.md) (this file) and appends one new section to `MiniQuantDesk_Master_Patch_Ledger_v2.md`. No other files were modified.
- No broker submit changes. No live routing changes. No order/outbox writes. No DB migrations. `.env.local` was not read or touched. No provider/broker network calls were made. No paper/live orders were submitted. No short-entry enablement. B5/risk gates untouched.
- No daemon was started at any point during this audit.
- No full test suite or full workspace build was run.

---

## 16. ETF-FOUNDATION-01 Closure Note (follow-up patch)

`ETF-FOUNDATION-01` (combining `ETF-REGISTRY-01` + `ETF-RISK-01`) closed `ETF-REGISTRY-01` in full and found `ETF-RISK-01` blocked on a dependency deeper than this audit identified. Recorded here rather than left only in commit history, per `audit_repo_truth_rules.md` ("repo state is authoritative... if memory contradicts the current file state, trust the file").

**`ETF-REGISTRY-01` — CLOSED.** The 14 target ETFs (§ mission brief) are tagged in `config/instruments/equities.json` with `instrument_kind: "etf"`, `sector`, and `category`, added as new optional fields on `TrackedInstrument` (`core-rs/crates/mqk-md/src/instrument_registry.rs`). `asset_class` was deliberately left `"equity"` for every ETF — `enabled_equities()`/`validate_registry()`/ingestion/backtest/GUI behavior is unchanged. A new pure `sector_map()` helper builds the `symbol -> sector` map shape `mqk_portfolio::constraints::check_sector_limits` expects.

**`ETF-RISK-01` — PARTIAL, blocker is more fundamental than "missing sector metadata."** This audit's §4/§13 framed the gap as `SectorConstraint`/`check_sector_limits` having "zero callers" — implying wiring was mostly a metadata problem. Direct inspection of the live admission path during `ETF-FOUNDATION-01` found a deeper blocker: **no live per-symbol mark-price, notional, or portfolio-weight computation reaches any admission boundary today, for any symbol, equity or ETF.**

Evidence:
- `mqk_risk::{RiskInput, RiskState}` (`core-rs/crates/mqk-risk/src/types.rs`) carry no symbol field at all — the engine is a portfolio-level kill switch (drawdown/daily-loss/PDT/reject-storm), not a per-instrument gate.
- `RiskRequestContext` (`core-rs/crates/mqk-execution/src/gateway.rs`), the struct carried into the live `RiskGate::evaluate_gate_for_request` call at `BrokerGateway::submit_with_context` — the closest thing to a "pre-broker-submit risk gate" — has exactly one field, `is_risk_reducing: bool`. No symbol, no quantity, no price.
- The live tick-loop snapshot consumed at decision time, `PositionSnapshot`/`PortfolioSnapshot` (`core-rs/crates/mqk-runtime/src/observability.rs`), carries only `symbol`/`net_qty`/`cash_micros`/`realized_pnl_micros` — no mark price, no market value, no NAV.
- The existing per-symbol/per-strategy caps that *do* run at the decision boundary (`mqk-daemon/src/capital_policy/{position_sizing,portfolio_risk}.rs`, cap #3/#5) are single-order notional checks computed from the order's own `qty x limit_price` — they do not aggregate cross-symbol portfolio state, and `portfolio_risk.rs` says so explicitly: market-order / drift cases return `RiskUnverifiable` because "portfolio drift is not measurable at signal time without runtime portfolio state."
- `cargo test -p mqk-risk sector` and `cargo test -p mqk-runtime sector` both match zero tests against current HEAD, confirming no sector-aware code exists in either crate today.

`check_sector_limits` takes a weight map (`BTreeMap<String, f64>`) and is meant to be evaluated against prospective post-trade weights. Building that input for real would require fabricating a price/NAV source that does not exist in the live runtime — which is exactly the "no fabricated truth, no optimistic defaults" failure mode `CLAUDE.md`'s operator-truth discipline forbids. Per this patch's own operating brief ("create only the registry metadata and a small pure bridge/helper if safe... report ETF-RISK-01 as PARTIAL"), `ETF-FOUNDATION-01` stopped at the registry + `sector_map()` bridge and did not touch `mqk-portfolio`, `mqk-risk`, `mqk-runtime`, or `mqk-daemon` production code.

**Recommended next dependency patch:** a live portfolio-weight/notional patch (e.g. `PORTFOLIO-LIVE-WEIGHTS-01`) that adds mark-price and NAV/weight computation to the live snapshot/decision path — equity-wide, not ETF-specific. Only after that exists can `ETF-RISK-01` wire `check_sector_limits` against real (not fabricated) inputs.

**`PORTFOLIO-LIVE-WEIGHTS-01` — CLOSED_LOCAL, truth seam built, not yet wired to the decision boundary.** `mqk_portfolio::compute_portfolio_weights` (`core-rs/crates/mqk-portfolio/src/valuation.rs`) is a pure function that turns explicit signed quantities + explicit, attributed marks into per-symbol market value, NAV, and weight (`weight_bps`), with three honest truth states — `"active"`, `"missing_marks"`, `"nav_unavailable"` — and never substitutes a missing mark with zero. A read-only `GET /api/v1/portfolio/live-weights` route (`mqk-daemon/src/routes/portfolio.rs`) sources positions/cash from the in-memory execution snapshot and marks from the latest *completed* `md_bars` row per non-flat symbol (never the broker, a live quote, or an order/entry price); it adds a fourth, daemon-level truth state, `"db_unavailable"`, distinguishing "no DB to even attempt a lookup" from "DB present but this symbol has no bar." This closes the specific gap identified above (no live mark/NAV/weight computation existed anywhere) as a standalone, inspectable surface. It does **not** close `ETF-RISK-01`: nothing in `mqk-risk`, `RiskRequestContext`, or the live admission/decision path calls this seam yet — that remains the next dependency patch.

**`ETF-RISK-CLOSURE-01` — CLOSED.** `mqk_portfolio::evaluate_sector_risk` (`constraints.rs`, distinct from and additive to the untouched `SectorConstraint`/`check_sector_limits`) recomputes `compute_portfolio_weights` before/after a candidate order and applies a configured per-sector bps cap, fail-closed on missing marks/NAV, with a risk-reducing override. It is wired as **Gate 1h** in `decision.rs`'s `submit_internal_strategy_decision` — pre-outbox, default-off via `MQK_SECTOR_EXPOSURE_LIMITS_BPS`, reusing the exact registry `sector_map()` and `md_bars` seam `PORTFOLIO-LIVE-WEIGHTS-01` built. `mqk_risk`/`RiskRequestContext` (the pre-broker-submit seam) remain untouched — unnecessary, since the pre-outbox seam is earlier and sufficient. Known gap at the time: the separate external-signal HTTP path (`routes/strategy.rs`) was not wired, by scope decision.

**`ETF-RISK-EXTERNAL-SIGNAL-GATE-01` — CLOSED.** Closes the gap named directly above. The registry/snapshot/marks glue that built Gate 1h's inputs was extracted from `decision.rs` into a new shared module, `capital_policy::sector_risk_gate::evaluate_sector_risk_gate` — both the internal decision path (`decision.rs` Gate 1h) and the external signal path (`routes/strategy.rs`, new **Gate 1i**) now call the same function, so sector exposure risk cannot drift between an internally-generated order and an externally-submitted signal. Same env var, same default-off behavior, same pure `evaluate_sector_risk` evaluator underneath (untouched). On the external path, a verified breach (`sector_limit_exceeded`) returns `403`; every other deny outcome — malformed config, unreadable registry, missing snapshot/DB/mark, non-positive NAV — returns `503` (the gate could not verify safety, distinct from a verified breach). 9 new scenario tests (`mqk-daemon/tests/scenario_external_signal_sector_risk_01.rs`), 6 DB-backed and run for real against the local paper DB, using a fixture distinct from `ETF-RISK-CLOSURE-01`'s own test file. Full detail, test matrix, and validation evidence: `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ETF-RISK-EXTERNAL-SIGNAL-GATE-01` entry.

---

## 17. LEDGER-MULTI-ASSET-RECONCILE-01 Closure Note (maintenance)

Docs/ledger-only maintenance patch. No production code, config, DB, or trading-path changes. Recorded here per `audit_repo_truth_rules.md` ("repo state is authoritative... if memory contradicts the current file state, trust the file").

**What this patch closed:**

- Ledger §11's five historical planning labels (`DATA-MULTI-ASSET-MODEL-01`, `DATA-INGEST-CRYPTO-PLAN-01`, `DATA-INGEST-FUTURES-PLAN-01`, `DATA-INGEST-OPTIONS-PLAN-01`, `DATA-INGEST-FOREX-PLAN-01`) are reconciled — mapped to this roadmap's Phase 0–4 patch IDs directly in §11, marked `RECONCILED / SUPERSEDED` rather than `QUEUED`. None were deleted.
- `multi_asset_scaffold_01.md`'s "Future Patch Lane IDs" table refreshed: `ASSET-CAPABILITY-MATRIX-01` (`424f0de`), `MULTI-ASSET-ROUTING-GUARD-01` (`ff2ae59`), `DISABLED-ASSET-GATE-TESTS-01` (`6fe1697`) marked `SHIPPED` — independently re-verified via `git merge-base --is-ancestor` against current `HEAD` (not just copied from §2/§5 above), plus a direct read confirming `mqk-daemon/src/routes/system.rs::static_asset_capability_matrix` and `mqk-execution/tests/scenario_asset_class_guard_multi_asset_routing_guard_01.rs` exist in the working tree.
- §13 above updated: recommended next patch is now `ASSET-CORE-01`, not `ETF-RISK-01` (closed) or `LEDGER-MULTI-ASSET-RECONCILE-01` (closed by this patch).
- §5's patch status table: this patch's own row updated from `MISSING (new, recommended)` to `CLOSED`.

**What this patch deliberately did not do:** no re-audit of `ASSET-CORE-01` or any other PARTIAL/MISSING patch's completion percentage; no change to the dependency graph (§6), build sequence (§7), Top-20 list (§8), or any asset-class roadmap (§9); no change to `multi_asset_scaffold_01.md`'s promotion-gate philosophy or hard boundaries. This is a pure status-reconciliation pass, not a re-audit.

**Safety confirmation:** docs only. No Rust/GUI/config/DB files touched. No daemon started. No provider/broker network calls. No paper/live orders submitted. `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` (untracked draft) and `smoke_logs/` were not staged or touched.

---

## 18. ASSET-CORE-01A Closure Note (maintenance)

`ASSET-CORE-01A` started `ASSET-CORE-01` with its safest slice: the §2 "architecture-debt finding" two-enum split is now explicitly mapped and exhaustively tested, not just documented as a gap. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Resolved:** `mqk_md::provider::provider_asset_class_trading_class`/`provider_asset_class_instrument_kind` (`core-rs/crates/mqk-md/src/provider.rs`) give `ProviderAssetClass` an explicit, exhaustive (no-wildcard-match), pure mapping to the same canonical singular vocabulary `mqk_schemas::AssetClass` and `mqk-runtime`'s `validated_asset_class` already use independently (`"equity"`, `"option"`, `"future"`, `"crypto"`, `"forex"`). `TrackedInstrument` gained typed accessors (`is_etf`, `trading_asset_class`, `normalized_instrument_kind`/`sector`/`category`) proving the same ETF-as-equity invariant the registry already encoded. A new cross-mapping test (`im_08`) directly checks the registry's real ETF entries against the provider-side mapping and confirms they agree. 17 new tests total; zero behavior change to any existing function; zero new `Cargo.toml` dependency edges (confirmed dependency-graph-legal in either direction, but deliberately not taken — see the patch's own ledger entry for the Option A vs Option B rationale).

**Not resolved (`ASSET-CORE-01` remains PARTIAL):** the two enums still exist as separate types — this patch did not collapse them into one shared type, did not add a `mqk-md → mqk-schemas` dependency, and did not touch `equities.json`'s schema. A real unified instrument-registry v2 (multi-provider, multi-asset-class-aware, replacing today's equities-only registry file and string `asset_class` field) is still `ASSET-CORE-01B`'s job, not done here.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-01A` entry (end of §19).

**Safety confirmation:** no broker submit changes; no Alpaca adapter changes; no live routing changes; no order/outbox writes; no DB migrations; `.env.local` untouched; no provider/broker calls; no paper/live orders; no non-equity asset class enabled; disabled-asset gates re-proven unmodified (`scenario_asset_class_scope_b8` 12/12, `scenario_asset_class_guard_multi_asset_routing_guard_01` 8/8). No daemon started.

---

## 19. ASSET-CORE-01B Closure Note (maintenance)

`ASSET-CORE-01B` built the real registry v2 schema/loader/validator `ASSET-CORE-01A` deferred: `core-rs/crates/mqk-md/src/instrument_registry_v2.rs` (`InstrumentRegistryV2`/`InstrumentDefinitionV2`/`ContractDefinitionV2`, covering equity/ETF/option/future/crypto/forex), `load_instrument_registry_v2`, `validate_registry_v2`, and pure v1→v2 conversion (`convert_v1_registry_to_v2`/`convert_tracked_instrument_to_v2`). 26 new tests; the entire real 88-row `config/instruments/equities.json` converts and validates cleanly under v2 rules. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Resolved:** a real, additive multi-asset-aware registry model now exists and is proven compatible with current production data in memory — the §2/§5 gap this audit identified as `ASSET-CORE-01B`'s job.

**Not resolved (`ASSET-CORE-01` remains PARTIAL):** nothing consumes `InstrumentRegistryV2` yet — no daemon route, no CLI command, no ingestion/backtest/GUI path. `equities.json` is still the only registry file any production code reads. No `mqk-md → mqk-schemas` dependency was added (same Option B rationale as `ASSET-CORE-01A`, re-justified: the existing `mqk_schemas::Instrument`/`ContractSpec` types are live execution-path types and narrower than what v2 needs). No non-equity asset class is enabled anywhere.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-01B` entry (end of §19).

**Safety confirmation:** no broker submit changes; no Alpaca adapter changes; no live routing changes; no order/outbox writes; no DB migrations; `.env.local` untouched; no provider/broker calls; no paper/live orders; no non-equity asset class enabled (the one `enabled=true` non-equity test case is `#[cfg(test)]`-only and proves the validator's own explicit escape hatch, not a production path). No daemon started.

## 20. ASSET-CORE-01C Closure Note (maintenance)

`ASSET-CORE-01C` gave `InstrumentRegistryV2` (`ASSET-CORE-01B`) its first real, non-test reader: `GET /api/v1/system/instrument-registry-v2/status` (`mqk-daemon/src/routes/system.rs::system_instrument_registry_v2_status`) and `mqk md registry-v2-status --registry <path>` (`mqk-cli`) both load the configured v1 registry, convert it to v2 in memory, validate it, and report `truth_state`/counts/validation errors. Both are strictly read-only diagnostics: `production_cutover_enabled` and `trading_uses_v2` are hardcoded `false` in every response, on every `truth_state` path including failures. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Resolved:** the §16/§19 gap ("nothing consumes `InstrumentRegistryV2` yet") now has a concrete, tested answer — an operator/CI-facing surface exists that proves, against the real `config/instruments/equities.json`, that v1→v2 conversion and validation succeed (`v1_count == v2_count == 88`, `etf_count == 14`, zero non-equity/paper/live-enabled rows) and reports truthfully when they don't (missing file, malformed JSON, or a v2-shape validation failure).

**Not resolved (`ASSET-CORE-01` remains PARTIAL):** `equities.json` is still the only registry file any production trading/ingestion/backtest/GUI/risk/broker path reads. No daemon/runtime/ingest/backtest/GUI code path was changed to read `InstrumentRegistryV2`; the new route and CLI command are diagnostic-only and were the explicit mission boundary. No `mqk-md → mqk-schemas` dependency was added (no occasion to revisit it — this slice touched only `mqk-daemon`/`mqk-cli`, not `mqk-md`). No non-equity asset class is enabled anywhere.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-01C` entry (end of §19).

**Safety confirmation:** no broker submit changes; no Alpaca adapter changes; no live routing changes; no order/outbox writes; no DB migrations; `.env.local` untouched; no provider/broker calls; no paper/live orders; no non-equity asset class enabled; disabled-asset gates untouched. No daemon started — all proof is route/unit-level (`axum::Router::oneshot`).

## 21. ASSET-CORE-05A Closure Note (maintenance)

ASSET-CORE-05A added an additive session-classification seam for equity US regular, crypto continuous, futures regular/extended/overnight, and forex 24x5 concepts. It is model/test-only and does not switch runtime behavior, enable non-equity assets, add DB state, or change trading. `ASSET-CORE-05` remains `PARTIAL` pending authoritative calendar/holiday/early-close and per-instrument session routing work.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-05A` entry (end of §19).

## 22. ASSET-CORE-05B-COMBINED Closure Note (maintenance)

ASSET-CORE-05B-COMBINED strengthened/proved the bounded US equity calendar/holiday/early-close provider and added a pure session-profile resolution seam for future instrument metadata. Current runtime behavior is unchanged; non-equity profiles remain model-only/unwired. `ASSET-CORE-05` remains `PARTIAL` pending true per-instrument session routing and non-equity authoritative session providers.

Part A found the existing bounded table (`mqk-integrity::calendar`, 2023–2028, 60 holiday + 10 early-close entries) already well-covered and did not expand it — expanding it would require dates beyond what the repo already encodes, which this patch's mission forbade inventing. It corrected one stale doc-comment (`NyseWeekdaysProvider` claimed 2023–2026) and added `EQCAL01`/`EQCAL02` contract tests proving one known holiday and one known early-close per covered year, closing a real per-year test-coverage gap (2023 and 2025 had none before this patch). Part A also surfaced — but did not fix, since fixing it would be a runtime behavior change out of scope — that the `MarketCalendarProvider` trait seam in `mqk-daemon` is consulted only by its own tests; production session gating (`session_controller.rs`) calls `mqk_integrity::CalendarSpec::NyseWeekdays` directly. Both paths converge on the same underlying table today, so there is no truth drift, but the daemon-side seam remains an unconsumed abstraction.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-05B-COMBINED` entry (end of §19).

## 22A. ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED Closure Note (maintenance)

ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED added additive session-profile modeling diagnostics and read-only GUI/API visibility for equity regular, 24/7 crypto scaffold, futures-electronic/Globex-style scaffold, and FX 24/5 scaffold sessions. Current equity behavior remains unchanged; non-equity profiles are not used for trading and do not enable non-equity execution. `ASSET-CORE-05` remains `PARTIAL` pending true per-instrument session routing, authoritative non-equity calendars, and maintenance-break-aware product calendars.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED` entry (end of §19).

## 22B. ASSET-CORE-05B-INSTRUMENT-SESSION-STATUS-01-COMBINED — CLOSED_LOCAL / PARTIAL Note (maintenance)

ASSET-CORE-05B-INSTRUMENT-SESSION-STATUS-01-COMBINED added a read-only daemon status route, `GET /api/v1/system/instrument-sessions/status`, that connects the ASSET-CORE-01C v1→v2 registry conversion truth to the ASSET-CORE-05A/05B session-profile seam on a per-instrument basis. It reports the profile each converted instrument maps to, the session state at an injected UTC timestamp, and whether that profile is production-backed or model-only.

The current real registry remains equity-only: all converted production rows map to `equity_us_regular`, ETFs remain equity instruments with `instrument_kind="etf"`, and zero non-equity rows are enabled. Crypto, futures, and forex are proven only through disabled/model-only fixtures; no non-equity registry row, route, gate, broker adapter, risk path, OMS path, portfolio path, strategy path, or runtime path was enabled.

The new route hard-reports `production_cutover_enabled=false`, `trading_uses_session_v2=false`, `runtime_uses_session_v2=false`, and per-instrument `trading_uses_this=false`. CLI was skipped to keep this slice bounded; ASSET-CORE-01C's existing `mqk md registry-v2-status` remains unchanged.

`ASSET-CORE-05` remains `PARTIAL` pending authoritative non-equity calendars where missing, any approved production runtime cutover, and actual per-instrument enforcement/routing.

Validation passed in full: the focused ASSET-CORE-05B daemon test (11/11), ASSET-CORE-05A/01C regressions (31/31, 13/13), the GUI daemon contract gate (23/23), `cargo check`, daemon clippy (`-D warnings`), and fmt check all passed. This slice is closed; `ASSET-CORE-05` as a whole remains `PARTIAL` per the note above.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-05B-INSTRUMENT-SESSION-STATUS-01-COMBINED` entry (end of §19).

## 22C. ASSET-CORE-05C-SESSION-PARITY-STATUS-SHADOW-01-COMBINED — CLOSED_LOCAL / PARTIAL Note (maintenance)

ASSET-CORE-05C-SESSION-PARITY-STATUS-SHADOW-01-COMBINED added a read-only shadow-parity daemon route, `GET /api/v1/system/instrument-sessions/parity`, that independently re-derives production session truth (via `NyseWeekdaysProvider`) per equity instrument and compares it against ASSET-CORE-05B's per-instrument session-profile classification, plus a compact `instrument_session_shadow` summary embedded on `/api/v1/system/status` and `/api/v1/system/preflight`.

Real-registry proof at a fixed timestamp (`2026-06-25T15:00:00Z`): all 88 production equity rows report `parity_state="matched"` (`checked_count=88`, `matched_count=88`, `mismatched_count=0`, `unknown_count=0`, `model_only_count=0`, `all_equity_profiles_match_production=true`). Holiday (2026-07-03) and early-close (2024-11-29, before/after 13:00 ET) fixed timestamps were also proven matched against the same production calendar. Non-equity (crypto/futures/forex) classification was proven model-only at the pure-function level (`resolve_session_profile_for_instrument_metadata` resolves to `UnsupportedAssetClass`/`model_only`, never `Active`) — a route-level non-equity fixture row is not constructible because `mqk-md`'s v1→v2 conversion unconditionally assigns an `Equity` contract regardless of the v1 `asset_class` string, and the real production v1 registry contains only equity rows.

The new route and both summary fields hard-report `production_cutover_enabled=false`, `runtime_uses_session_v2=false`, `trading_uses_session_v2=false`, and `shadow_only=true`. The shadow summary is observability only: it is never added to `blockers`/`warnings` on `/api/v1/system/preflight` and never affects `deployment_start_allowed` on either surface. No DB, provider/broker call, daemon runtime start, order submission, or strategy/risk/OMS/portfolio/broker code was touched.

`ASSET-CORE-05` remains `PARTIAL` pending authoritative non-equity calendars where missing, any approved production runtime cutover, and actual per-instrument enforcement/routing — this patch is observability only and does not change which session truth gates trading today.

Validation passed in full: the focused ASSET-CORE-05C daemon test (15/15), ASSET-CORE-05B/market-calendar/01C regressions (11/11, 31/31, 13/13), the GUI daemon contract gate (23/23), the route contract gate (2/2), `cargo check`, daemon clippy (`-D warnings`), and fmt check all passed.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-05C-SESSION-PARITY-STATUS-SHADOW-01-COMBINED` entry (end of §19).

## 22D. ASSET-CORE-05D-EQUITY-SESSION-V2-CUTOVER-SCAFFOLD-01-COMBINED — CLOSED_LOCAL / PARTIAL Note (maintenance)

ASSET-CORE-05D-EQUITY-SESSION-V2-CUTOVER-SCAFFOLD-01-COMBINED added a default-off, env-gated `RuntimeSessionSourceMode` seam (`MQK_RUNTIME_SESSION_SOURCE`, values `legacy`/`v2_equity_shadow`) in a new `mqk-daemon/src/state/runtime_session_source.rs` module, plus a compact `runtime_session_source` summary embedded on `/api/v1/system/status` and `/api/v1/system/preflight`, mirroring the existing `instrument_session_shadow` (ASSET-CORE-05C) field pattern.

With no env var set (the default), `session_source_mode="legacy"` and the v2 registry is never loaded at all — proven directly by pointing the daemon at a deliberately missing registry path in legacy mode and confirming no `fallback_reason` is produced. In `v2_equity_shadow` mode, the candidate evaluator reuses the same load/convert/validate registry path and the same public classification primitives (`resolve_session_profile_for_instrument_metadata`, `NyseWeekdaysProvider`) ASSET-CORE-05B/05C already use, requires every enabled registry row to be equity, requires at least one equity/ETF row to be checkable, and requires the v2 per-instrument classification to match the legacy production state exactly before reporting `candidate_would_activate=true`. Any registry problem, any enabled non-equity row, or any parity mismatch produces an explicit `fallback_reason`/`activation_refusal_reason` rather than silent activation. An unrecognized `MQK_RUNTIME_SESSION_SOURCE` value resolves to `legacy` (matching this repo's existing `parse_deployment_mode`/`BrokerKind::parse` fail-soft convention) and never aborts daemon startup.

Real-registry candidate parity was proven at four fixed timestamps against the same 88-row production registry ASSET-CORE-05C uses: regular-open (`2026-06-25T15:00:00Z`, 88/88 matched), closed weekend (`2026-06-27T15:00:00Z`, matched), holiday (`2026-07-03T14:00:00Z`, observed Independence Day, matched), and Black Friday early close before/after the 13:00 ET close (`2024-11-29T17:59:00Z`/`18:01:00Z`, both matched). Refusal wiring was proven for a synthetic parity mismatch, a missing registry, a malformed registry, and an enabled non-equity row (with the converse case — a present-but-disabled non-equity row — proven NOT to block activation, since the requirement scopes to runtime-enabled sources).

`production_cutover_enabled`, `runtime_uses_session_v2`, and `trading_uses_session_v2` are hardcoded `false` on the summary regardless of mode or candidate outcome — this patch proves candidate parity, it does not flip any of those flags. `session_controller.rs` and `start_execution_runtime` were not touched; this was confirmed by re-running the full `scenario_autonomous_readiness_auton_truth01` regression (18/18 pass) unchanged.

A genuine test-isolation race was found and fixed during validation: `MQK_RUNTIME_SESSION_SOURCE` is a process-global env var and Rust runs `#[test]` functions concurrently by default, so the unit tests mutating it (in `runtime_session_source.rs`'s own test module) intermittently failed against each other until a `static ENV_LOCK: std::sync::Mutex<()>` was added to serialize them — confirmed stable across 5 repeated runs afterward.

Validation passed in full: 21/21 new unit tests, 16/16 new integration tests, plus regressions — ASSET-CORE-05C (15/15), ASSET-CORE-05B (11/11), market-calendar (31/31), GUI daemon contract gate (23/23), route contract gate (2/2), autonomous readiness (18/18) — `cargo check` across `mqk-daemon`/`mqk-runtime`/`mqk-integrity`/`mqk-cli`/`mqk-md`, daemon and integrity clippy (`-D warnings`), and fmt check all passed.

`ASSET-CORE-05` remains `PARTIAL` pending an actual default production cutover, authoritative non-equity calendars, per-instrument enforcement for non-equity, and any market-hours (live wall-clock) proof — this patch used only injected fixed timestamps.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-05D-EQUITY-SESSION-V2-CUTOVER-SCAFFOLD-01-COMBINED` entry (end of §19).

## 22E. ASSET-CORE-05E-EQUITY-SESSION-V2-ACTIVE-CUTOVER-HOOK-01-COMBINED — CLOSED_LOCAL / PARTIAL Note (maintenance)

ASSET-CORE-05E-EQUITY-SESSION-V2-ACTIVE-CUTOVER-HOOK-01-COMBINED added a third, explicit `RuntimeSessionSourceMode::V2EquityActive` value (`MQK_RUNTIME_SESSION_SOURCE=v2_equity_active`) that — unlike ASSET-CORE-05D's `v2_equity_shadow` — can actually drive the real `AutonomousSessionSchedule::is_in_session` in-window decision in `session_controller.rs`. This is the single decision point consumed by the autonomous session controller's auto-start/auto-stop tick, `/api/v1/system/preflight`'s `session_in_window` field, and `/api/v1/autonomous/readiness`'s `session_in_window`/`session_window_state`/`overall_ready` fields — one hook fixes all three consistently rather than wiring each call site separately.

Default behavior is unchanged: with `MQK_RUNTIME_SESSION_SOURCE` unset, or explicitly `legacy`, or `v2_equity_shadow`, `is_in_session` takes the exact pre-patch code path with zero v2 registry IO. Only `v2_equity_active` triggers evaluation, reusing ASSET-CORE-05D's existing load/convert/validate/parity-check sequence unchanged. When that evaluation proves `candidate_would_activate=true`, the v2-sourced in-window boolean becomes authoritative; on any refusal (registry missing/malformed, enabled non-equity row, parity mismatch, zero checked instruments), `is_in_session` returns `false` — fail-closed — even at a timestamp where the legacy NYSE calendar alone would say the market is open. This override-not-coincidence behavior was proven directly: a missing-registry test at a real regular-open timestamp returns `false`.

Real-registry proof was extended from ASSET-CORE-05D's parity-only check to an actual in-window-boolean check at the same four fixed timestamps (regular-open, closed-weekend, holiday, before/after Black Friday early close) — all match legacy. The compact `runtime_session_source` summary on `/api/v1/system/status` and `/api/v1/system/preflight` gained `candidate_would_activate` (`Some(true)`/`Some(false)`/`null`) and `active_source_used` (`bool`); `production_cutover_enabled` now means "active mode is explicitly configured" while `active_source_used`/`runtime_uses_session_v2`/`trading_uses_session_v2` mean "v2 is actually driving the decision right now" — all three of the latter are computed from one shared expression so they cannot disagree.

Two real test-infrastructure bugs were found and fixed during validation, both worth recording because they generalize beyond this patch: (1) a cross-module env-var race — `runtime_session_source.rs`'s and the new `session_controller.rs` tests mutate the same process-global `MQK_RUNTIME_SESSION_SOURCE` inside one `cargo test --lib` binary, and two independent file-local locks do not serialize against each other; fixed by hoisting one shared `RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK`. (2) five *pre-existing* `session_controller.rs` tests (`sw11`-`sw15`) had never touched this env var before and so never guarded against it; once `is_in_session` started reading it, they could intermittently fail against a concurrently-scheduled new test. Fixed by adding the same lock acquisition to those five pre-existing tests with no change to what they assert. Confirmed flake-free across 4 consecutive full `--lib` runs (233/233 every time).

Validation passed in full: 11 new unit tests in `runtime_session_source.rs`, 5 new unit tests in `session_controller.rs` (plus the 5 pre-existing tests patched for the lock), 21/21 new integration tests in a new dedicated scenario file, plus regressions — ASSET-CORE-05D (16/16), ASSET-CORE-05C (15/15), ASSET-CORE-05B (11/11), market-calendar (31/31), registry-v2-status-01C (13/13), premarket-data-readiness (25/25), autonomous readiness (18/18), GUI daemon contract gate (23/23), route contract gate (2/2) — `cargo check` across `mqk-daemon`/`mqk-runtime`/`mqk-integrity`/`mqk-cli`/`mqk-md`, daemon/runtime/integrity clippy (`-D warnings`), and fmt check all passed.

`ASSET-CORE-05` remains `PARTIAL`: this is a cutover *hook* (default-off, explicit opt-in), not a default production cutover; no live wall-clock market-hours proof exists; no authoritative non-equity calendar exists; non-equity remains model-only and disabled with no per-instrument enforcement anywhere in the runtime/trading path.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-05E-EQUITY-SESSION-V2-ACTIVE-CUTOVER-HOOK-01-COMBINED` entry (end of §19).

## 22F. ASSET-CORE-05F-V2-EQUITY-ACTIVE-PROOF-RUNBOOK-COLLECTOR-01-COMBINED — CLOSED_LOCAL / PARTIAL Note (maintenance)

ASSET-CORE-05F-V2-EQUITY-ACTIVE-PROOF-RUNBOOK-COLLECTOR-01-COMBINED is off-market preparation for the one proof category ASSET-CORE-05D and ASSET-CORE-05E both explicitly deferred: an operator-supervised, real-wall-clock, paper-only session run with `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active`. This patch does not run that proof — it adds a read-only evidence collector script and an operator runbook so the proof can be captured repeatably when an operator chooses to run it.

Concretely: `scripts/windows/Collect-V2EquitySessionActiveProof.ps1` calls only `GET` routes already proven read-only and unauthenticated by `scenario_token_auth_middleware.rs` (`system/status`, `system/preflight`, `autonomous/readiness`, `market-data/intraday-refresh/status`, `alerts/active`, `execution/summary`, `execution/flow`), plus three optional read-only `select` queries via `docker exec mqk-paper-postgres psql` (reusing the exact pattern already documented in the ledger's own DB-probe section) when `-IncludeDb` is passed. It computes a `verdict` block — `daemon_reachable`, `paper_mode_confirmed`, `live_routing_disabled`, `v2_active_configured`, `v2_active_source_used`, `runtime_uses_session_v2`, `trading_uses_session_v2`, `session_in_window`, `session_window_state`, `runtime_start_allowed`, `overall_ready`, `intraday_refresh_passed`, and `safe_to_continue_to_manual_paper_proof` — from the live `runtime_session_source` summary ASSET-CORE-05D/05E added to `system/status`/`system/preflight`, and from `autonomous/readiness`'s `session_window_state`/`overall_ready` (confirmed to exist only on that route, not on `preflight`, per ASSET-CORE-05E's own test-driven discovery). It never starts/stops the daemon, never arms/disarms, never clears halted state, never calls `/api/v1/ops/action` or any strategy/signal/order/broker/provider route, and never writes to the database or `.env.local`. `docs/runbooks/v2_equity_session_active_market_hours_proof.md` documents the exact pre-market checklist, the one additional env var to layer onto the existing documented paper-startup sequence, the collector command, what `PASS`/`BLOCKED` mean, an explicit "what not to do" list, and how to classify the eventual verdict as `ASSET-CORE-05-MARKET-HOURS-V2-ACTIVE-PROOF CLOSED`/`PARTIAL`/`BLOCKED`.

Validation: PowerShell `Parser::ParseFile` reported zero syntax errors; a dry no-daemon probe (`-AllowOutsideMarketWindow -SkipIntradayRefreshCheck` against the local daemon, which was not running at the time) printed a clear "daemon is not reachable" error, wrote a minimal `daemon_reachable: false` evidence file under `smoke_logs/`, and exited `1` — confirming the fail-clearly/no-mutation contract — without starting anything. The generated dry-run evidence file was inspected and was not staged. No backend Rust, GUI, config, or DB file was touched; this is a scripts/docs/ledger-only patch.

`ASSET-CORE-05` remains `PARTIAL`: the actual operator-supervised, real-wall-clock market-hours proof with `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` configured still has not been run. This patch only prepares the runbook and read-only collector for that proof.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-05F-V2-EQUITY-ACTIVE-PROOF-RUNBOOK-COLLECTOR-01-COMBINED` entry (end of §19).

## 23. BACKTEST-MULTIPLIER-MARGIN-01-COMBINED Closure Note (maintenance)

BACKTEST-MULTIPLIER-MARGIN-01-COMBINED added/proved a multiplier-aware backtest economics seam (`core-rs/crates/mqk-backtest/src/economics.rs`: `BacktestInstrumentEconomics` + pure `notional_micros`/`mark_to_market_value_micros`/`realized_pnl_micros` helpers). Equity behavior remains multiplier=1 and unchanged — the seam is additive only and is not wired into `BacktestEngine`. Futures/options-style multipliers (50 and 100) are proven by synthetic unit tests only; no futures/options registry, broker, execution, or live portfolio path was enabled or modified. `mqk-portfolio` (the accounting engine shared with the live/paper runtime) was not touched. Margin is scaffolded as `Option<i64>` metadata only — read by nothing, enforced nowhere. `BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL` pending engine wiring and broader non-equity backtest readiness.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `BACKTEST-MULTIPLIER-MARGIN-01-COMBINED` entry (end of §19).

## 24. BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED Closure Note (maintenance)

BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED wired the backtest-only economics seam into the backtest run path with default equity multiplier=1. Synthetic multiplier tests prove full-run behavior where implemented. Live/shared portfolio accounting and trading remain unchanged; futures/options are still not enabled.

Wiring is opt-in via a new `BacktestEngine::with_economics(...)` builder method rather than a `BacktestConfig` field, because an exhaustive `BacktestConfig { .. }` struct literal in `mqk-daemon/tests/scenario_backtest_jobs_01.rs` (outside this patch's scope) would have failed to compile the moment a field was added; `BacktestReport` has the same problem via `mqk-artifacts/src/lib.rs`'s own test fixtures. Both structs are therefore untouched. Allocation-cap notional and a new backtest-only shadow ledger (mirroring `mqk_portfolio`'s FIFO cash/P&L logic, proven byte-identical to it at multiplier=1) are multiplier-aware; `BacktestReport.equity_curve`, `metrics.json`/`manifest.json`, and `config_id()`/`run_id` are not — they remain on the un-multiplied `mqk-portfolio` path and carry no economics identity yet. `BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED` entry (end of §19).

## 25. BACKTEST-ECONOMICS-CONFIG-READY-01 Closure Note (maintenance)

BACKTEST-ECONOMICS-CONFIG-READY-01 removed/isolated exhaustive backtest config/report literals so future economics fields can be added safely. It does not change economics behavior, artifacts, daemon/CLI request shape, or live/paper trading. `BacktestReport::test_fixture()` was added (mirroring `BacktestConfig::test_defaults()`); the exhaustive `BacktestConfig` literal in `mqk-daemon/tests/scenario_backtest_jobs_01.rs` and the two exhaustive `BacktestReport` literals in `mqk-artifacts/src/lib.rs` that the prior patch identified are now de-risked, along with two previously undocumented exhaustive `BacktestConfig` literals found in `mqk-backtest/tests/`. A new, larger blocker was found and deliberately left untouched (outside this patch's strict scope): `mqk-promotion/tests/` contains seven exhaustive `BacktestReport` literals across six files — the largest remaining obstacle to a future `BacktestReport` field. `BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `BACKTEST-ECONOMICS-CONFIG-READY-01` entry (end of §19).

## 26. BACKTEST-REPORT-FIXTURE-READY-01-COMBINED Closure Note (maintenance)

BACKTEST-REPORT-FIXTURE-READY-01-COMBINED removed the remaining promotion-test exhaustive `BacktestReport` literals by routing them through `BacktestReport::test_fixture()`. This keeps behavior unchanged while making future economics/report fields safer to add. Direct repo enumeration at current HEAD found **nine** exhaustive literals across six `mqk-promotion/tests/` files (not the seven the prior closure note counted, and one file — `scenario_tie_break_correctness.rs` — that the prior note's per-file list omitted entirely); all nine are now de-risked. No `BacktestReport`/`BacktestConfig` field was added, no promotion logic changed, and no artifact/config_id/run_id behavior changed. `BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`; the last concrete construction-safety blocker ahead of a real economics field is now closed.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `BACKTEST-REPORT-FIXTURE-READY-01-COMBINED` entry (end of §19).

## 27. BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED Closure Note (maintenance)

BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED added a truthful backtest report/artifact economics surface. Default equity behavior remains multiplier=1; synthetic multiplier runs now surface economics in report/artifact output where implemented. Live/shared portfolio accounting and trading remain unchanged.

Concretely: `BacktestReport` gained an `economics: BacktestEconomicsReport` field (multiplier, optional margins, multiplier-aware realized P&L, an always-`false` `margin_enforced` flag). `report.equity_curve` now switches to the multiplier-aware economics curve whenever `contract_multiplier != 1`, closing the exact gap `BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED`'s closure note flagged ("`BacktestReport.equity_curve`... remain on the un-multiplied `mqk-portfolio` path"); it stays byte-identical to the pre-existing curve at multiplier=1. `run_id` is now economics-sensitive — any economics other than exactly `BacktestInstrumentEconomics::equity()` (including margin-only changes) produces a different `run_id` — while `config_id` deliberately stays a pure function of `BacktestConfig` only, since economics is not a `BacktestConfig` field. `metrics.json` and `report.md` (not `manifest.json`, which would require CLI call-site changes not proven necessary) now carry the economics section truthfully. No `BacktestConfig`/`mqk-portfolio`/daemon/CLI/broker/DB file was touched. `BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`: a registry-derived multiplier source and a daemon/CLI/GUI entry point to configure non-default economics for a real run are still open.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED` entry (end of §19).

## 28. BACKTEST-ECONOMICS-CLI-ENTRY-01-COMBINED Closure Note (maintenance)

BACKTEST-ECONOMICS-CLI-ENTRY-01-COMBINED added the first real operator entry point for backtest economics via CLI-only opt-in flags. Default equity behavior remains unchanged; multiplier and margin metadata now flow through the CLI into report/artifact economics output. Daemon/GUI/API, registry-derived multipliers, and live trading remain unwired.

Concretely: `mqk backtest csv` gained three optional flags — `--contract-multiplier`, `--initial-margin-micros`, `--maintenance-margin-micros` — on `BacktestCmd::Csv` only (`core-rs/crates/mqk-cli/src/main.rs` + `src/commands/bkt.rs`). When none are supplied, the engine keeps its pre-existing default `BacktestInstrumentEconomics::equity()` and output is byte-identical to before this patch. When any are supplied, `run_backtest_csv` calls the already-validating `BacktestInstrumentEconomics::new(...)` constructor (proven by the prior `BACKTEST-MULTIPLIER-MARGIN-01-COMBINED` patch) before `engine.run()`/`init_run_artifacts` ever execute, so a non-positive multiplier fails closed with no artifact directory created. `BacktestCmd::CsvSweep` and `BacktestCmd::Db` were not touched — only the CSV single-run command, which needs neither DB nor a provider, gained the flags. No `mqk-backtest/src` or `mqk-artifacts/src` change was needed: the report/artifact economics surface built by `BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED` already existed and only needed a real caller. No `BacktestConfig`/`mqk-portfolio`/daemon/GUI/broker/DB file was touched. `BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`: daemon/GUI/API entry points, a registry-derived multiplier source, and CLI flags on the sweep/DB-backed commands are still open.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `BACKTEST-ECONOMICS-CLI-ENTRY-01-COMBINED` entry (end of §19).

## 29. BACKTEST-ECONOMICS-DB-CLI-ENTRY-01-COMBINED Closure Note (maintenance)

BACKTEST-ECONOMICS-DB-CLI-ENTRY-01-COMBINED extended the backtest economics opt-in path from CSV artifacts to DB-backed CLI backtests. Default equity behavior remains unchanged; multiplier and margin metadata now flow through the DB CLI path into report/artifact economics output. Daemon/GUI/API, registry-derived multipliers, and live trading remain unwired.

Concretely: `mqk backtest db` gained the same three optional flags as `mqk backtest csv` — `--contract-multiplier`, `--initial-margin-micros`, `--maintenance-margin-micros` (`core-rs/crates/mqk-cli/src/main.rs` + `src/commands/bkt.rs`). The CSV economics block was extracted into a shared `build_backtest_economics_from_cli_flags(...)` helper so both commands validate and construct `BacktestInstrumentEconomics` identically; `run_backtest_csv`'s behavior is unchanged (proven by its 7 pre-existing tests passing as-is). In `run_backtest_db`, the economics flags are validated **before** `mqk_db::connect_from_env()` is called, so an invalid `--contract-multiplier` fails closed without attempting a DB connection. No `mqk-backtest/src` or `mqk-artifacts/src` change was needed — `BacktestEngine::with_economics(...)` and the `report.economics` → `metrics.json`/`report.md` surface already existed and were source-agnostic. `BacktestCmd::CsvSweep` still has no economics flags. `BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`: daemon/GUI/API entry points and a registry-derived multiplier source are still open.

**Addendum (same-day follow-up, commit `267442a` + a later re-verification):** local DB-backed proof was initially incomplete — the three DB-backed `mqk-cli` tests gracefully skipped because the local `mqk-test-postgres` Docker container's host-published port (5433) had a stale Docker Desktop port-forward (confirmed not a credential problem: the same password worked via `docker exec` and Docker's internal bridge network). Recreating the container on host port 5434 fixed it; live/paper containers were never touched. Re-running `cargo test -p mqk-cli --test scenario_cli_backtest_db_economics` now passes **5/5, 0 skipped** — the DB-backed proof gap is closed. `README_TECHNICAL.md`/`scripts/reset-mqk-testdb.ps1` were updated to caution that this machine's persistent live/paper containers occupy two of the three documented default proof-DB ports.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `BACKTEST-ECONOMICS-DB-CLI-ENTRY-01-COMBINED` entry (end of §19).

## 30. BACKTEST-ECONOMICS-DAEMON-JOB-REQUEST-01-COMBINED Closure Note (maintenance)

BACKTEST-ECONOMICS-DAEMON-JOB-REQUEST-01-COMBINED added an explicit daemon backtest-job economics request surface. Default equity behavior remains unchanged; multiplier and margin metadata now flow from daemon backtest jobs into `BacktestEngine::with_economics(...)`, `BacktestReport.economics`, `metrics.json`, and `report.md`.

Concretely: `POST /api/v1/backtests/jobs` accepts optional nested `economics` fields (`contract_multiplier`, `initial_margin_micros`, `maintenance_margin_micros`). Omitted `economics` preserves multiplier=1, no margins, and `margin_enforced=false`. Present economics defaults an omitted multiplier to 1 and validates through the existing fail-closed backtest economics constructor; invalid multipliers fail the queued daemon job without successful artifacts. GUI controls, registry-derived multipliers, manifest economics metadata, and live trading remain unwired. No broker/provider/runtime/OMS/risk/`mqk-portfolio` path was changed, and no non-equity trading was enabled.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `BACKTEST-ECONOMICS-DAEMON-JOB-REQUEST-01-COMBINED` entry (end of §19).

## 31. BACKTEST-ECONOMICS-GUI-REGISTRY-01-COMBINED Closure Note (maintenance)

BACKTEST-ECONOMICS-GUI-REGISTRY-01-COMBINED added operator-facing GUI controls for daemon backtest economics and a safe read-only registry economics suggestion path. Default GUI submissions still omit `economics` and preserve equity behavior. Registry suggestions do not enable non-equity trading, and live trading remains untouched.

Concretely: the Backtest Results workflow can now optionally submit `contract_multiplier`, `initial_margin_micros`, and `maintenance_margin_micros` through the daemon's nested `economics` request object. Blank fields preserve the old POST body shape. `metrics.json` economics metadata is rendered truthfully when present; older artifacts show it as not reported. The new backtest-only suggestion route loads the configured v1 registry, converts it to `InstrumentRegistryV2`, validates it, and returns multiplier `1` for current equity/ETF symbols or a truthful unavailable/not-found state. No CLI, live/paper runtime, broker, OMS/outbox/inbox, risk gate, shared portfolio accounting, or DB migration path was changed. `BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL` because manifest economics metadata and non-equity registry-derived economics remain incomplete.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `BACKTEST-ECONOMICS-GUI-REGISTRY-01-COMBINED` entry (end of §19).

## 32. BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01-COMBINED Closure Note (maintenance)

BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01-COMBINED added manifest-level backtest economics metadata and a registry-v2 economics metadata seam for read-only backtest suggestions. Default equity behavior remains unchanged; registry suggestions do not enable non-equity trading, and live/paper execution remains untouched.

Concretely: `manifest.json` now carries the same truthful economics block `metrics.json`/`report.md` already had (`contract_multiplier`, both margins, `margin_enforced`, plus a derived `source` of `default_equity`/`explicit_request`) — written by `mqk_artifacts::write_backtest_report`, so every existing CLI CSV/DB and daemon CSV/`md_bars` backtest path gained it without any call-site change; the live/paper `run` CLI command never calls that function and is unaffected. Old manifests without the field still parse (`#[serde(default)]`). `InstrumentDefinitionV2` gained an opt-in `economics: Option<InstrumentEconomicsMetadataV2>` field (positive-multiplier/non-negative-margin validated, independent of `enabled`/asset class), and the economics-suggestion route now delegates to a pure `backtest_economics_suggestion_for_instrument` helper instead of its old equity-only inline match. Because the route's only production data path (converted v1 `equities.json`) never carries non-equity entries or explicit economics, the explicit-economics and non-equity branches are proven by direct unit tests against hand-built fixtures, not by a production-wired example — `ASSET-CORE-01`'s "no production v2 non-equity registry data exists" gap is unchanged by this patch. `BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`: the multiplier/margin plumbing and now the manifest/registry metadata are real and tested, but no real multi-asset data source or operator-facing non-equity economics authoring path exists yet.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01-COMBINED` entry (end of §19).

## 33. INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED Closure Note (maintenance)

INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED added a separate, read-only `InstrumentRegistryV2` source for backtest economics suggestions. Explicit multiplier/margin metadata can now be proven end-to-end through the daemon route without replacing `equities.json` or enabling non-equity trading.

Concretely: `AppState::instrument_registry_v2_path: Option<String>` is sourced only from `MQK_INSTRUMENT_REGISTRY_V2_PATH`, with no fixed-path default — when unset, `GET /api/v1/backtests/economics-suggestion` is byte-for-byte the pre-existing v1-only route. When set, the route searches the configured v2 source first; a missing or invalid configured source fails closed (`registry_unavailable`/`validation_failed`) rather than silently falling back to v1, and a configured-and-healthy source that simply lacks the requested symbol falls through to the unchanged v1 lookup. A committed reference fixture, `config/instruments/instruments_v2.backtest_suggestions.example.json`, carries three disabled/non-tradable example instruments (two test futures, one test crypto pair) with explicit `contract_multiplier`/margin economics — proven by direct `mqk-md` tests that load and validate the real file, not just a string literal. The response gained `asset_class`/`enabled`/`paper_trading_enabled`/`live_trading_enabled` fields so an operator can never mistake a disabled/non-equity suggestion for trading permission; the GUI's "Load registry economics" hint surfaces this truthfully via a small pure helper and still never auto-submits a suggestion into the job form. `ASSET-CORE-01`'s "no production v2 non-equity registry data exists" gap is **narrowed** by this patch (a real, separate, committed file can now be read end-to-end) but not closed in the sense of being wired into any trading path — `InstrumentRegistryV2` is still never read by `mqk-execution`/`mqk-runtime`/`mqk-risk`/`mqk-portfolio`/broker adapters, and `equities.json` remains the sole source of trading truth. `BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL` for the same reason.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED` entry (end of §19).

## 34. ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED Closure Note (maintenance)

ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED made the separate `InstrumentRegistryV2` source (`INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED`) and the existing static asset-capability matrix (`ASSET-CAPABILITY-MATRIX-01`) operator-visible through read-only daemon/GUI status surfaces. Operators can now see whether a v2 source is configured, valid, suggestion-only, and disabled for trading. This does not replace `equities.json`, does not feed live/paper trading, and does not enable non-equity execution.

Concretely: a new route, `GET /api/v1/system/instrument-registry-v2-source/status`, is distinct from `ASSET-CORE-01C`'s `GET /api/v1/system/instrument-registry-v2/status` — the latter diagnoses the v1→v2 *conversion* of `equities.json` and is unaffected by `MQK_INSTRUMENT_REGISTRY_V2_PATH`; the new route reads only `AppState::instrument_registry_v2_path` and reports `not_configured`/`configured_valid`/`registry_unavailable`/`validation_failed` with full instrument/asset-class/enablement counts, proven against the same committed `instruments_v2.backtest_suggestions.example.json` fixture `INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED` introduced (2 future + 1 crypto, all disabled, all carrying economics metadata). `used_for_trading`/`enabled_for_live_trading`/`enabled_for_paper_trading` are hardcoded `false` on every branch. Separately, the GUI's `MetadataSummary` type had silently dropped the daemon's existing `asset_capability_matrix` field on every fetch cycle since `ASSET-CAPABILITY-MATRIX-01` shipped it — now typed and rendered. Both the new route's status and the capability matrix are rendered as two new self-fetching, fail-closed panels on the "Backtest Results" screen (the only in-scope screen with both existing registry/economics UI and file-scope access); a dedicated System/Settings screen would be the more natural home but sits outside this patch's strict file scope, so this placement is recorded as an explicit, intentional `PARTIAL` rather than a silent compromise. `ASSET-CORE-01`'s "v2 registry source is not production trading input" status is unchanged by this patch — visibility, not trading wiring, was the goal.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED` entry (end of §19).

## 35. GUI-SYSTEM-STATUS-SURFACE-01-COMBINED Closure Note (maintenance)

GUI-SYSTEM-STATUS-SURFACE-01-COMBINED moved/surfaced the InstrumentRegistryV2 source status and Asset Capability Matrix onto the existing GUI `Settings / Operations` operator surface. The GUI now shows registry source health and disabled non-equity capability truth without relying on Backtest Results placement or curl/API-only inspection. No backend or trading behavior changed.

Concretely: the repo already had `Settings / Operations` registered as an operator screen and left-rail target, rendering daemon endpoint and operations metadata. The read-only registry-v2 source status and asset capability matrix panels now live in `features/system/*` and render from that surface; Backtest Results no longer owns those unrelated status panels. The helper/tests still prove `not_configured`/`configured_valid`/`registry_unavailable`/`validation_failed` labels, unavailable/fail-closed rendering, and disabled non-equity capability truth. No daemon route, live/paper runtime, broker/provider, OMS/outbox/inbox, risk, portfolio, DB migration, or non-equity trading enablement changed.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `GUI-SYSTEM-STATUS-SURFACE-01-COMBINED` entry (end of §19).

## 36. ASSET-CORE-02-ORDER-INTENT-V2-FOUNDATION-01-COMBINED Closure Note (maintenance)

ASSET-CORE-02-ORDER-INTENT-V2-FOUNDATION-01-COMBINED hardened the inert `OrderIntentV2` / `ExecutionIntentV2` scaffold with pure validation/routability helpers and model-level multi-asset fixtures. The v2 model remains research/foundation-only and is not wired to live or paper trading; non-equity remains disabled.

Concretely: `OrderIntentV2` now carries additive v2-only model metadata for instrument identity, contract shape, order type/prices, time in force, strategy/source metadata, and research-only marking. `IntentV2Validation` separates structural validity from routability via `IntentV2Routability::{ResearchOnly, EquityRoutableCandidate, DisabledAssetClass, Invalid}`. Equity and ETF-as-equity fixtures validate as model-level candidates; crypto, future, option, and forex fixtures validate structurally but return disabled/not-routable. Invalid quantity and missing required price fixtures fail with explicit reason codes. A caller-supplied routing request flag cannot make disabled non-equity routable.

No daemon/API/GUI status surface was added in this slice; the change stayed in `mqk-execution` model/tests plus docs. No broker adapter, runtime, OMS/outbox/inbox, risk, portfolio, DB migration, provider, registry trading path, or current equity lifecycle code was changed.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-02-ORDER-INTENT-V2-FOUNDATION-01-COMBINED` entry (end of §6).

## 37. ASSET-CORE-03-RISK-ROUTER-FOUNDATION-01-COMBINED Closure Note (maintenance)

ASSET-CORE-03-RISK-ROUTER-FOUNDATION-01-COMBINED added a pure asset-aware risk policy/router foundation for equity, ETF-as-equity, crypto, futures, options, forex, and rates/fixed-income scaffolds. The model remains foundation-only and is not wired into production live/paper execution; existing fail-closed disabled-asset gates remain the active enforcement boundary and non-equity remains disabled.

Concretely: `mqk-execution` now exposes static `AssetRiskPolicy` summaries, `AssetRiskRouteDecision`, and a pure `OrderIntentV2` bridge that validates intent structure before policy routing. Equity and ETF-as-equity fixtures classify only as model-level `AllowedEquity` candidates. Crypto, future, option, and forex fixtures remain `DisabledAssetClass`; rates/fixed-income remains a research-only scaffold unless represented by current equity/ETF metadata. Caller-supplied routing flags cannot force disabled non-equity to become allowed.

No daemon/API/GUI status surface was added in this slice. No broker adapter, runtime, OMS/outbox/inbox, `mqk-risk`, `RiskRequestContext`, portfolio accounting, DB migration, provider, registry trading path, or current equity order lifecycle code was changed.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-03-RISK-ROUTER-FOUNDATION-01-COMBINED` entry (end of §6).

## 38. SHORT-SIDE-EXTERNAL-SIGNAL-WIRING-01-COMBINED Closure Note (maintenance)

SHORT-SIDE-EXTERNAL-SIGNAL-WIRING-01-COMBINED wired short-entry policy into the external `strategy_signal` path, added a read-only shortable-preflight broker asset route, and extended canonical paper flatten to close short positions with buy-to-cover orders. Short opens remain default-off/fail-closed unless the existing capital-policy JSON explicitly enables them and the broker asset preflight proves the symbol is tradable/shortable/easy-to-borrow.

Concretely: `routes/strategy.rs` now classifies external signal order intent against the current execution snapshot before outbox enqueue, denies sell-from-flat/sell-beyond-long short opens when policy or preflight proof is missing, and preserves sell-to-reduce-long behavior. `GET /api/v1/broker/assets/:symbol/shortable-preflight` reports read-only truth states (`active`, `not_configured`, `unsupported_adapter`, `symbol_not_found`, `broker_unavailable`, `query_failed`) without exposing submit/cancel/replace behavior. Alpaca integration only added `GET /v2/assets/{symbol}` asset metadata fetch. `flatten-paper-positions` now accepts negative paper positions and enqueues canonical buy-to-cover close JSON, while rejecting duplicate/blank-symbol snapshot ambiguity before enqueue.

Validation was targeted only: the new external signal gate tests, preflight route tests, canonical flatten tests, existing short-entry/intent/preflight/flatten/lifecycle/reconcile/Alpaca-parser regressions, asset-class guard regressions, and the requested daemon/execution/Alpaca clippy plus rustfmt checks passed. The default target directory was blocked by an already-running local `mqk-daemon.exe`, so proof used `C:\tmp\mqk-target-short-side`. No daemon live/paper runtime, provider/live/paper smoke, broker submit/cancel/replace, full workspace test, DB migration, or paper/live order was run.

Status: `CLOSED_LOCAL / PARTIAL`. The local short-side external signal and flatten gaps are closed. Market-hours proof remains partial because the separate stale intraday data/provider freshness gap still blocks a trustworthy live-market retry.

Follow-up: `INTRADAY-MD-PROVIDER-FRESHNESS-TRUTH-01-COMBINED` closes the local truth gap. The refresher/status surface now fails `all_passed` when provider success still leaves stale intraday completed bars, with explicit stale/no-row/missing-bar reason codes. A market-hours short proof retry still requires a real provider refresh that produces current-session completed 5m bars; no proof was run in that patch.

## 39. ASSET-CORE-04A-PORTFOLIO-INSTRUMENT-ECONOMICS-MODEL-01-COMBINED Closure Note (maintenance)

ASSET-CORE-04A-PORTFOLIO-INSTRUMENT-ECONOMICS-MODEL-01-COMBINED started `ASSET-CORE-04` (Multi-Asset Portfolio Ledger — §5's highest-risk, XL-difficulty item, "touches live capital accounting invariants") with its safest possible slice: a pure, default-unused, position-level economics model in a new `mqk-portfolio/src/instrument_economics.rs` module, with zero production callers anywhere in the workspace. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

Concretely: `InstrumentEconomics`/`PositionEconomicsInput`/`PositionEconomicsValue`/`value_position_economics` make explicit, and generalize, the implicit equity assumptions `mqk-portfolio/src/accounting.rs` and `metrics.rs` already rely on (whole shares, single currency, multiplier always 1). Equity multiplier=1 reproduces existing un-multiplied `qty * price` math exactly; futures/options-style multipliers (50x, 100x) and fractional/crypto-style quantities are representable as pure math without enabling any non-equity trading (the value type has no enablement field of any kind). Missing mark, invalid multiplier, missing currency, and cross-currency mismatches all fail closed with an explicit `InstrumentEconomicsTruthState` — this model never fabricates a notional or a currency conversion. All arithmetic is `i128`-checked with explicit overflow reporting, never `f64`, never a silent wrap. `compute_portfolio_weights` (`PORTFOLIO-LIVE-WEIGHTS-01`) and `evaluate_sector_risk` (`ETF-RISK-CLOSURE-01`) were not modified and are proven unchanged by both a direct in-file regression smoke test and their own pre-existing dedicated scenario files passing byte-for-byte.

**Not resolved (`ASSET-CORE-04` remains far from closed — still effectively the §5/§12 "highest-blast-radius patch in this entire roadmap"):** `PortfolioState` (the live/paper accounting type) is completely unchanged; there is no DB schema or migration for multi-currency/multi-asset accounting; no runtime/orchestrator cutover; no broker/account sync changes; no real margin model (deliberately narrower in scope than even `mqk-backtest::economics::BacktestInstrumentEconomics`'s margin scaffold); no currency conversion (by design); and no bridge from `mqk_md::instrument_registry_v2::InstrumentDefinitionV2` to this new model — `mqk-portfolio` has zero Cargo dependencies today and adding one to `mqk-md` was judged out of this foundation patch's scope. Deferred as `ASSET-CORE-04B`.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-04A-PORTFOLIO-INSTRUMENT-ECONOMICS-MODEL-01-COMBINED` entry (end of §19).

**Safety confirmation:** no daemon runtime started; no provider/broker network calls; no live or paper order submitted; no DB read, write, or migration; no strategy thresholds changed; no broker/OMS/risk/runtime/DB-schema code touched; no non-equity asset class enabled; `.env.local` untouched; no smoke logs or generated evidence staged.

## 40. ASSET-CORE-04B-REGISTRY-V2-ECONOMICS-BRIDGE-01-COMBINED Closure Note (maintenance)

ASSET-CORE-04B-REGISTRY-V2-ECONOMICS-BRIDGE-01-COMBINED closed the registry-v2 bridge gap `ASSET-CORE-04A` deferred: a pure, read-only translation from `mqk_md::instrument_registry_v2::InstrumentDefinitionV2` to `ASSET-CORE-04A`'s `mqk_portfolio::InstrumentEconomics`, plus a read-only diagnostic route, `GET /api/v1/system/instrument-economics/status`. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

Concretely: `instrument_v2_to_economics`/`bridge_instrument_registry_v2_to_economics` live in `mqk-daemon/src/state/instrument_economics_bridge.rs` — `mqk-daemon` already depends on both `mqk-md` and `mqk-portfolio`, so this adds zero new Cargo dependency edges, and `mqk-portfolio` stays at zero dependencies exactly as `ASSET-CORE-04A` recommended. The real production registry (88 equity/ETF rows) bridges 88/88 with multiplier=1/USD; futures/options fixtures bridge their explicit raw multiplier scaled into the `MICROS_SCALE` convention; a crypto fixture bridges as model-only with a fractional-capable `quantity_scale`; a same-currency forex fixture bridges safely while a mismatched-currency fixture truthfully refuses (`currency_conversion_unsupported`) since the model has only one currency field and cannot represent a genuinely cross-currency instrument. Missing/invalid multiplier, missing currency, and unsupported asset class all fail closed with `economics: None` — never a fabricated default. `trading_enabled_by_bridge` is hardcoded `false` on every code path, proven across every asset class and every failure mode in one consolidated test. The route's `bridge_model_only`/`trading_uses_instrument_economics`/`runtime_uses_instrument_economics`/`risk_uses_instrument_economics`/`order_path_uses_instrument_economics` flags are always `true`/`false` as documented; no runtime, risk, order, broker, or accounting path calls this bridge. `ASSET-CORE-04A`'s economics model, and the `ASSET-CORE-01C`/`05B`/`05C` registry-v2/session routes this patch's route pattern was copied from, are all proven unchanged (regression suites green byte-for-byte).

**Not resolved (`ASSET-CORE-04` remains far from closed — still effectively the §5/§12 "highest-blast-radius patch in this entire roadmap"):** `PortfolioState` (the live/paper accounting type) is completely unchanged; there is no DB schema or migration; no runtime/orchestrator cutover (`mqk-runtime` never calls this bridge); no broker/account sync changes; no real margin model; no currency conversion (cross-currency instruments are refused, not converted, by design); no non-equity asset class enabled; and no portfolio-level NAV aggregation that composes bridged `InstrumentEconomics` values across a multi-asset book the way `compute_portfolio_weights` already does for equities. Recommended next slice: `ASSET-CORE-04C` (that NAV aggregation).

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-04B-REGISTRY-V2-ECONOMICS-BRIDGE-01-COMBINED` entry (end of §19).

## 41. ASSET-CORE-04C-MULTI-ASSET-NAV-AGGREGATION-MODEL-01-COMBINED Closure Note (maintenance)

ASSET-CORE-04C-MULTI-ASSET-NAV-AGGREGATION-MODEL-01-COMBINED closed the portfolio-level NAV/exposure aggregation gap `ASSET-CORE-04B` deferred: a pure, default-unused model that composes multiple already-valued `mqk_portfolio::PositionEconomicsValue` rows into a portfolio-level NAV, gross/net exposure, per-position weight, and asset-class/currency exposure breakdown. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

Concretely: `aggregate_portfolio_economics`/`PortfolioEconomicsInput`/`PortfolioEconomicsPositionRow`/`PortfolioEconomicsExposureRow`/`PortfolioEconomicsSnapshot`/`PortfolioEconomicsTruthState` live in a new `mqk-portfolio/src/portfolio_economics.rs`, exported from `lib.rs` alongside `ASSET-CORE-04A`'s existing block — `mqk-portfolio` stays at zero Cargo dependencies. Equity-only aggregation composes `ASSET-CORE-04A`'s multiplier=1 equity valuation at the portfolio level; long/short signs are preserved in net exposure while gross exposure uses absolute value; future/option/crypto fixtures aggregate together by asset class without any enablement signal anywhere in the model. Any position whose `quote_currency` differs from the portfolio's `account_currency` fails the *whole* snapshot closed as `CurrencyConversionUnsupported` — proven even when that position individually valued `Active` in its own currency, showing the aggregator re-checks currency itself rather than trusting an upstream result. Any currency-consistent position that is not itself `Active` (missing mark, empty asset class, etc.) fails the whole snapshot closed as `PositionValueUnavailable`, while each position's own already-known value is still surfaced per-row — mirroring `compute_portfolio_weights`'s `"missing_marks"` precedent. `nav_micros <= 0` fails closed as `NavUnavailable` without blocking gross/net exposure or weight-less exposure breakdowns, mirroring `compute_portfolio_weights`'s `"nav_unavailable"` precedent exactly. All NAV/gross/per-asset-class/per-currency summation is `checked_add`-based and fails closed to an explicit `Overflow` state on the first detected overflow — a stricter, non-saturating posture than `compute_portfolio_weights`, proven with two near-`i128::MAX` fixtures. Asset-class and currency exposure breakdowns use a sorted `BTreeMap` accumulator, so output ordering is deterministic regardless of input position order.

**Not resolved (`ASSET-CORE-04` remains far from closed — still effectively the §5/§12 "highest-blast-radius patch in this entire roadmap"):** `PortfolioState` (the live/paper accounting type) is completely unchanged; there is no DB schema or migration; no runtime/orchestrator wiring (`mqk-runtime` never calls this module); no broker/account sync; no real margin model; no currency conversion (cross-currency positions are refused, not converted, by design); no non-equity asset class enabled; no daemon/API route exposing this aggregation (deliberately deferred to a future `ASSET-CORE-04D`); and no integration with the `ASSET-CORE-04B` registry-v2 bridge at runtime (this module composes `PositionEconomicsValue` rows however the caller obtains them).

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-04C-MULTI-ASSET-NAV-AGGREGATION-MODEL-01-COMBINED` entry (end of §19).

**Safety confirmation:** no daemon runtime started; no provider/broker network calls; no live or paper order submitted; no DB read, write, or migration; no strategy thresholds changed; no broker/OMS/risk/runtime/DB-schema code touched; no non-equity asset class enabled (`non_equity_enabled_count: 0` for the real registry; the bridge has no enablement concept); `.env.local` untouched; no smoke logs or generated evidence staged.

## 42. ASSET-CORE-04D-PORTFOLIO-ECONOMICS-READONLY-STATUS-01-COMBINED Closure Note (maintenance)

ASSET-CORE-04D-PORTFOLIO-ECONOMICS-READONLY-STATUS-01-COMBINED closed the read-only operator status route gap `ASSET-CORE-04C` recommended next: a thin diagnostic route, `GET /api/v1/portfolio/economics/status`, composing the `ASSET-CORE-04B` registry-v2 bridge with the `ASSET-CORE-04A`/`04C` economics model against the live in-memory execution snapshot (positions/cash) and the latest completed `md_bars` mark per non-flat symbol — the same sources `/api/v1/portfolio/live-weights` already uses. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

Concretely: `portfolio_economics_status` lives in `mqk-daemon/src/routes/portfolio.rs` alongside the existing `portfolio_live_weights` it mirrors for position/cash/mark sourcing. A single equity position against a real, DB-seeded completed bar produces correct `active` NAV/gross/net/weight via the full 04B-bridge -> 04A-valuation -> 04C-aggregation chain; long and short positions preserve sign in net exposure while gross exposure uses the absolute sum; a missing completed mark (DB present) reports `missing_marks` while the same case with no DB pool at all reports the distinct `db_unavailable`, mirroring `live-weights`' existing precedent; an incomplete-only bar is confirmed never read as a mark. A position whose symbol is absent from the registry — or whose registry row fails to bridge — fails the whole portfolio closed as `position_value_unavailable` with an explicit `reason_code`, `asset_class=""`, and `quote_currency=""`: no fabricated equity multiplier, currency, or notional. `?symbol=`/`?limit=` filter and truncate the returned `positions` rows only, never the full-portfolio counts. The route's `model_only`/`trading_uses_portfolio_economics`/`runtime_uses_portfolio_economics`/`risk_uses_portfolio_economics`/`order_path_uses_portfolio_economics` flags are always `true`/`false`/`false`/`false`/`false`; `account_currency` is a hardcoded `"USD"` constant since `PortfolioSnapshot` carries no account-currency field. `ASSET-CORE-04A`/`04B`/`04C`'s pure models, `live-weights`, and the GUI/route-contract regression suites are all proven unchanged (regression suites green byte-for-byte, including against the real local paper Postgres for the DB-backed cases).

**Not resolved (`ASSET-CORE-04` remains far from closed — still effectively the §5/§12 "highest-blast-radius patch in this entire roadmap"):** `PortfolioState` (the live/paper accounting type) is completely unchanged; there is no DB schema or migration; no runtime/orchestrator decision wiring (`mqk-runtime` never calls this route); no broker/account sync; no real margin model; no currency conversion (cross-currency positions are refused, not converted, by design, and not even constructible through this route's v1-registry-only input); no non-equity asset class enabled; no real non-equity market-data source (still the blocking prerequisite before any futures/options/crypto/forex math in this model could be exercised against real marks); and no risk/order path consuming this route's output (zero callers anywhere in the workspace, by design). Recommended next slice: a real non-equity market-data source decision, which remains the single highest-leverage step before any further `ASSET-CORE-04` math can move past hand-built fixtures.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-04D-PORTFOLIO-ECONOMICS-READONLY-STATUS-01-COMBINED` entry (end of §19).

**Safety confirmation:** no daemon runtime started; no provider/broker network calls; no live or paper order submitted; no DB mutation except isolated, cleaned-up `md_bars` test-fixture rows on dedicated test-only `(symbol, timeframe)` pairs; no DB migration; no strategy thresholds changed; no broker/OMS/risk/runtime/DB-schema code touched; no non-equity asset class enabled (`non_equity_enabled_count: 0` for the real registry; the route has no enablement concept); `.env.local` untouched; no smoke logs or generated evidence staged.

## 43. ASSET-CORE-04E-NON-EQUITY-MARK-DATA-SOURCE-DECISION-01-COMBINED Closure Note (maintenance)

ASSET-CORE-04E-NON-EQUITY-MARK-DATA-SOURCE-DECISION-01-COMBINED answered the single highest-leverage gap `ASSET-CORE-04D`'s own closure note (§42) named: which real non-equity mark-data source/path should be first. This patch is a decision/spec patch only — it builds none of the source it recommends. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Decision:** crypto spot marks (`BTC/USD`, `ETH/USD`), sourced first from a local, committed, no-network CSV fixture loaded through the existing `mqk-md/src/ingest_csv.rs` path into the existing (unmigrated) `md_bars` schema, behind a new but still-disabled registry-v2 entry. This independently re-derives, from direct current-repo evidence rather than from this audit, the same conclusion §1/§7-§9 above already reached: crypto is the cheapest non-equity vertical, and the decision's own grounding additionally found that `mqk-execution::asset_risk_policy::crypto_policy()` (commit `04a8fb50`, predating this audit's own original survey) is the only non-equity policy with both `requires_margin_model: false` and `requires_contract_multiplier: false` — a concrete, code-level confirmation of this audit's "crypto first" recommendation that did not exist in those terms at original audit time. Futures, options, and forex were evaluated and explicitly deferred (per-lane rationale in the decision doc, `docs/specs/asset_core_04e_non_equity_mark_source_decision.md` §3/§5); a live/network crypto provider (TwelveData-crypto or CoinLore) was also deferred, not rejected, since neither is verified for crypto today and this patch is barred from network calls.

**Resolved:** the §5/§42 "no real non-equity market-data source" blocker now has a concrete, repo-grounded next step instead of an open-ended gap — `CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01`, sub-slicing this audit's own `CRYPTO-DATA-01` roadmap entry (§5 Phase 3) into its smallest safe first proof, and folding `CRYPTO-REGISTRY-01` (§5 Phase 3, "cheapest non-equity asset class to start") into the same patch as a side effect.

**Not resolved (`ASSET-CORE-04` remains far from closed, and the crypto lane in §5/§7/§9 remains `MISSING`):** no registry-v2 entry, CSV fixture, or real (non-fixture-economics) mark of any kind exists yet — this patch decided the lane, it did not build it. `CRYPTO-REGISTRY-01` and `CRYPTO-DATA-01` (§5 Phase 3) remain `MISSING` until the recommended next patch lands. No DB migration, provider implementation, or trading enablement was added or proposed as already-authorized.

**Full detail, exact decision rationale, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-04E-NON-EQUITY-MARK-DATA-SOURCE-DECISION-01-COMBINED` entry; full decision document at `docs/specs/asset_core_04e_non_equity_mark_source_decision.md`; machine-readable artifact at `docs/specs/asset_core_04e_non_equity_mark_source_decision.json`.

## 44. CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01 Closure Note (maintenance)

CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01 implemented the first slice `ASSET-CORE-04E` (§43 above) recommended: one real but disabled `BTC/USD` registry-v2 entry, one deterministic local CSV fixture, and a no-network proof that a real mark reaches the unmodified `ASSET-CORE-04A`/`04B`/`04C` model chain. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** `config/instruments/instruments_v2.crypto_local_marks.example.json` adds one real (non-`_TEST`) disabled `CryptoPair{BTC,USD}` row; `core-rs/crates/mqk-md/tests/fixtures/crypto_btcusd_1d_local.csv` adds 3 deterministic `BTC/USD` `1D` bars with real epoch-second timestamps. A pure in-memory test chain (`mqk-daemon/tests/scenario_crypto_portfolio_economics_local_marks_01a.rs`) parses the CSV, bridges the registry row through the existing `ASSET-CORE-04B` bridge, values a `0.01 BTC` position through `ASSET-CORE-04A`, and aggregates it through `ASSET-CORE-04C` — producing an `Active` snapshot with a `$441.00` notional, a `"crypto"` exposure bucket, and a `"USD"` exposure bucket, with every enablement flag `false` throughout.

**Notable repo-truth finding:** `mqk-md/src/ingest_csv.rs` — the parser this entire lane depends on — existed with 19 passing internal unit tests but was never declared as a module in `mqk-md/src/lib.rs`, so it was dead/uncompiled code with zero reachability from any caller, in this crate or any other. This patch's only non-fixture code change is the single line `pub mod ingest_csv;`, which made those 19 pre-existing tests executable for the first time and unblocked this lane without writing any new parsing logic, exactly matching the mission discipline of reusing an existing parser rather than inventing one. This is the kind of gap `audit_repo_truth_rules.md`'s "verify against committed HEAD before asserting closure" standard exists to catch — the 04E decision doc had described `ingest_csv.rs` as part of "the existing CSV-ingestion path" in good faith from a source-existence read, without confirming module-tree reachability.

**Not resolved — `ASSET-CORE-04D` route-level crypto proof remains structurally blocked, not merely deferred:** `portfolio_economics_status` sources its registry exclusively via `load_instrument_registry` (v1) → `convert_v1_registry_to_v2`, and `convert_tracked_instrument_to_v2` unconditionally stamps an `Equity`/`Etf` contract onto every row regardless of its `asset_class` string — so no v1-sourced row can ever carry a `CryptoPair` contract, and `validate_contract_v2` would reject one that tried. A real crypto position can only reach that HTTP route once a registry-v2-shaped route-input seam is built; this patch correctly did not force that change (it would have required touching forbidden `mqk-daemon/src/routes` or `mqk-md/src/instrument_registry.rs` conversion logic beyond this patch's scope).

**Not resolved (`ASSET-CORE-04` remains far from closed; crypto lane moves `MISSING` → `PARTIAL`, not `CLOSED`):** no live/network crypto provider; no production registry-v2 cutover (zero production callers anywhere); no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface; no DB persistence proof for crypto bars (this patch's chain proof is deliberately pure in-memory); no `ASSET-CORE-04D` route-level crypto proof (see above).

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01` entry (end of the `ASSET-CORE-04` section).

**Safety confirmation:** no daemon runtime started; no provider/broker network calls; no API credits spent; no live or paper order submitted; no DB read, write, or migration; no strategy thresholds changed; no broker/OMS/risk/runtime/DB-schema code touched; no crypto/futures/options/forex trading enabled; `config/instruments/equities.json` and `.env.local` untouched; no smoke logs or generated evidence staged.

**Safety confirmation:** no daemon runtime started; no provider/broker network calls; no API credits spent; no live or paper order submitted; no DB read, write, or migration; no strategy thresholds changed; no broker/OMS/risk/runtime/DB-schema/`mqk-md`-provider-implementation/`mqk-portfolio`/GUI code touched; no non-equity asset class enabled; `.env.local` untouched; no smoke logs or generated evidence staged.

## 45. CRYPTO-DATA-01B-DB-BACKED-LOCAL-MARK-PERSISTENCE-01 Closure Note (maintenance)

CRYPTO-DATA-01B-DB-BACKED-LOCAL-MARK-PERSISTENCE-01 continued the crypto local-mark lane by extending `CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01`'s pure in-memory chain proof to a real DB-backed persistence + readback proof, with zero production code, fixture, or registry changes. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** a single new test file, `core-rs/crates/mqk-daemon/tests/scenario_crypto_local_mark_db_persistence_01b.rs`, reuses `CRYPTO-DATA-01A`'s committed registry-v2 and CSV fixtures unmodified. It parses the CSV via the existing `mqk_md::ingest_csv::parse_csv_file`, maps the result into `mqk_db::ProviderBar`, and inserts it into the real local paper Postgres (`miniquantdesk_paper`, port 5440) via the existing `mqk_db::ingest_provider_bars_to_md_bars` upsert helper — no migration, since `md_bars` and its provider-metadata columns already existed at HEAD. It then reads the row back via the existing `mqk_db::fetch_recent_completed_bars_for_strategy`, proving the exact `BTC/USD` symbol and `44,100.00` close survive the round-trip, that an incomplete newer probe row is excluded by the query's own `is_complete` filter, and that an unrelated symbol's rows do not leak into the lookup. The DB-read close is then fed into the same unmodified `ASSET-CORE-04A`/`04B`/`04C` chain `CRYPTO-DATA-01A` already proved in-memory, producing the same `$441.00`/`crypto`/`USD` result from a real persisted-and-read mark instead of an in-memory CSV parse. A final check confirms zero `oms_outbox` writes anywhere in the test.

**Notable design note:** `db01`-`db06`/`chain_db01`-`chain_db04`/`safety01` all key off the same `(BTC/USD, 1D)` row family, and two of them (`db05`/`db06`) must insert extra probe rows into that same family. Since `cargo test` runs `#[tokio::test]` functions concurrently by default within one binary, these were composed into one sequential test rather than several independent ones, to avoid the same kind of cross-test race `scenario_md_fetch_returns_ordered_rows.rs` already avoids by the same means. The one test that needs the *opposite* precondition (`db07`, zero rows) runs separately against a private timeframe tag no other test touches.

**Not resolved — `ASSET-CORE-04D` route-level crypto proof remains structurally blocked, unchanged from `CRYPTO-DATA-01A`:** same root cause — `portfolio_economics_status` sources its registry exclusively via the v1 loader and `convert_v1_registry_to_v2`, which cannot produce a `CryptoPair` contract from any v1 row. This patch did not touch `routes/portfolio.rs`, `api_types.rs`, or `routes.rs`.

**Not resolved (`ASSET-CORE-04` remains far from closed; crypto lane stays `PARTIAL`, not `CLOSED`):** no live/network crypto provider; no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface; no `ASSET-CORE-04D` route-level crypto proof (see above); no scheduler or recurring ingest job for crypto bars (this patch's insert is a one-shot test action).

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01B-DB-BACKED-LOCAL-MARK-PERSISTENCE-01` entry (end of the `ASSET-CORE-04` section).

## 46. ASSET-CORE-04F-PORTFOLIO-ECONOMICS-V2-REGISTRY-SEAM-01-COMBINED Closure Note (maintenance)

ASSET-CORE-04F-PORTFOLIO-ECONOMICS-V2-REGISTRY-SEAM-01-COMBINED closed the one structural blocker `CRYPTO-DATA-01A` (§44), `CRYPTO-DATA-01B` (§45), and `ASSET-CORE-04D` (§42) all independently named: `GET /api/v1/portfolio/economics/status` could only ever source its registry from the v1 file converted via `convert_v1_registry_to_v2`, which always stamps an `Equity`/`Etf` contract regardless of `asset_class`, so no real crypto/future/option/forex row could ever reach the route. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** a new `?registry_source=v2` query-param lane (default/omitted/`legacy` unchanged) lets the route load a server-side-configured registry-v2 document (`AppState::portfolio_economics_registry_v2_path`, sourced only from `MQK_PORTFOLIO_ECONOMICS_REGISTRY_V2_PATH` — never a client-supplied path) instead of the v1-converted one. Everything downstream of registry load (the `ASSET-CORE-04B` bridge, `ASSET-CORE-04A` valuation, `ASSET-CORE-04C` aggregation) was already generic over the registry's origin, so the entire blocker closed with zero changes to bridging/valuation/aggregation logic — only a new registry-*source* resolver was needed. A missing-configured-path, missing-file, malformed-file, invalid-registry, or unrecognized-`registry_source` request all fail closed (`registry_unavailable`/`registry_invalid`/the new `invalid_registry_source`) with no fallback to legacy and no panic. Querying the route with `registry_source=v2` against the committed disabled `BTC/USD` fixture (`CRYPTO-DATA-01A`'s `instruments_v2.crypto_local_marks.example.json`) and a DB-seeded completed `md_bars` row now returns a real `active` valuation through the actual HTTP route — `asset_class="crypto"`, `quote_currency="USD"`, correct notional, a `"crypto"` exposure bucket and a `"USD"` exposure bucket — the first time any crypto position has been valued through this specific route rather than only at the pure-model layer.

**Documented scope limit:** the mission's illustrative `0.01 BTC` notional example could not be reproduced at the route level — `mqk_runtime::observability::PositionSnapshot::net_qty` is a whole-unit `i64` with no fractional representation, and the route already scales every position (any asset class) by `MICROS_SCALE` from that whole-unit quantity. Reproducing the fractional example would require changing `mqk-runtime`, outside this patch's scope; the route-level test instead uses a whole `1 BTC` position at the same documented `$44,100.00` fixture mark, and the fractional case remains correctly proven at the pure-model layer by `CRYPTO-DATA-01A`/`01B`, unaffected by this patch.

**Resolved:** the §42/§44/§45 "`ASSET-CORE-04D` route-level crypto proof remains structurally blocked" finding, repeated identically across three prior closure notes, no longer applies — a real (non-v1-sourced) crypto row can now be valued through the actual HTTP route, behind an explicit, default-off, fail-closed query parameter.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** registry-v2 is still not this route's (or any route's) production default; `BTC/USD` remains `enabled=false`/disabled/model-only; no production portfolio-ledger cutover; no DB schema or migration; no runtime/orchestrator wiring; no broker/account sync; no real margin model; no currency conversion; no live/network crypto provider; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface; no scheduler or recurring ingest job for crypto bars; no operator decision yet on whether/when registry-v2 should become a production default rather than an explicitly-requested alternate lane.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-04F-PORTFOLIO-ECONOMICS-V2-REGISTRY-SEAM-01-COMBINED` entry (end of the `ASSET-CORE-04` section).

**Safety confirmation:** no daemon runtime started; no provider/broker network calls; no API credits spent; no live or paper order submitted; no DB migration; the only DB mutation was this patch's own isolated, cleaned-up `BTC/USD` test-fixture rows at a private timeframe tag; no strategy thresholds changed; no broker/OMS/risk/runtime code touched; no crypto/futures/options/forex trading enabled; the registry-v2 fixture, `equities.json`, and `.env.local` all untouched; no smoke logs or generated evidence staged.

**Safety confirmation:** no daemon runtime started; no provider/broker network calls; no API credits spent; no live or paper order submitted; no DB migration; the only DB mutation anywhere was this test's own isolated, cleaned-before-and-after fixture rows against the local paper DB; no strategy thresholds changed; no broker/OMS/risk/runtime/DB-schema code touched; no crypto/futures/options/forex trading enabled; `config/instruments/equities.json` and `.env.local` untouched; no smoke logs or generated evidence staged.

## 47. CRYPTO-DATA-01C-PROVIDER-INGEST-SCHEDULER-DESIGN-01-COMBINED Closure Note (maintenance)

CRYPTO-DATA-01C-PROVIDER-INGEST-SCHEDULER-DESIGN-01-COMBINED continued the crypto data lane after `ASSET-CORE-04F` (§46) by deciding the next implementation lane for real `BTC/USD`/`ETH/USD` marks reaching `md_bars`. This is a decision/spec patch only — it builds none of the scheduler or wrapper it recommends. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** direct read of `mqk-md/src/provider_registry.rs::build_market_data_provider_from_config` found its factory match arms cover exactly `"fake"`/`"twelvedata"`/`"alpaca"` — every other configured provider (`coinlore`, `polygon`, `alphavantage`, `yfinance`) is mechanically unbuildable, not merely unverified. Combined with `providers.json` showing `alpaca` never even declares crypto in `asset_classes` and `twelvedata` remaining self-labeled unverified for crypto, this patch concluded no live network crypto provider can be honestly chosen without a network call this patch (and, by the same hard safety rules, the next patch) is barred from making. Instead, it found the existing `mqk-cli md ingest-csv` command is **already fully generic** (free-text `--source`, any `--path`) and already routes through the exact asset-class-agnostic parse/upsert chain `CRYPTO-DATA-01A`/`01B` proved for `BTC/USD` — so the safest next lane is to operationalize that already-working path into an explicit, default-off, operator-run import script, modeled directly on this repo's own `Register-PremarketDataRefreshTask.ps1`/`Refresh-IntradayMarketData.ps1` scheduler precedent (Task-Scheduler-registers-a-single-scoped-script, `-CheckOnly`/`-Once` modes, paper-DB guard, fail-closed evidence JSON). Recommended next patch: `CRYPTO-DATA-01D-EXPLICIT-LOCAL-CRYPTO-INGEST-RUNNER-01`.

**Notable repo-truth finding:** `mqk_db::md::ingest_csv_to_md_bars` (the function the CLI's `ingest-csv` command actually calls) always stamps `MdBarProviderMetadata::unknown()` rather than calling the existing `ingest_provider_bars_to_md_bars_with_provider_metadata` sibling — every CSV-ingested `md_bars` row's `provider_id` column is literally `"unknown"` regardless of the `--source` flag the operator passes on the command line; that flag only reaches the `md_quality_reports.stats_json.source` field. This is a small, real, previously-unrecorded gap, flagged as optional next-patch polish, not a blocker (the symbol-keyed read path every downstream `ASSET-CORE-04*` consumer uses never reads `provider_id`).

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** no operator-run import script exists yet; no Task Scheduler registration script exists yet; no `ETH/USD` registry-v2 entry or CSV fixture exists yet (confirmed by a repo-wide search matching zero results outside this patch's own docs); `md_bars` rows ingested via CSV still carry `provider_id="unknown"`; no live/network crypto provider is implemented or verified for any candidate (TwelveData, Alpaca, Coinbase, Kraken, Polygon, CoinLore, AlphaVantage, yfinance — all evaluated and deferred with reasons); no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface; no scheduler or recurring ingest job of any kind exists yet for crypto bars.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01C-PROVIDER-INGEST-SCHEDULER-DESIGN-01-COMBINED` entry (end of the `ASSET-CORE-04` section); full design document at `docs/specs/crypto_data_01c_provider_ingest_scheduler_design.md`; machine-readable artifact at `docs/specs/crypto_data_01c_provider_ingest_scheduler_plan.json`.

**Safety confirmation:** no daemon runtime started; no provider/broker scripts or network calls; no API credits spent; no live or paper order submitted; no DB read, write, or migration; no strategy thresholds changed; no broker/OMS/risk/runtime/DB-schema/`mqk-md`-provider-implementation/`mqk-portfolio`/GUI code touched (all reads only); no crypto/futures/options/forex trading enabled; `config/instruments/*`, `config/providers/*`, and `.env.local` all untouched; no smoke logs or generated evidence staged; `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` untouched/unstaged.

## 48. CRYPTO-DATA-01D-EXPLICIT-LOCAL-CRYPTO-INGEST-RUNNER-01-COMBINED Closure Note (maintenance)

CRYPTO-DATA-01D-EXPLICIT-LOCAL-CRYPTO-INGEST-RUNNER-01-COMBINED operationalized the local-CSV ingestion path (`CRYPTO-DATA-01A`/`01B`) decided by `CRYPTO-DATA-01C` (§47) into an explicit, default-off, operator-run import runner. This is an operator-facing PowerShell wrapper patch — no Rust source changes, no network calls, no daemon start, no order/broker/runtime path, no DB migration, no scheduler registration, no crypto trading enablement.

**Concretely:** `scripts/windows/Import-LocalCryptoMarks.ps1` wraps the already-proven `mqk-cli md ingest-csv --path <file> --timeframe <tf> --source <label>` command with explicit `-CheckOnly` (read-only, always default) and `-Once` (mutation, requires paper DB env var) modes. The DB guard (`MQK_DATABASE_URL` must contain `5440` and `miniquantdesk_paper`) fires before any cargo invocation in `-Once` mode. Evidence JSON (`schema_version="local-crypto-import-v1"`) with explicit `all_passed`/`reason_code`/`fail_reasons`/`checks`/`db_guard`/`mutation` fields is written to `exports/market_data/local_crypto_import_<ts>.json` on every run. The validator (`scripts/guards/validate_import_local_crypto_marks.ps1`) proves all 6 checks in subprocess without cargo or DB: parser passes, stale-fixture check-only exits 1, `-AllowStaleForValidation` check-only exits 0, no-DB-URL once exits 1 before mutation, evidence JSON has all required keys, forbidden patterns absent. `git diff --check` clean. `-Once` DB mutation not run — the underlying path was already proven by `CRYPTO-DATA-01B`/`ASSET-CORE-04F`.

**Not resolved (`CRYPTO-DATA-01`/`ASSET-CORE-04`/`CRYPTO-REGISTRY-01` remain `PARTIAL`, not `CLOSED`):** no Windows Task Scheduler registration exists yet (planned as `CRYPTO-DATA-01E`); `md_bars` rows ingested via CSV still carry `provider_id="unknown"` (optional polish — read path never queries it); no `ETH/USD` fixture or registry-v2 entry exists; no live/network crypto provider is implemented or verified; no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface.

**Full detail, exact validation commands, and evidence shape:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01D-EXPLICIT-LOCAL-CRYPTO-INGEST-RUNNER-01-COMBINED` entry (immediately after the `CRYPTO-DATA-01C` section); runbook at `docs/runbooks/local_crypto_marks_ingest.md`; validator at `scripts/guards/validate_import_local_crypto_marks.ps1`.

**Safety confirmation:** no daemon runtime started; no provider/broker scripts or network calls; no API credits spent; no live or paper order submitted; no DB read, write, or migration; no Rust source touched; no strategy thresholds, risk policy, or live routing changed; no crypto/futures/options/forex trading enabled; no scheduler task registered; `config/instruments/*`, `config/providers/*`, `.env.local` all untouched; no smoke logs or generated evidence staged.

## 49. CRYPTO-DATA-01E-LOCAL-CRYPTO-INGEST-TASK-REGISTRATION-01-COMBINED Closure Note (maintenance)

CRYPTO-DATA-01E-LOCAL-CRYPTO-INGEST-TASK-REGISTRATION-01-COMBINED adds an explicit, default-unregistered Windows Scheduled Task registration wrapper for the `CRYPTO-DATA-01D` import runner. This is an operator-facing PowerShell wrapper patch — no Rust source changes, no network calls, no daemon start, no order/broker/runtime path, no DB migration, no crypto trading enablement.

**Concretely:** `scripts/windows/Register-LocalCryptoIngestTask.ps1` (modeled on `Register-PremarketDataRefreshTask.ps1`) supports `-CheckOnly` (display planned config + evidence, no mutation), `-Unregister` (idempotent removal), and default register mode (requires `-CsvPath`). The scheduled task action calls only `Import-LocalCryptoMarks.ps1 -Once` with the configured parameters; it never calls daemon, runtime, broker, provider, or order scripts. `MQK_DATABASE_URL` is not embedded — the import runner resolves it at runtime and fails closed if absent. Evidence JSON (`schema_version="local-crypto-task-registration-v1"`) with `task_exists_before`, `task_exists_after`, `registered`, `check_only`, `task_action`, `runner_path`, `safety.calls_import_runner_only=true` is written to `exports/market_data/local_crypto_ingest_task_registration.json` on every run. The validator (`scripts/guards/validate_register_local_crypto_ingest_task.ps1`) proves all 14 checks without cargo or DB: parser passes, `-CheckOnly` exits 0 and writes evidence, `task_exists_after=false` in evidence, evidence has all required keys, `task_action` contains `Import-LocalCryptoMarks.ps1`/`-Once`/`CsvPath`/`-Timeframe`/`-Source`/`-OutputDir`, `check_only=true`/`registered=false`, 11 forbidden patterns absent, `-Unregister` idempotent on non-existent task, no task remains after validation. `git diff --check` clean. No registration smoke run — parser + check-only + validator proof is sufficient; `Get-ScheduledTask` confirms 0 tasks remain.

**Not resolved (`CRYPTO-DATA-01`/`ASSET-CORE-04`/`CRYPTO-REGISTRY-01` remain `PARTIAL`, not `CLOSED`):** no `ETH/USD` fixture or registry-v2 entry; `md_bars` rows ingested via CSV still carry `provider_id="unknown"` (optional polish); no live/network crypto provider implemented or verified; no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface.

**Full detail, exact validation commands, and evidence shape:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01E-LOCAL-CRYPTO-INGEST-TASK-REGISTRATION-01-COMBINED` entry; runbook at `docs/runbooks/local_crypto_marks_ingest.md` (scheduled task section); validator at `scripts/guards/validate_register_local_crypto_ingest_task.ps1`.

**Safety confirmation:** no daemon runtime started; no import runner `-Once` executed; no provider/broker scripts or network calls; no API credits spent; no live or paper order submitted; no DB read, write, or migration; no Rust source touched; no strategy thresholds, risk policy, or live routing changed; no crypto/futures/options/forex trading enabled; no scheduled task left registered after validation; `config/instruments/*`, `config/providers/*`, `.env.local` all untouched; no smoke logs or generated evidence staged.

## 50. CRYPTO-DATA-01F-CSV-PROVIDER-METADATA-01-COMBINED Closure Note (maintenance)

CRYPTO-DATA-01F-CSV-PROVIDER-METADATA-01-COMBINED closes the provider-metadata/provenance gap `CRYPTO-DATA-01C`/`01D`/`01E` each independently recorded: CSV-ingested `md_bars` rows carried `provider_id="unknown"` regardless of the CLI's `--source` flag. This is provenance/evidence polish only — no network provider, no daemon route, no GUI work, no scheduler change, no registry-v2 cutover, no DB migration, no crypto trading enablement.

**Concretely:** `mqk_db::md::ingest_csv_to_md_bars` (`crates/mqk-db/src/md.rs`) previously called the metadata-less `ingest_provider_bars_to_md_bars`, which always stamps `MdBarProviderMetadata::unknown()` — the exact root cause, confirmed by direct read. It now builds a `MdBarProviderMetadata` from the operator's `--source` label (`provider_id`/`provider_source` = the source label, or `"unknown"` only if blank; `ingest_mode="csv_import"`) and calls the existing, already-proven `ingest_provider_bars_to_md_bars_with_provider_metadata` sibling instead — the same helper `mqk-daemon`'s live market-data-feed route and `scenario_md_ingest_provider.rs` already exercise. No new `md_bars` columns were needed; the provider-metadata columns already existed in the schema. `provider_symbol` was deliberately left unset rather than fabricated from one CSV row's symbol, since the existing upsert SQL applies that field uniformly across an entire ingest batch and a CSV file is not guaranteed to be single-symbol. `md_quality_reports.stats_json.source` (the operator's verbatim `--source` string) is completely unchanged. No CLI, PowerShell runner, or scheduler file required any change — `--source` already flowed through to `IngestCsvArgs.source`; only the internal `mqk-db` wiring needed to connect it to provider metadata.

**Not resolved (`CRYPTO-DATA-01`/`ASSET-CORE-04`/`CRYPTO-REGISTRY-01` remain `PARTIAL`, not `CLOSED`):** no `ETH/USD` fixture or registry-v2 entry; no live/network crypto provider implemented or verified; no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface; no new scheduler work beyond the already-existing `CRYPTO-DATA-01E` task wrapper.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01F-CSV-PROVIDER-METADATA-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); runbook update at `docs/runbooks/local_crypto_marks_ingest.md` (new "Provider Metadata" section replacing the old "provider_id stays unknown" gap note); tests at `core-rs/crates/mqk-db/tests/scenario_md_ingest_csv.rs`.

**Safety confirmation:** no daemon runtime started; no provider/broker scripts or network calls; no API credits spent; no live or paper order submitted; no DB migration (existing `md_bars` provider-metadata columns only); no strategy thresholds, risk policy, or live routing changed; no crypto/futures/options/forex trading enabled; only `core-rs/crates/mqk-db/src/md.rs` and `core-rs/crates/mqk-db/tests/scenario_md_ingest_csv.rs` changed in `core-rs/`; the only DB mutation anywhere was this patch's own isolated, cleaned-before-and-after test rows against the local paper DB; no smoke logs or generated evidence staged.

## 51. CRYPTO-DATA-01G-ETHUSD-LOCAL-CSV-MARKS-01-COMBINED Closure Note (maintenance)

CRYPTO-DATA-01G-ETHUSD-LOCAL-CSV-MARKS-01-COMBINED closes the last previously-recorded gap in the local crypto lane: `CRYPTO-DATA-01C`/`01D`/`01E`/`01F` each independently noted "no `ETH/USD` fixture or registry-v2 entry exists yet." This patch adds an additive, disabled `ETH/USD` registry-v2 row and a committed local CSV fixture beside the existing `BTC/USD` ones, proving the entire local-mark lane (fixture load/validate, CSV parse, `ASSET-CORE-04B`/`04A`/`04C` model chain, `CRYPTO-DATA-01F` provider-metadata stamping) is generic over symbol, not hardcoded to `BTC/USD`. Test-file/fixture/docs only — no network provider, no daemon route, no GUI work, no scheduler change, no registry-v2 production cutover, no DB migration, no crypto trading enablement.

**Concretely:** `config/instruments/instruments_v2.crypto_local_marks.example.json` gains a second row, `ETH/USD` (`instrument_id="crypto:GLOBAL:ETHUSD"`, `CryptoPair{base:"ETH",quote:"USD"}`, `currency`/`quote_currency="USD"`), disabled identically to `BTC/USD` (`enabled=false`, `paper_trading_enabled=false`, `live_trading_enabled=false`, `allow_enabled_non_equity_for_testing` absent). `core-rs/crates/mqk-md/tests/fixtures/crypto_ethusd_1d_local.csv` adds 3 deterministic `ETH/USD` `1D` bars with the same schema as the `BTC/USD` fixture; latest completed close is `$3,200.00`. `scenario_crypto_local_marks_registry_data_01a.rs` (mqk-md) is extended with `REG-11`..`REG-16`/`CSV-08`..`CSV-14` proving both rows/fixtures load, validate, and parse correctly, with all existing `BTC/USD` assertions preserved unchanged (only the "exactly one instrument" test necessarily became "exactly two, BTC/USD at index 0, ETH/USD at index 1," since both rows now share one fixture file). `scenario_crypto_portfolio_economics_local_marks_01a.rs` (mqk-daemon)'s `chain01` bridge-count assertions were updated from 1 to 2 instruments for the same reason; every other BTC/USD assertion in that file is untouched. Two new files complete the ETH/USD-specific proof: `scenario_crypto_ethusd_portfolio_economics_local_marks_01g.rs` (pure in-memory `ASSET-CORE-04B`/`04A`/`04C` chain proof, 1 ETH position -> `$3,200.00` notional, `Active` snapshot, `"crypto"`/`"USD"` exposure buckets, zero enablement anywhere, and a `chain08` regression confirming `BTC/USD` is unaffected by ETH/USD's presence in the same fixture) and `scenario_crypto_ethusd_local_mark_db_persistence_01g.rs` (DB-backed proof against the real local paper Postgres, port 5440). The DB-backed test calls `mqk_db::ingest_csv_to_md_bars` directly against the committed CSV file — the same production function `mqk-cli md ingest-csv`/`Import-LocalCryptoMarks.ps1` call — with `--source`-equivalent `"local_crypto_manual_ethusd_01g"`, proving `CRYPTO-DATA-01F`'s provider-metadata stamping (`provider_id`/`provider_source="local_crypto_manual_ethusd_01g"`, `ingest_mode="csv_import"`) applies unconditionally, not specifically to `BTC/USD`. It then reads the row back via the existing `mqk_db::fetch_recent_completed_bars_for_strategy`, feeds the DB-read `$3,200.00` close through the unmodified `ASSET-CORE-04B`/`04A`/`04C` chain to the same `Active`/`crypto`/`USD` result, confirms zero `oms_outbox` writes, and confirms zero leftover `ETH/USD` rows in `md_bars` after cleanup. `docs/runbooks/local_crypto_marks_ingest.md` gained an "ETH/USD Fixture (CRYPTO-DATA-01G)" section and had the now-closed "No `ETH/USD` fixture..." gap line removed from "Remaining Gaps."

**Validation (run at HEAD `8b85d897`):** `cargo check -p mqk-md -p mqk-daemon -p mqk-portfolio` clean. `cargo test -p mqk-md --test scenario_crypto_local_marks_registry_data_01a` — 31/31 pass (10 original BTC/USD `reg`/`csv`/`net` + 6 new ETH/USD `reg` + 7 new ETH/USD `csv`, `reg03` restructured to 2-instrument assertion). `cargo test -p mqk-daemon --test scenario_crypto_portfolio_economics_local_marks_01a` — 5/5 pass (BTC/USD chain proof, `chain01` counts updated to 2/2/0). `cargo test -p mqk-daemon --test scenario_crypto_ethusd_portfolio_economics_local_marks_01g` — 6/6 pass. `cargo test -p mqk-daemon --test scenario_crypto_ethusd_local_mark_db_persistence_01g` (against the real local paper Postgres at `127.0.0.1:5440/miniquantdesk_paper`) — 1/1 pass. Regressions: `scenario_crypto_local_mark_db_persistence_01b` (BTC/USD DB proof) — 2/2 pass, unchanged; `scenario_portfolio_economics_v2_registry_seam_asset_core_04f` (route-level BTC/USD valuation) — 10/10 pass, unchanged; `mqk-portfolio`'s `scenario_portfolio_instrument_economics_asset_core_04a` — 31/31 pass; `scenario_portfolio_economics_aggregation_asset_core_04c` — 21/21 pass. `validate_import_local_crypto_marks.ps1`/`validate_register_local_crypto_ingest_task.ps1` (untouched `CRYPTO-DATA-01D`/`01E` scripts) — both `ALL CHECKS PASSED`, proving neither script needed or received changes. `cargo clippy -p mqk-md -p mqk-daemon -p mqk-portfolio --all-targets -- -D warnings` hit pre-existing, unrelated `await_holding_lock`/`bool_assert_comparison` drift confined entirely to `mqk-daemon/src/state/runtime_session_source.rs`, `mqk-daemon/src/state/session_controller.rs`, and two `scenario_runtime_session_v2_*` test files (all last touched 2026-06-28, before this patch's lineage began, confirmed via `git log`) — no patch-owned file appears anywhere in the clippy output. `cargo fmt -p ... -- --check`-equivalent (`rustfmt --check` on the four patch-owned Rust files) shows diffs that are confirmed pre-existing at `HEAD` (verified via `git show HEAD:...` on the pre-patch file) for the untouched surrounding lines this patch's new code deliberately mirrors stylistically; no new drift was introduced. `git diff --check` clean.

**Not resolved (`CRYPTO-DATA-01`/`ASSET-CORE-04`/`CRYPTO-REGISTRY-01` remain `PARTIAL`, not `CLOSED`):** no live/network crypto provider implemented or verified (still zero); no production registry-v2 cutover (registry-v2 still has zero production route callers for default/legacy config); no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface; no new scheduler work beyond the already-existing `CRYPTO-DATA-01E` task wrapper (which still points at whatever CSV path the operator configures — this patch did not change the wrapper or point it at `ETH/USD`). `ASSET-CORE-04D`'s route-level crypto-valuation structural blocker for a genuinely fresh non-`BTC/USD` symbol was already closed generically by `ASSET-CORE-04F`; this patch did not add a new `ETH/USD`-specific route-level test (out of file scope) but nothing about `ASSET-CORE-04F`'s generic `?registry_source=v2` seam is symbol-specific, so no new structural gap was introduced.

**Full detail, exact test names, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01G-ETHUSD-LOCAL-CSV-MARKS-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no daemon runtime started; no import runner `-Once` executed by this patch itself (the DB-backed test calls `mqk_db::ingest_csv_to_md_bars` directly, in-process, not via the PowerShell wrapper); no scheduled task registered or modified; no provider/broker scripts or network calls; no API credits spent; no live or paper order submitted; no DB migration; no strategy thresholds, risk policy, or live routing changed; no crypto/futures/options/forex trading enabled; only the files in this patch's stated scope changed; the only DB mutation anywhere was this patch's own isolated, cleaned-before-and-after `ETH/USD` test rows against the local paper DB (confirmed zero remaining afterward); no smoke logs or generated evidence staged.

---

## 52. CRYPTO-DATA-01H-LIVE-CRYPTO-PROVIDER-DECISION-01-COMBINED Closure Note (maintenance)

CRYPTO-DATA-01H-LIVE-CRYPTO-PROVIDER-DECISION-01-COMBINED continued the crypto data lane after `CRYPTO-DATA-01G` (§51) by deciding the first live/network crypto market-data provider verification lane and first implementation lane for `BTC/USD`/`ETH/USD`. This is a decision/spec patch only — it builds none of the verification call or provider adapter it names. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** direct read of `mqk-md/src/provider_registry.rs::build_market_data_provider_from_config` at HEAD `6271c048` reconfirmed the factory's three match arms (`"fake"`/`"twelvedata"`/`"alpaca"`) are unchanged since `CRYPTO-DATA-01C` (`6535db8e`) — no crypto-capable provider factory exists in code today, still mechanical proof, not inference. Direct read of `alpaca_provider.rs::bars_url()` confirmed it remains hardcoded to `/v2/stocks/bars` with zero crypto-endpoint code, and `providers.json`'s `alpaca` entry still declares only `["equity","etf"]`. `providers.json`'s `twelvedata` entry remains `implementation_status: "implemented_equity_provider"` / crypto-unverified despite its Rust client (`TwelveDataHistoricalProvider`) being mechanically symbol-agnostic. This patch's incremental contribution beyond `01C`'s own analysis: it ranks the field. CoinLore (crypto-exclusive `asset_classes`, no API key required) is chosen as the first network-authorized verification candidate over TwelveData specifically because CoinLore's verification would touch zero shared credentials or rate-limit budget with the equity-provisioned `TWELVEDATA_API_KEY`, isolating the risk of a future verification mistake. Alpaca and Coinbase/Kraken are classified `rejected_for_first_lane` (crypto not declared / zero repo presence, respectively) rather than merely `deferred`, distinguishing "structurally unfit as a first lane" from "unimplemented but plausible later." Recommended next patches: `CRYPTO-DATA-01I-COINLORE-READONLY-NETWORK-VERIFY-01` (one bounded, operator-authorized, read-only network call confirming CoinLore's actual response shape before any provider code is written) and, only after that succeeds, `CRYPTO-DATA-01J-COINLORE-PROVIDER-ADAPTER-LOCAL-INGEST-01` (a fourth `provider_registry.rs` factory arm plus a `CoinLoreHistoricalProvider`, default-disabled).

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** no live/network crypto provider is implemented or verified for any candidate (still zero, unchanged from every prior patch in this lineage); `01I`'s single verification call was not made by this patch (it is barred from making it); `01J`'s adapter/factory-arm/tests do not exist; `ingest-provider`/`sync-provider` remain hard-locked to `"twelvedata"|"alpaca"`; no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface; no new scheduler work.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01H-LIVE-CRYPTO-PROVIDER-DECISION-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); full decision document at `docs/specs/crypto_data_01h_live_provider_decision.md`; machine-readable artifact at `docs/specs/crypto_data_01h_live_provider_decision.json`; validator at `scripts/guards/validate_crypto_data_01h_provider_decision.ps1`; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no daemon runtime started; no provider/broker scripts or network calls made; no API credits spent; no live or paper order submitted; no DB connection made or mutation performed; no DB migration; no strategy thresholds, risk policy, or live routing changed; no crypto/futures/options/forex trading enabled; no CLI, GUI, scheduler, or provider-implementation file touched; only the files in this patch's stated scope changed; no smoke logs or generated evidence staged.

---

## 53. CRYPTO-DATA-01I-COINLORE-READONLY-NETWORK-VERIFY-01 Closure Note (maintenance)

CRYPTO-DATA-01I-COINLORE-READONLY-NETWORK-VERIFY-01 continued the crypto data lane after `CRYPTO-DATA-01H` (§52) by performing the first explicitly-authorized, bounded, read-only network verification of CoinLore. This is a verification/evidence patch only — it builds no provider code, no factory arm, no CLI change, no DB write. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** 2 of the authorized 3 bounded, keyless HTTP GET requests were made to `api.coinlore.net` at HEAD `4eaa14c2`: `GET /api/tickers/` (top-100 discovery/shape call, HTTP 200, 37,100 bytes) and `GET /api/ticker/?id=90,80` (targeted per-ID lookup for BTC id=90 and ETH id=80 together, HTTP 200, 725 bytes). Both endpoints reliably identified BTC/ETH and returned USD `price_usd` for both, but **neither exposes OHLCV history or a per-ticker timestamp** — only a rolling `volume24` and a list-level (not per-ticker) `info.time`. This resolves the open question `01H` §15 explicitly flagged ("Does CoinLore's public API actually expose historical OHLCV bars, or only a current spot ticker?") with real evidence: spot ticker only, for the two endpoints called. Populating the existing `RawBar`/`ProviderBar` model as a completed bar from this data would require fabricating `open`/`high`/`low` (copying `close`) and `end_ts`/`is_complete` (asserting client request time as a provider-confirmed bar close) — forbidden by this patch's authorization and `CLAUDE.md`'s no-fabricated-truth invariant. Decision recorded: `PARTIAL_TICKER_ONLY` — CoinLore is not rejected as unfit, but `01J`'s scope must adapt to a ticker/latest-mark model (or make its own further-authorized call to an undiscovered endpoint) rather than the bar-shaped adapter `01H` originally assumed.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** no live/network crypto provider is implemented (still zero); `01J`'s adapter/factory-arm/tests do not exist and must be re-scoped per the evidence doc's §11; CoinLore's official rate limit remains unknown (no header observed, docs page not fetched by this patch); `ingest-provider`/`sync-provider` remain hard-locked to `"twelvedata"|"alpaca"`; no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface; no new scheduler work.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01I-COINLORE-READONLY-NETWORK-VERIFY-01` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); full evidence document at `docs/specs/crypto_data_01i_coinlore_network_verify.md`; machine-readable artifact at `docs/specs/crypto_data_01i_coinlore_network_verify.json`; validator at `scripts/guards/validate_crypto_data_01i_coinlore_verify.ps1`; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no daemon runtime started; no import runner `-Once` executed; no scheduled task registered; no provider implementation added; exactly 2 bounded, non-looped, non-retried, read-only GET requests to CoinLore's public keyless endpoints — no other broker/provider network call made; no API credits spent (CoinLore requires no key); no credentials used; `.env.local` not read; no live or paper order submitted; no DB mutation or migration; no strategy thresholds, risk policy, or live routing changed; no crypto/futures/options/forex trading enabled; no CLI, GUI, scheduler, or provider-implementation file touched; only the files in this patch's stated scope changed; no smoke logs or generated evidence staged (raw response bodies/headers were captured only to the session scratchpad outside the repo, never committed).

---

## 54. CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED Closure Note (maintenance)

CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED continued the crypto data lane after `CRYPTO-DATA-01I` (§53) by adapting to its `PARTIAL_TICKER_ONLY` finding: it built a distinct latest-mark model, a CoinLore ticker parser/client, registry-v2 aliases, and a read-only CLI surface, without making any additional network call. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** at HEAD `82d02a05`, `core-rs/crates/mqk-md/src/latest_mark.rs` adds `LatestMark` with no `open`/`high`/`low`/`is_complete`/`end_ts` field and no conversion into `RawBar`/`ProviderBar` — a test asserts the serialized JSON shape contains none of those bar-like field names, proving at the wire level (not just the type system) that this model cannot be mistaken for a completed-bar payload. `core-rs/crates/mqk-md/src/providers/coinlore.rs` parses the exact `/api/ticker/?id=90,80` shape `01I` verified, rejecting (not silently dropping or fabricating) an empty array, malformed JSON, a missing/empty `id`/`symbol`/`price_usd`, a non-decimal `price_usd`, a duplicate `id`, a response missing a configured asset, and a mismatched `id`/`symbol` pair. `build_market_data_provider_from_config` (the bar-oriented factory) was **not** given a `"coinlore"` arm — it was already, and remains, unmodified, so `"coinlore"` still falls through to the existing `ProviderFactoryError::UnsupportedProvider` refusal, satisfying `01I` §11.2 without any new code needed there. `config/instruments/instruments_v2.crypto_local_marks.example.json` gained `provider_symbols.coinlore_id`/`coinlore_symbol` on both existing disabled rows (`90`/`BTC`, `80`/`ETH` — exactly `01I`'s verified IDs), with `enabled`/`paper_trading_enabled`/`live_trading_enabled` untouched (`false`). A new `mqk md coinlore-latest-mark` CLI command defaults to parsing a local `--input-file` (zero network calls) and only attempts one live GET when the operator sets `MQK_ALLOW_COINLORE_NETWORK_SMOKE=1`; it never opens a DB connection and never writes `md_bars`. 21 new fixture-based tests (7 model, 14 parser, 7 registry-integration, 6 CLI) all pass; the pre-existing `01A` registry/CSV test (31 tests) and four `mqk-daemon` crypto-economics regression tests (19 tests) are unaffected.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** no live network crypto provider exists for completed-bar/OHLCV ingestion (still zero); no dedicated `latest_marks` storage/route exists — a `LatestMark` produced by the new CLI is an operator evidence artifact only, not persisted anywhere queryable, and the next storage decision (dedicated table/route vs. continued no-DB evidence-only usage vs. further provider verification for real OHLCV) remains open; `ingest-provider`/`sync-provider` remain hard-locked to `"twelvedata"|"alpaca"`; no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy; no GUI/operator crypto surface.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); full bundle document at `docs/specs/crypto_data_01j_klm_coinlore_latest_mark_provider_bundle.md`; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no daemon runtime started; no autonomous runtime run; no market-hours proof run; no import runner or scheduled task touched; no live network call made during this patch's own validation (the one network-capable function added, `fetch_coinlore_ticker_body`, was not invoked — only fixture-based tests ran); no API credits spent; no credentials used; `.env.local` not read; no live or paper order submitted; no DB connection opened by any new code, no DB mutation, no DB migration; `providers.json`'s `coinlore` entry remains `enabled: false`; no strategy thresholds, risk policy, or live routing changed; no crypto/futures/options/forex trading enabled; no `mqk-daemon`/`mqk-runtime`/`mqk-execution`/`mqk-broker-*`/`mqk-risk`/`mqk-portfolio/src`/`mqk-db/src`/`mqk-db/migrations`/`mqk-gui` file touched; no `equities.json` change; only the files in this bundle's stated scope changed; no smoke logs or generated evidence staged.


---

## 55. CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED Closure Note (maintenance)

CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED continued the crypto data lane after `CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED` (§54) by deciding the first operator-visible surface for `LatestMark` output: an evidence-file-only status route, with no DB table and no `md_bars` reuse. This is operator-visibility and evidence-contract work only — no completed-bar ingestion, no DB mutation, no DB migration. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** at HEAD `1163f45c`, `docs/specs/crypto_data_01n_op_latest_mark_evidence_status_bundle.md` records the storage decision: evidence-file-only was chosen over a dedicated `latest_marks` table (no real consumer yet needs queryable history) and over reusing `md_bars` with a non-bar flag (rejected — `md_bars`'s schema requires real `open`/`high`/`low`/`close`/`is_complete`/`end_ts`, and adding a "this isn't really a bar" flag to a bar table is the same fabrication risk the `01I`/`01J` lineage was built to avoid). `mqk-cli/src/commands/md.rs::md_coinlore_latest_mark`'s evidence JSON contract (previously `01M`'s minimal shape: `schema_version`/`provider_id`/`network_call_made`/`db_write`/`md_bars_write`/`completed_bar_claim`/`requested_symbols`/`marks`) was extended with `producer`, `produced_at_utc`, `provider`, `mode` (`"input_file"` | `"network_smoke"`), `provider_enabled` (read-only visibility from a new `--provider-registry` flag; never gates parsing or the network call), `registry_path`, `symbols_requested`, `truth_state`, `stale_or_missing`, `all_passed`, `reason_code`, `fail_reasons` — all 8 pre-existing `01M` tests still pass unmodified, plus 2 new tests proving the full contract. A new `GET /api/v1/market-data/latest-marks/status` route (`mqk-daemon/src/routes/transport_quality.rs::latest_mark_status`) reads the latest matching evidence file from the same directory `intraday-refresh/status` already reads (filtered to a distinct filename prefix, so the two evidence streams never collide) and surfaces `active`/`stale`/`no_evidence`/`parse_error`/`unsafe_evidence`/`backend_unavailable`. The `unsafe_evidence` state is a fail-closed check the route performs independently of what the evidence claims about itself: if an evidence file (however produced) claims `db_write`/`md_bars_write`/`completed_bar_claim=true`, or a mark carries any bar-like field (`open`/`high`/`low`/`close`/`is_complete`/`end_ts`), the route refuses to surface it as `active` — proven by 5 of the route's 12 new tests. The route opens no DB connection, makes no provider/network call, and starts no CLI/daemon runtime.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** no live network crypto provider exists for completed-bar/OHLCV ingestion (still zero); no `latest_marks` DB table exists — deliberate per this bundle's own decision, not an oversight, and may be built later as a separately-authorized patch with its own migration/idempotency/restart-safety proof if a real consumer needs queryable history rather than "latest snapshot only"; no GUI/operator crypto surface; `ingest-provider`/`sync-provider` remain hard-locked to `"twelvedata"|"alpaca"`; no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); full decision document at `docs/specs/crypto_data_01n_op_latest_mark_evidence_status_bundle.md`; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no daemon runtime started (route tests use `axum::Router::oneshot` against a synthetic in-memory router, not a live daemon process); no autonomous runtime run; no market-hours proof run; no import runner or scheduled task touched; no CoinLore or any other network call made anywhere in this bundle or its validation; no API credits spent; no credentials used; `.env.local` not read; no live or paper order submitted; no DB connection opened by the new route or the extended CLI path, no DB mutation, no DB migration; `providers.json`'s `coinlore` entry remains `enabled: false` and was not modified; no strategy thresholds, risk policy, or live routing changed; no crypto/futures/options/forex trading enabled; no `mqk-runtime`/`mqk-execution`/`mqk-broker-*`/`mqk-risk`/`mqk-portfolio/src`/`mqk-db/src`/`mqk-db/migrations`/`mqk-gui` file touched; no `config/instruments/*` or `config/providers/providers.json` change; only the files in this bundle's stated scope changed; no smoke logs or generated evidence staged.

## 56. CRYPTO-DATA-01Q-R-LATEST-MARK-GUI-SURFACE-BUNDLE-01-COMBINED Closure Note (maintenance)

CRYPTO-DATA-01Q-R-LATEST-MARK-GUI-SURFACE-BUNDLE-01-COMBINED built the GUI operator surface `§55` (`CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED`) recommended as its "next slice": a typed API client and a read-only "Crypto latest marks" panel on the operator Ingest screen, consuming `GET /api/v1/market-data/latest-marks/status`. This is GUI/operator-visibility work only — no backend Rust, daemon route, API contract, CLI, DB, or trading-path code was changed. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** at HEAD `7f83d72f`, `core-rs/mqk-gui/src/features/ingest/types.ts` gained `LatestMarkStatusMark`/`LatestMarkStatusResponse`, mirroring `mqk-daemon/src/api_types.rs`'s `LatestMarkStatusRow`/`LatestMarkStatusResponse` field-for-field. `api.ts` gained `fetchLatestMarkStatus` (read-only GET, no operator token required, no CLI/provider call reachable from the GUI), `isLatestMarkStatusActive`, `latestMarkStatusTruthLabel` (renders `unsafe_evidence` as an explicit severe fail-closed label, not a bare passthrough of the raw string), and `isLatestMarkEvidenceUnsafe` — a GUI-side defense-in-depth check that treats a response as unsafe if it claims `db_write`/`md_bars_write`/`completed_bar_claim=true`, independent of whatever `truth_state` the backend itself reports, so a hypothetically-misclassified backend response is still never rendered as trustworthy on the GUI side. `IngestScreen.tsx` gained a "Crypto latest marks" panel directly below the existing "Intraday refresh status" panel, following that panel's exact structure (auto-load on mount, a local "Refresh" button that only re-GETs the same read-only route, no provider/network/CLI-triggering action anywhere in the panel). All 6 `truth_state` values render distinctly; `unsafe_evidence` (or the GUI-side defense-in-depth check firing independently) shows a dedicated critical banner and suppresses the marks table even if a response also carried mark data. The panel carries a fixed, non-conditional caption: "Ticker-only latest marks. Not OHLCV, not md_bars, not portfolio valuation, and not trading enablement."

**Repo-truth finding surfaced during this bundle:** `core-rs/mqk-gui` has zero `.tsx` component-render test files anywhere in the codebase, and no jsdom or `@testing-library/react` dependency in `package.json` — every existing GUI test across `system/`, `backtests/`, and `ingest/` is a pure-function/shape-level test executed via `tsx --test`, not a DOM-rendering assertion. This bundle's 29 new tests (in `ingest/__tests__/api.test.ts`) follow that same established convention — truth-label/active-check/unsafe-check unit tests, plus response-shape tests built from the real `BTC/USD 62906.61`/`ETH/USD 1777.74` fixture in `scenario_latest_mark_status_route_01nop.rs` — rather than introducing new test infrastructure, which would have required a `package.json` change outside this bundle's stated file scope.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** no live network crypto provider exists for completed-bar/OHLCV ingestion (still zero); no `latest_marks` DB table exists (unchanged by this GUI-only bundle); no production registry-v2 cutover; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy. This bundle also did not complete a live in-browser render proof against a running daemon: the operator GUI shell hard-blocks every screen behind a live daemon connectivity poll, and this bundle's hard constraints forbade starting the daemon; verification instead rests on `tsc` typecheck + Vite production build passing against the real daemon response types, plus the 29 new tests exercising every rendering branch with realistic fixture data.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01Q-R-LATEST-MARK-GUI-SURFACE-BUNDLE-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no backend Rust, daemon route, API contract, CLI, or DB code changed; no DB migration; no daemon runtime started; no CoinLore or any other network/provider call made or made reachable from any new GUI control; no API credits spent; no credentials used; `.env.local` not read; no live or paper order submitted; no DB connection opened, no DB mutation; `providers.json`'s `coinlore` entry untouched; no strategy thresholds, risk policy, or live routing changed; no crypto/futures/options/forex trading enabled; no `mqk-runtime`/`mqk-execution`/`mqk-broker-*`/`mqk-risk`/`mqk-portfolio`/`mqk-db`/`core-rs/mqk-gui/src-tauri` file touched; no config/script/DB-migration file touched; only the files in this bundle's stated scope changed; no generated screenshot/log/evidence file staged.

## 57. CRYPTO-DATA-01S-T-OHLCV-PROVIDER-DECISION-VERIFY-BUNDLE-01-COMBINED Closure Note (maintenance)

CRYPTO-DATA-01S-T-OHLCV-PROVIDER-DECISION-VERIFY-BUNDLE-01-COMBINED continued the crypto data lane after CoinLore's network-proven ticker-only finding (`§53`, `01I`) by comparing completed-bar/OHLCV provider candidates and bounded-verifying the selected one. This is decision + bounded read-only network verification only — no provider adapter, no factory arm, no DB write, no `md_bars` write, no crypto trading enablement. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** at HEAD `064ca584`, direct re-read of `core-rs/crates/mqk-md/src/provider_registry.rs` (lines 226-283) reconfirmed the market-data provider factory still has exactly three match arms (`"fake"`, `"twelvedata"`, `"alpaca"`) — every other provider id, including a future `"kraken"`, falls through to `ProviderFactoryError::UnsupportedProvider`. `config/providers/providers.json`'s `coinlore` entry was reconfirmed unchanged (`ticker_only_network_verified_01i_no_ohlcv`). The bundle compared Kraken public OHLC, Coinbase Exchange product candles, Binance/Binance.US klines, TwelveData crypto (credentialed, excluded from this unauthenticated verification), Alpaca crypto (structurally unfit — crypto not declared, hardcoded to `/v2/stocks/bars`), and CoinLore (ineligible, ticker-only) against current repo evidence, then made 3 of the 6 authorized bounded, keyless, read-only GET requests: 1 fetch of Kraken's official OHLC endpoint documentation (confirming `security: []`, the 8-field response shape, and the documented "current, not-yet-committed timeframe" caveat for the last array entry) and 2 live data calls — `GET https://api.kraken.com/0/public/OHLC?pair=XBTUSD&interval=1440` (200, 62 277 bytes) and `GET https://api.kraken.com/0/public/OHLC?pair=ETHUSD&interval=1440` (200, 61 855 bytes) — both resolving unambiguously to Kraken's internal pair keys (`XXBTZUSD`, `XETHZUSD`) with 721 real daily rows each. **Selected: Kraken public OHLC** as the next completed-bar/OHLCV adapter-lane candidate.

**Key verification finding (derived from the real response, not asserted from docs alone):** in both the BTC and ETH responses, `result.last` is byte-identical to the second-to-last array row's `time`, and the final row sits exactly one interval (86 400 seconds) later — a provider-supplied, mechanical signal that lets a future adapter derive `is_complete = row.time <= result.last` honestly, instead of guessing or fabricating completion the way CoinLore's ticker-only shape would have required. A second finding: Kraken's `time` field is the bar's **start**, not its end, so `RawBar.end_ts` must be computed as `row.time + interval_seconds`. Every OHLC value quoted in the decision doc is copied verbatim from the real response bodies — no field was fabricated.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** no Kraken adapter exists — this bundle is a decision/verification artifact only; Kraken's numeric rate limit is unknown (not fetched, to keep the request count minimal); `RawBar.volume`'s `i64` type vs. Kraken's fractional base-currency volume string is unresolved; `ingest-provider`/`sync-provider` remain hard-locked to `"twelvedata"|"alpaca"`; Coinbase Exchange and Binance/Binance.US remain unverified candidates, not ruled out (Binance carries an honestly-flagged, unresolved geo-restriction risk); no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01S-T-OHLCV-PROVIDER-DECISION-VERIFY-BUNDLE-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); full decision/verification document at `docs/specs/crypto_data_01s_t_ohlcv_provider_decision_verify.md`; machine-readable artifact at `docs/specs/crypto_data_01s_t_ohlcv_provider_decision_verify.json`; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no provider adapter, factory arm, or CLI ingestion path added; no Rust, GUI, or config file changed; no DB connection opened, no DB mutation, no DB migration; no `md_bars` write; no `latest_marks` write; exactly 3 of the 6 authorized bounded, non-looped, non-retried, read-only GET requests were made, all unauthenticated, no credentials or API keys used or spent, `.env.local` not read; no raw response body staged (raw bodies written only to the session scratchpad directory outside the repo); `providers.json`'s `coinlore` entry untouched; no strategy/risk/broker/runtime/OMS/portfolio file touched; no live or paper order submitted; no crypto/futures/options/forex trading enabled; no daemon runtime started; only the six files in this bundle's stated scope changed.

## 58. CRYPTO-DATA-01U-V-W-KRAKEN-OHLCV-ADAPTER-PARSER-CLI-BUNDLE-01-COMBINED Closure Note (maintenance)

Continuing after `§57` (`01S-T`, Kraken selected + bounded live-verified), this bundle built the first disabled-by-default, fixture-first Kraken OHLCV adapter lane for `BTC/USD`/`ETH/USD`, closing three adjacent slices together: `CRYPTO-DATA-01U-KRAKEN-OHLCV-PROVIDER-ADAPTER-LOCAL-INGEST-01`, `CRYPTO-DATA-01V-KRAKEN-OHLCV-VOLUME-SEMANTICS-01`, `CRYPTO-DATA-01W-KRAKEN-OHLCV-CLI-EVIDENCE-01`. This is a parser/adapter/CLI-evidence patch only — no recurring ingestion, no scheduling, no GUI, no DB write, no DB migration, no crypto trading enablement. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** added `mqk_md::providers::kraken` (parser, `KrakenAlias` registry-v2 extraction, `KrakenHistoricalProvider` implementing `HistoricalProvider`), a fourth `provider_registry.rs` factory match arm (`"kraken"`, reached only when `providers.json`'s `kraken` entry is `enabled: true` — the committed registry keeps it `false`), disabled `kraken_pair`/`kraken_result_key` registry-v2 aliases on the existing `BTC/USD`/`ETH/USD` rows, and a read-only `mqk md kraken-ohlc-dry-run` CLI command mirroring `mqk md coinlore-latest-mark`'s fixture-first design (`MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`-gated network opt-in, never exercised in default validation). Two committed fixtures (`kraken_ohlc_xbtusd_1d.json`, `kraken_ohlc_ethusd_1d.json`) carry the exact live-verified committed/forming row pair from `01S-T`'s decision doc plus one synthetic filler row for multi-row proof.

**Volume semantics decided (`01V`):** Kraken's fractional base-asset `volume` string is scaled by `1e8` into `ProviderBar.volume: i64` (e.g. `"1317.15941434"` -> `131715941434`, `"15588.01759742"` -> `1558801759742`, both exact matches to `01S-T`'s live-verified samples) — documented as `kraken_base_asset_volume_scaled_by_1e8_not_whole_coins_not_usd` in code and CLI evidence. More than 8 fractional digits, negative values, and `i64` overflow during scaling are rejected (typed errors), never truncated or wrapped.

**Completion semantics implemented (not just decided):** `is_complete = row.time <= result.last` and `end_ts = row.time + interval_seconds`, both derived structurally from the parsed response, never from a fabricated flag or wall-clock guess. The forming row is excluded from every `completed_provider_bars()` call and cannot be converted directly (`kraken_bar_to_provider_bar` refuses forming rows as a defense-in-depth guard).

**Bug found and fixed in the same patch (in scope, `provider.rs` was already listed in the allowed file set):** `HistoricalProviderMarketDataAdapter::new` unconditionally seeded `supported_asset_classes` with `ProviderAssetClass::Equity` regardless of the wrapped provider — harmless for the existing equity-only Alpaca/TwelveData usages, but would have made a Kraken-wrapped adapter falsely advertise `Equity` instead of `Crypto`. Added `with_capabilities` (mirroring `FakeMarketDataProvider`'s existing builder pattern) and used it only in the new `"kraken"` factory arm; Alpaca/TwelveData arms are untouched.

**DB-backed ingest proof deliberately deferred:** no ingest command wires `"kraken"` into `ingest-provider`/`sync-provider` in this patch (both remain hard-locked to `"twelvedata"|"alpaca"`), so a DB-backed proof would only exercise the already-proven `ingest_provider_bars_to_md_bars_with_provider_metadata` reuse path without proving anything new; deferred to the future patch that first wires Kraken into a DB-writing ingest path.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** the `kraken` provider stays `enabled: false`; no recurring/scheduled Kraken ingestion; no daemon ingest job; no GUI status surface for Kraken OHLCV; no DB-backed Kraken ingest proof; Kraken's numeric public rate limit remains unestablished (unchanged from `01S-T`); Coinbase Exchange and Binance/Binance.US remain unverified candidates (unchanged); no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01U-V-W-KRAKEN-OHLCV-ADAPTER-PARSER-CLI-BUNDLE-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); full spec document at `docs/specs/crypto_data_01u_v_w_kraken_ohlcv_adapter_parser_cli.md`; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no live Kraken network call made in default validation (fixture files + `httpmock` only); the `kraken` provider config stays `enabled: false`; no DB connection opened, no DB mutation, no DB migration; no `md_bars` write; no scheduler, no daemon ingest job, no GUI change; no broker/risk/execution/OMS/runtime/strategy file touched; no live or paper order submitted; no crypto/futures/options/forex trading enabled; no daemon runtime started; only the files in this bundle's stated scope changed.

## 59. CRYPTO-DATA-01X-Y-KRAKEN-INGEST-PROVIDER-DB-PROOF-BUNDLE-01-COMBINED Closure Note (maintenance)

Continuing after `§58` (`01U-V-W`, Kraken parser + read-only dry-run, no DB write), this bundle closed the DB-backed ingest gap that `01U-V-W` explicitly deferred, closing two adjacent slices together: `CRYPTO-DATA-01X-KRAKEN-INGEST-PROVIDER-CLI-GATE-01`, `CRYPTO-DATA-01Y-KRAKEN-MD-BARS-DB-PROOF-01`. This is a provider-ingest DB-proof patch only — no recurring ingestion, no scheduler registration, no GUI, no DB migration, no broker/risk/execution/runtime/strategy change, no crypto trading enablement. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** direct inspection of `mqk-cli/src/commands/md.rs::md_ingest_provider`/`md_sync_provider` at HEAD `fbfcc17a` reconfirmed both commands hard-reject any `--source` other than `"twelvedata"`/`"alpaca"` and have no fixture/`--input-file` mode at all — both always call `HistoricalProvider::fetch_bars` over a chunked `--start`/`--end` date range sized for TwelveData's response cap. Rather than widen that date-range/chunking contract to accommodate Kraken's single fixed-shape fixture response (or special-case Kraken inside otherwise TwelveData/Alpaca-shaped code), this bundle added a new, additive `mqk md kraken-ohlc-ingest` command mirroring `kraken-ohlc-dry-run`'s (`01W`) parse/alias-resolution path exactly and adding only the `md_bars` DB write at the end. `ingest-provider`/`sync-provider` are byte-for-byte unchanged — same source allowlist, same chunking, same credential loading — zero regression risk to the equities path (confirmed by the unmodified `scenario_md_ingest_provider.rs`/`scenario_md_sync_provider.rs` suites still passing).

**DB write path:** `kraken-ohlc-ingest` calls the existing, unchanged, provider-agnostic `mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata` — no schema change, no migration — stamping `provider_id="kraken"`, `provider_source="kraken"`, `provider_symbol=<Kraken's own wire-level result key, e.g. "XXBTZUSD"/"XETHZUSD">` (not the alias config's query-string `kraken_pair`, so provenance records exactly what Kraken's API returned as its own key), `ingest_mode="provider_ingest"` (truthful, distinct from `"csv_import"`). Only `KrakenPairOhlc::completed_provider_bars()` output ever reaches the DB write call — the forming (not-yet-committed) row is never converted, never sent. The same fail-closed safety gate as `kraken-ohlc-dry-run` applies: without `--input-file`, the command refuses with `kraken_requires_input_file_or_network_opt_in` before any DB connection is attempted, unless `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` is explicitly set (never set during this bundle's validation).

**DB-backed proof (`01Y`):** `core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_ingest_db_01xy.rs` — a CLI-level test (the Kraken-specific alias/parse/metadata logic lives entirely in `mqk-cli`, so no separate `mqk-db`-level test was needed; the underlying upsert helper is already covered generically by `scenario_md_ingest_provider.rs`). Run against a real local Postgres (`postgres://postgres:postgres@127.0.0.1:5434/mqk_test`), it proved: `network_call_made=false`/`db_write=true`/`md_bars_write=true`; the forming row (`end_ts=1783296000`) is never written (`row_exists` returns `false`); scaled volume readback matches the documented exact values (BTC latest `131715941434`, BTC earlier `98012345678`, ETH latest `1558801759742`); `end_ts = row.time + 86400` (rows keyed at `1783123200`/`1783209600`); re-running the identical ingest is idempotent (`inserted=0`, `updated=2` on the second call, row count stays at 2 per symbol); `oms_outbox` row count is unchanged before vs. after the whole test; and cleanup — which deletes only rows matching the exact fixture `(symbol, timeframe='1D', end_ts)` keys **and** `provider_id='kraken'`, so it can never touch pre-existing non-Kraken history at the same canonical symbol — leaves a verified zero-row count for both `BTC/USD` and `ETH/USD`.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** the `kraken` provider config stays `enabled: false`; `sync-provider` (incremental backfill) still has no Kraken path, an explicit deferral; no recurring/scheduled Kraken ingestion of any kind; no daemon ingest job; no GUI status surface for Kraken ingest; Kraken's numeric public rate limit remains unestablished (unchanged from `01S-T`); Coinbase Exchange and Binance/Binance.US remain unverified candidates (unchanged); no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01X-Y-KRAKEN-INGEST-PROVIDER-DB-PROOF-BUNDLE-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); full spec document at `docs/specs/crypto_data_01x_y_kraken_ingest_provider_db_proof.md`; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

## 60. CRYPTO-DATA-01Z-AA-KRAKEN-INCREMENTAL-SYNC-DB-PROOF-BUNDLE-01-COMBINED Closure Note (maintenance)

Continuing after `§59` (`01X-Y`, one-shot fixture-first Kraken `md_bars` DB-write proof), this bundle added a Kraken-specific incremental sync command, closing two adjacent slices together: `CRYPTO-DATA-01Z-KRAKEN-INCREMENTAL-SYNC-CLI-01`, `CRYPTO-DATA-01AA-KRAKEN-INCREMENTAL-SYNC-DB-PROOF-01`. This is an incremental-sync CLI/DB-proof patch only — no recurring ingestion, no scheduler registration, no GUI, no DB migration, no `mqk-db` source change, no broker/risk/execution/runtime/strategy change, no crypto trading enablement. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** direct inspection of `mqk-cli/src/commands/md.rs`, `mqk-db/src/md.rs`, and `scenario_cli_kraken_ohlc_ingest_db_01xy.rs` at HEAD `5762cf88` confirmed `ingest_provider_bars_to_md_bars_with_provider_metadata` performs a plain `INSERT ... ON CONFLICT DO UPDATE` upsert with no row-content-diff "skip if unchanged" capability (only `rows_inserted`/`rows_updated` via an `(xmax = 0)` trick), and `mqk-cli` has no direct `sqlx` dependency in `src/` (only `dev-dependencies`) to run ad-hoc comparison queries. Rather than add a new `mqk-db/src/*` read helper or a `Cargo.toml` dependency change (both outside this bundle's file scope), the new `mqk md kraken-ohlc-sync` command reuses the existing, unmodified `mqk_db::md::latest_stored_bar_end_ts` helper to read the pre-sync high-water mark, classifies completed candidate bars as missing/new vs. existing-candidate by `end_ts` presence (not value-equality), and documents that limitation honestly rather than fabricating a content-diff skip the code cannot prove.

**Sync semantics:** default policy (`--no-update-existing` absent) sends every completed bar through the same unchanged upsert helper `kraken-ohlc-ingest` uses — a previously-stored row is always updated, never skipped by value-equality (`rows_skipped_if_known=0` always under this policy). The optional `--no-update-existing` flag proves a real, provable-by-presence skip instead: any completed bar whose `end_ts` was already covered by the pre-sync high-water mark is never sent to the write helper at all. Rows are stamped `ingest_mode="provider_sync"` (distinct from `kraken-ohlc-ingest`'s `"provider_ingest"`) with a separate deterministic `ingest_id` namespace, so DB rows and `md_quality_reports` history trace back to which command wrote them. Same fail-closed fixture/network gate as `kraken-ohlc-ingest`, with its own named reason `kraken_sync_requires_input_file_or_network_opt_in`.

**DB-backed proof (`01AA`):** `core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_sync_db_01zaa.rs`, run against the real local test Postgres (`postgres://postgres:postgres@127.0.0.1:5434/mqk_test`). Seeded a single stale BTC/USD row directly (bypassing the CLI) with values that provably differ from the fixture, then proved: first sync inserts the genuinely new row and updates the stale existing row to the fixture's real values (`inserted=1`/`updated=1`); the forming row (`end_ts=1783296000`) is never written; scaled-volume readback exactly matches documented values (BTC latest `131715941434`, BTC earlier `98012345678` post-correction, ETH latest `1558801759742`); `end_ts = row.time + 86400`; a second identical run is idempotent (`inserted=0`/`updated=2`, row count unchanged); a third run with `--no-update-existing` proves a real skip (`rows_skipped_if_known=2`, `inserted=0`, `updated=0`, `md_bars_write=false`); ETH/USD (no prior rows) proves the path is not hardcoded to one symbol (`inserted=2`); `oms_outbox` row count is unchanged before vs. after the whole test; and cleanup — deleting only rows matching the exact fixture `(symbol, timeframe='1D', end_ts)` keys **and** `provider_id='kraken'` — leaves a verified zero-row count for both symbols.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** the `kraken` provider config stays `enabled: false`; `sync-provider`/`ingest-provider` remain hard-locked to `"twelvedata"|"alpaca"`; no row-content-diff "skip if unchanged" detection exists — an explicit, documented deferral, not a bug; no recurring/scheduled Kraken sync of any kind; no daemon ingest job; no GUI status surface for Kraken sync; Kraken's numeric public rate limit remains unestablished (unchanged from `01S-T`); Coinbase Exchange and Binance/Binance.US remain unverified candidates (unchanged); no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01Z-AA-KRAKEN-INCREMENTAL-SYNC-DB-PROOF-BUNDLE-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); full spec document at `docs/specs/crypto_data_01z_aa_kraken_incremental_sync_db_proof.md`; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no live Kraken network call made during validation (fixture files only; `MQK_ALLOW_KRAKEN_NETWORK_SMOKE` never set); the `kraken` provider config stays `enabled: false`; no DB migration; the only DB mutation was the isolated, self-cleaning test proven to leave zero leftover rows and zero `oms_outbox` writes; no scheduler, no daemon ingest job, no GUI change; no broker/risk/execution/OMS/runtime/strategy file touched; `ingest-provider`/`sync-provider` source allowlists unchanged; no live or paper order submitted; no crypto/futures/options/forex trading enabled; no daemon runtime started; only the files in this bundle's stated scope changed.

## 61. CRYPTO-DATA-01AB-AC-KRAKEN-CONTENT-DIFF-SYNC-BUNDLE-01-COMBINED Closure Note (maintenance)

Continuing after `§60` (`01Z-AA`, presence-based Kraken incremental sync), this bundle closed the row-content-diff gap `01Z-AA` explicitly documented as open, closing two adjacent slices together: `CRYPTO-DATA-01AB-KRAKEN-MD-BARS-CONTENT-DIFF-HELPER-01`, `CRYPTO-DATA-01AC-KRAKEN-SYNC-CONTENT-DIFF-PROOF-01`. This is a content-diff read-helper/CLI-classification patch only — no recurring ingestion, no scheduler registration, no GUI, no DB migration, no schema change, no broker/risk/execution/runtime/strategy change, no crypto trading enablement. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** `01Z-AA` explicitly deferred content-diff skip semantics because it treated a new `mqk-db/src/*` read helper as out of its own stated file scope. This bundle adds exactly that: `mqk_db::md::fetch_md_bars_for_provider_sync_keys(pool, symbol, timeframe, end_ts_keys)` reads existing `md_bars` rows for exactly the candidate `end_ts` keys (never a broader scan, never a different symbol/timeframe, never a broker/order/OMS table), returning a `BTreeMap<i64, ExistingMdBarForProviderSync>` keyed by `end_ts`. A pure companion function, `mqk_db::md::provider_bar_matches_existing(candidate, metadata, existing)`, compares `open_micros`/`high_micros`/`low_micros`/`close_micros` (parsed via the same `price_to_micros` the write path uses), `volume`, `is_complete`, and provider provenance (`provider_id`/`provider_source`/`provider_symbol`/`ingest_mode`) — deliberately excluding `provider_bar_id`/`provider_updated_at_utc` since Kraken never supplies either and the write path always leaves them `None` (documented in code, not silently dropped). No other `mqk-db/src/md.rs` function was changed; five new unit tests cover the comparison helper's match/mismatch branches.

**Sync semantics:** `mqk-cli/src/commands/md.rs::md_kraken_ohlc_sync` now reads existing rows for the candidate keys before deciding what to write, classifying each completed (non-forming) bar as missing/new (no existing row — always written), changed (`provider_bar_matches_existing` returns `false` — written unless `--no-update-existing`, else counted `rows_changed_skipped_due_to_no_update_existing`), or unchanged (content identical — never written, `rows_skipped_unchanged`, regardless of the flag). If no bar requires a write, the upsert helper (and its `md_quality_reports` persistence) is not called at all. `sync_policy` is now the fixed string `content_diff_skip_unchanged_update_changed_insert_missing`; evidence bumped `schema_version` from `kraken-ohlc-sync-v1` to `kraken-ohlc-sync-v2` (new `rows_changed`/`rows_skipped_unchanged`/`rows_changed_skipped_due_to_no_update_existing` fields).

**DB-backed proof:** `core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_sync_db_01zaa.rs` (updated: idempotent re-run assertions changed from `updated=2` to `rows_skipped_unchanged=2`/`updated=0`; `--no-update-existing` now re-dirties the row first to prove a genuine changed-vs-unchanged distinction, then asserts the stale value survives DB readback) and the new `core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_content_diff_sync_db_01abac.rs`, both run against the real local test Postgres (`postgres://postgres:postgres@127.0.0.1:5434/mqk_test`). Together they prove: a stale seeded BTC/USD row is detected changed and corrected (`rows_changed=1`/`updated=1`), a missing row is inserted (`bars_missing_new=1`/`inserted=1`); the forming row is never written; scaled-volume readback exact (BTC latest `131715941434`, BTC earlier `98012345678`, ETH latest `1558801759742`); provider metadata corrected from a deliberately wrong seed (`provider_symbol=seed_stale` -> `XXBTZUSD`); `end_ts = row.time + 86400`; true idempotency (`rows_skipped_unchanged=2`, `inserted=0`, `updated=0`, `md_bars_write=false` on re-run — the write helper is not called at all); `--no-update-existing` detects a re-dirtied row as changed but leaves its stale value in place (`rows_changed_skipped_due_to_no_update_existing=1`, verified by readback), while a truly-unchanged row is independently counted `rows_skipped_unchanged=1` in the same run; a subsequent default-policy run corrects the still-stale row, proving the flag is opt-in per invocation; ETH/USD proves the same matrix on a second symbol; `oms_outbox` row count is unchanged before vs. after; cleanup — deleting only rows matching the exact fixture `(symbol, timeframe='1D', end_ts)` keys **and** `provider_id='kraken'` — leaves a verified zero-row count for both symbols.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** the `kraken` provider config stays `enabled: false`; `sync-provider`/`ingest-provider` remain hard-locked to `"twelvedata"|"alpaca"`; no recurring/scheduled Kraken sync of any kind; no daemon ingest job; no GUI status surface for Kraken sync (tracked as `CRYPTO-DATA-01AD-KRAKEN-SYNC-EVIDENCE-STATUS-ROUTE-01`/`CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01`); Kraken's numeric public rate limit remains unestablished (unchanged from `01S-T`); Coinbase Exchange and Binance/Binance.US remain unverified candidates (unchanged); no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01AB-AC-KRAKEN-CONTENT-DIFF-SYNC-BUNDLE-01-COMBINED` entry (end of the `ASSET-CORE-04`/`CRYPTO-DATA-01` section); full spec document at `docs/specs/crypto_data_01ab_ac_kraken_content_diff_sync.md`; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no live Kraken network call made during validation (fixture files only; `MQK_ALLOW_KRAKEN_NETWORK_SMOKE` never set); the `kraken` provider config stays `enabled: false`; no DB migration, no schema change; the only DB mutation was the isolated, self-cleaning tests proven to leave zero leftover rows and zero `oms_outbox` writes; no scheduler, no daemon ingest job, no GUI change; no broker/risk/execution/OMS/runtime/strategy file touched; `ingest-provider`/`sync-provider` source allowlists unchanged; no live or paper order submitted; no crypto/futures/options/forex trading enabled; no daemon runtime started; only the files in this bundle's stated scope changed.

## 62. CRYPTO-DATA-01AD-KRAKEN-SYNC-EVIDENCE-STATUS-ROUTE-01 Closure Note (maintenance)

Continuing after `§61` (`01AB-AC`, content-diff-aware Kraken sync), this bundle added a read-only daemon status route exposing Kraken OHLC ingest/sync evidence, mirroring the existing `CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED` CoinLore latest-mark pattern. This is a data/operator-visibility patch only — no DB connection opened by the route, no provider/network call, no CLI/sync trigger, no scheduler, no daemon ingest job, no GUI change, no broker/risk/execution/OMS/runtime/strategy change, no crypto trading enablement. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** `GET /api/v1/market-data/kraken-ohlc/status` (`mqk-daemon/src/routes/transport_quality.rs::kraken_ohlc_status`) reads the same evidence directory as the sibling `latest-marks/status`/`intraday-refresh/status` routes (`st.md_refresh_evidence_dir`), filtered to two filename prefixes: `kraken_ohlc_ingest_*.json` (`01X-Y`) and `kraken_ohlc_sync_*.json` (`01Z-AA`/`01AB-AC`). Because `"kraken_ohlc_ingest_"` sorts alphabetically before `"kraken_ohlc_sync_"` regardless of which file is actually newer, the route selects the latest candidate by the epoch-seconds timestamp embedded in each filename rather than by string comparison — a correctness detail the single-prefix `latest_mark_status` route didn't need to handle. `KrakenOhlcStatusResponse` (`api_types.rs`) is a union of the `kraken-ohlc-ingest-v1`/`kraken-ohlc-sync-v2` evidence shapes; fields absent from the selected file's own schema (e.g. `sync_policy` when `latest_mode="ingest"`) are `None`, never fabricated with a default.

**Fail-closed safety checks (independent of the CLI's own invariants):** `kraken_ohlc_unsafe_reason` surfaces `"unsafe_evidence"` (never `"active"`, regardless of freshness) when: `provider` is not `"kraken"`; `network_call_made=true` without the evidence's own `mode="network_smoke"` (the only mode the CLI ever pairs with a real network call); a `completed_bar_claim=true` field is present (Kraken evidence never sets this — presence alone is suspicious); `forming_candle_excluded=false` is present; `db_write=false` while `rows_inserted`/`rows_updated` report values > 0 (internally inconsistent); `md_bars_write=true` with `bars_excluded_forming=0` (inconsistent with the committed fixtures, which always carry exactly one forming row); any of `volume_semantics`/`provider_id`/`provider_source`/`ingest_mode` is missing; or any top-level trading/execution-like field (`order_id`, `broker_order_id`, `fill_price`, `position_id`, `account_id`, `side`, `limit_price`, `stop_price`) is present. Only `schema_version` values `"kraken-ohlc-ingest-v1"`/`"kraken-ohlc-sync-v2"` are accepted — the retired `"kraken-ohlc-sync-v1"` (pre-`01AB-AC` shape) and anything else is `"parse_error"`.

**Test proof:** `core-rs/crates/mqk-daemon/tests/scenario_kraken_ohlc_status_route_01ad.rs`, 11 tests, no DB required (the route itself never opens one — confirmed by a test constructing `AppState` with no DB pool configured and asserting the `active` path still works): missing/empty evidence dir, valid fresh sync evidence (all sync-only fields parsed correctly, including the new `01AB-AC` content-diff fields `rows_changed`/`rows_skipped_unchanged`/`rows_changed_skipped_due_to_no_update_existing`), valid fresh ingest evidence (`latest_mode="ingest"`, sync-only fields confirmed `null`), stale evidence, malformed JSON (no panic), wrong `schema_version`, `forming_candle_excluded=false` → unsafe, wrong `provider` → unsafe, and epoch-based (not alphabetical) latest-file selection proven by seeding a higher-epoch ingest file alongside a lower-epoch sync file and confirming the ingest file wins.

**Clippy note (environmental, not a regression):** `cargo clippy -p mqk-daemon --all-targets -- -D warnings` fails on this machine's current toolchain (rustc 1.93) due to pre-existing `clippy::await_holding_lock` errors in `state/session_controller.rs` and `tests/scenario_runtime_session_v2_cutover_scaffold_asset_core_05d.rs` — neither file was touched by this bundle. Verified via `git stash`: the identical errors reproduce with none of this bundle's changes applied, confirming this is a pre-existing lint-vs-toolchain issue unrelated to this patch, not something introduced here. `cargo clippy -p mqk-daemon --lib -- -D warnings` and `cargo clippy -p mqk-daemon --test scenario_kraken_ohlc_status_route_01ad -- -D warnings` (scoped to only this bundle's own code) are both clean.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** no GUI surface consumes this route yet (tracked as `CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01`, next); the `kraken` provider config stays `enabled: false`; `sync-provider`/`ingest-provider` remain hard-locked to `"twelvedata"|"alpaca"`; no recurring/scheduled Kraken sync of any kind; no daemon ingest job; the pre-existing `mqk-daemon` clippy toolchain issue above remains unresolved (out of this bundle's scope — unrelated files); no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01AD-KRAKEN-SYNC-EVIDENCE-STATUS-ROUTE-01` entry; full spec document at `docs/specs/crypto_data_01ad_kraken_sync_evidence_status_route.md`; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no live Kraken network call (route never calls Kraken); no DB connection opened by this route; no CLI execution triggered; no scheduler, no daemon ingest job, no GUI change; no broker/risk/execution/OMS/runtime/strategy file touched; no live or paper order submitted; no crypto/futures/options/forex trading enabled; no daemon runtime started; only the files in this bundle's stated scope changed.

## 63. CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01 Closure Note (maintenance)

Continuing after `§62` (`01AD`, read-only Kraken OHLC evidence status route), this bundle added a read-only GUI panel consuming that route, mirroring the existing "Crypto latest marks" (CoinLore) panel. This is a GUI/operator-visibility patch only — no backend/route/DB/CLI change, no provider/network call, no scheduler, no daemon runtime, no crypto trading enablement. Recorded here per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** `core-rs/mqk-gui/src/features/ingest/types.ts` gained `KrakenOhlcStatusResponse`, a field-for-field TypeScript mirror of `api_types.rs::KrakenOhlcStatusResponse` (sync-only fields typed `T | null`, matching the backend's "never fabricated" contract). `api.ts` gained `fetchKrakenOhlcStatus` plus pure helpers `isKrakenOhlcStatusActive`, `krakenOhlcStatusTruthLabel`, and `isKrakenOhlcEvidenceUnsafe` — the last a GUI-side defense-in-depth check independent of the backend's own `truth_state` classification (also flags `provider !== "kraken"` and an internally inconsistent `db_write=false` with `rows_inserted`/`rows_updated` > 0). The exported `KRAKEN_OHLC_STATUS_WARNING_TEXT` constant carries the mission's required fixed warning sentence verbatim, so it can be asserted in tests without string-duplicating it inline in the component.

**GUI panel:** `IngestScreen.tsx` renders a new "Kraken OHLCV sync status" `Panel` (auto-loaded on mount, manual Refresh button that only re-issues the same read-only GET) displaying `truth_state`/`provider`/`latest_mode`/`produced_at_utc`/`evidence_path`/staleness, `network_call_made`/`db_write`/`md_bars_write`, provider provenance, `sync_policy`, symbols requested, the full bar/row-count grid including the `01AB-AC` content-diff fields (`rows_changed`, `rows_skipped_unchanged`, `rows_changed_skipped_due_to_no_update_existing`, `rows_inserted`, `rows_updated`, `rows_skipped_if_known`), latest end_ts fields, volume semantics/scale, `all_passed`/`reason_code`/`fail_reasons`, the fixed warning notice, and an `unsafe_evidence` critical banner independent of the literal `truth_state` value. No GUI element calls a provider, CLI, or scheduler.

**Test proof:** `core-rs/mqk-gui/src/features/ingest/__tests__/api.test.ts` gained 28 tests mirroring the existing `fetchLatestMarkStatus`/`isLatestMarkStatusActive`/`latestMarkStatusTruthLabel`/`isLatestMarkEvidenceUnsafe` test pattern exactly: all 6 `truth_state` values for both `isKrakenOhlcStatusActive` and `krakenOhlcStatusTruthLabel` (plus an unknown-passthrough case and an explicit "worded as unsafe, not bare passthrough" assertion), 6 `isKrakenOhlcEvidenceUnsafe` cases, full response-shape tests for active-sync/active-ingest (proving sync-only fields stay `null` for ingest-mode evidence)/`no_evidence`/`unsafe_evidence` (including the content-diff fields' presence and typing), a warning-text content assertion, and route/transport-failure/backend-unavailable tests for `fetchKrakenOhlcStatus`. `npm test -- --run`: 628/628 pass (600 pre-existing + 28 new, zero regressions). `npm run build`: `tsc` type-checks clean, `vite build` succeeds.

**Manual browser verification not performed, honestly:** the Ingest screen requires a live daemon connection to render past its "Connecting to daemon…" bootstrap screen, and this mission's hard safety rules forbid starting daemon runtime for this patch. The exhaustive pure-function/fetch-mock unit test suite above (plus the clean `tsc`/`vite build`) is the verification standard used instead — the same standard the sibling CoinLore latest-mark panel was originally verified against in `01Q-R`.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain `PARTIAL`, not `CLOSED`):** the `kraken` provider config stays `enabled: false`; `sync-provider`/`ingest-provider` remain hard-locked to `"twelvedata"|"alpaca"`; no recurring/scheduled Kraken sync of any kind; no daemon ingest job; no live in-browser render proof against a running daemon (deliberately not attempted, per the safety rules above — not an oversight); no production registry-v2 cutover; no crypto session/calendar runtime enforcement; no crypto risk policy activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01` entry; runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no provider/network call from the GUI; no DB connection from the GUI; no CLI execution triggered; no scheduler button; no daemon runtime started for this patch; no broker/risk/execution/OMS/runtime/strategy file touched; no live or paper order submitted; no crypto/futures/options/forex trading enabled; only the files in this bundle's stated scope changed.

## 64. CRYPTO-REGISTRY-02-KRAKEN-DATA-REGISTRY-CUTOVER-DECISION-01 Closure Note (maintenance)

Continuing after `§63` (`01AE`, read-only Kraken OHLCV GUI status panel), this
decision patch answers what "production registry-v2 cutover" means for
`BTC/USD`/`ETH/USD` Kraken data. Decision/spec patch only — no config flag, no
source code, no scheduler, no trading enablement. Recorded here per
`audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** direct inspection of `instrument_registry_v2.rs` confirmed
`enabled`, `paper_trading_enabled`, and `live_trading_enabled` are three
independent boolean fields, and that no production execution/broker/risk/OMS
path reads this schema — the only consumer is `ASSET-CORE-04F`'s read-only,
default-off `?registry_source=v2` economics-status bridge. `validate_registry_v2`
fail-closed rejects `enabled=true` for any non-equity instrument without the
test-only `allow_enabled_non_equity_for_testing` flag, itself documented as
having no production enablement path. Decision: keep the existing
`instruments_v2.crypto_local_marks.example.json` fixture unchanged (no new
candidate file, no flag flip); classify Kraken as data-path-ready but not
production-default/scheduled/tradable
(`registry_cutover_status: "decision_only"`).

**Test/validation proof:** `python -m json.tool` on the decision JSON parses
clean; `validate_crypto_registry_02_kraken_cutover_decision.ps1` — all 10
checks pass; the two pre-existing referenced guard scripts
(`validate_crypto_data_01s_t_ohlcv_provider_decision.ps1`,
`validate_crypto_data_01i_coinlore_verify.ps1`) still pass, unaffected. No
Rust/GUI build or test run in this phase — no source code changed.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain
`PARTIAL`, not `CLOSED`):** `kraken.enabled` stays `false`; `BTC/USD`/`ETH/USD`
`enabled`/`paper_trading_enabled`/`live_trading_enabled` all stay `false`; no
recurring/scheduled Kraken sync; no daemon ingest job; no production
registry-v2 cutover; no crypto session/calendar runtime enforcement; no
crypto risk policy activation; no crypto broker/paper execution; no crypto
strategy.

**Full detail, exact evidence citations, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`CRYPTO-REGISTRY-02-KRAKEN-DATA-REGISTRY-CUTOVER-DECISION-01` entry; decision
doc at `docs/specs/crypto_registry_02_kraken_data_registry_cutover_decision.md`;
runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no config flag changed; no source code changed; no
network call; no DB connection; no scheduler added; no daemon runtime
started; no broker/risk/execution/OMS/runtime/strategy file touched; no live
or paper order submitted; no crypto/futures/options/forex trading enabled;
only the files in this bundle's stated scope changed.

## 65. CRYPTO-REGISTRY-03-KRAKEN-DATA-ONLY-REGISTRY-READINESS-CLI-01 Closure Note (maintenance)

Continuing after `§64` (`CRYPTO-REGISTRY-02`, cutover decision), this bundle
adds `mqk-cli md crypto-registry-readiness`, a read-only operator CLI proving
the current, unmodified registry-v2 fixture and providers config are ready
for data-only Kraken OHLCV operations for `BTC/USD`/`ETH/USD`. Recorded here
per `audit_repo_truth_rules.md` rather than left only in commit history.

**Concretely:** the command reuses two already-proven pure loaders
(`mqk_md::provider_registry::load_provider_registry`,
`mqk_md::instrument_registry_v2::load_instrument_registry_v2`) — no new
`mqk-md` code was added. It classifies provider existence/enablement,
per-symbol asset class, Kraken alias completeness
(`kraken_pair`/`kraken_result_key`), and trading-flag safety
(`paper_trading_enabled`/`live_trading_enabled`). The current disabled state
classifies as `data_readiness_state=data_ready_manual_only` — expected and
correct, not a failure. `kraken.enabled=true` fails closed as
`unsafe_provider_enabled`, consistent with `CRYPTO-REGISTRY-02`'s decision
not to permit provider enablement yet.

**Test proof:** `core-rs/crates/mqk-cli/tests/scenario_cli_crypto_registry_readiness_03.rs`
gained 9 tests invoking the built binary: happy path (exit 0,
`active`/`data_ready_manual_only`), missing provider, missing/incomplete
Kraken alias, missing symbol, `paper_trading_enabled=true`,
`live_trading_enabled=true`, `kraken.enabled=true` (six failure modes, each
nonzero exit with a distinct `truth_state`), an evidence-JSON content proof
(no DB/network/order/broker/position/account key anywhere in the envelope),
and a byte-for-byte proof that `--registry`/`--providers` are never mutated.
`cargo test -p mqk-cli --test scenario_cli_crypto_registry_readiness_03` —
9/9 pass. `cargo clippy -p mqk-cli -p mqk-md --all-targets -- -D warnings` —
clean. Regression: `scenario_crypto_local_marks_registry_data_01a` (31/31),
`scenario_kraken_ohlcv_provider_01uvw` (7/7), and
`scenario_cli_kraken_ohlc_dry_run_01w` (7/7) all still pass, unaffected.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain
`PARTIAL`, not `CLOSED`):** no daemon route or GUI panel yet (tracked as
`CRYPTO-REGISTRY-04`, conditional next slice); `kraken.enabled` stays
`false`; no registry flag flipped; no recurring/scheduled Kraken sync; no
production registry-v2 cutover; no crypto risk policy activation; no crypto
broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`CRYPTO-REGISTRY-03-KRAKEN-DATA-ONLY-REGISTRY-READINESS-CLI-01` entry;
runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no config flag changed; `config/providers/providers.json`
and `config/instruments/instruments_v2.crypto_local_marks.example.json` both
byte-for-byte unchanged (proven by test `rc_09`); no network call; no DB
connection; no scheduler added; no daemon runtime started; no
broker/risk/execution/OMS/runtime/strategy file touched; no live or paper
order submitted; no crypto/futures/options/forex trading enabled; only the
files in this bundle's stated scope changed.

## 66. CRYPTO-REGISTRY-04-KRAKEN-DATA-ONLY-REGISTRY-STATUS-SURFACE-01 Closure Note (maintenance)

Continuing after `§65` (`CRYPTO-REGISTRY-03`, readiness CLI), this bundle
exposes the same read-only classification through a daemon route and GUI
panel, since Phase B stayed small enough to proceed without stopping.
Recorded here per `audit_repo_truth_rules.md` rather than left only in
commit history.

**Concretely:** `GET /api/v1/market-data/crypto-registry/readiness`
reimplements the CLI's classification using the same two pure `mqk-md`
loaders already used elsewhere in the daemon — no new `mqk-md` code. Path
resolution reuses two pre-existing `AppState` fields
(`instrument_registry_v2_path`, `provider_registry_path`) with no new field
added; the registry path falls back to the committed
`instruments_v2.crypto_local_marks.example.json` fixture when
`MQK_INSTRUMENT_REGISTRY_V2_PATH` is unset. Neither config file is ever
mutated. The GUI's "Crypto registry readiness" panel (Ingest screen)
displays provider/readiness-state fields, per-symbol alias/trading-flag
status, and fail reasons behind the fixed warning: *"Registry readiness is
data-pipeline visibility only. It does not enable crypto trading, broker
routing, strategy execution, or scheduling."* No button mutates config or
triggers a sync.

**Test proof:** `core-rs/crates/mqk-daemon/tests/scenario_crypto_registry_readiness_route_04.rs`
gained 8 tests (real fixtures, missing provider, missing/incomplete alias,
missing symbol, both trading-flag-unsafe cases, provider-enabled-unsafe
case, no-DB-pool proof) — all `200 OK` with a distinct `truth_state`.
`core-rs/mqk-gui/src/features/ingest/__tests__/api.test.ts` gained 25 tests
mirroring the Kraken OHLC status test pattern. `cargo test -p mqk-daemon
--test scenario_crypto_registry_readiness_route_04` — 8/8 pass. `npm test
-- --run` — 653/653 pass (628 pre-existing + 25 new, zero regressions).
`npm run build` — clean. `cargo clippy -p mqk-daemon --lib` and the scoped
test-target clippy run both clean; `--all-targets` still fails only on the
same pre-existing, unrelated `await_holding_lock` errors in
`state/session_controller.rs` already documented in the `CRYPTO-DATA-01AD`
closure note (§62) — confirmed not a regression.

**Manual browser verification not performed, honestly:** this mission's
hard safety rule forbids starting daemon runtime beyond isolated route
tests for this phase, and the Ingest screen requires a live daemon to
render past its bootstrap screen — the same constraint and resolution
already documented for the sibling Kraken OHLCV panel (§63).

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain
`PARTIAL`, not `CLOSED`):** `kraken.enabled` stays `false`; no registry flag
flipped; no recurring/scheduled Kraken sync; no production registry-v2
cutover; no crypto risk policy activation; no crypto broker/paper
execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`CRYPTO-REGISTRY-04-KRAKEN-DATA-ONLY-REGISTRY-STATUS-SURFACE-01` entry;
runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no config flag changed; no config file mutated; no
network call; no DB connection from the route; no scheduler added; no
daemon runtime started outside isolated Rust tests; no
broker/risk/execution/OMS/runtime/strategy file touched; no live or paper
order submitted; no crypto/futures/options/forex trading enabled; only the
files in this bundle's stated scope changed.

## 67. CRYPTO-DATA-02A-KRAKEN-SCHEDULER-RATE-LIMIT-DECISION-01 Closure Note (maintenance)

Continuing after `§66` (`CRYPTO-REGISTRY-04`, registry readiness route/GUI),
this decision patch verifies Kraken's public-endpoint rate-limit guidance
via 2 bounded, keyless documentation-page reads (zero Kraken API calls) and
records a conservative, repo-local cadence policy for a **future** Kraken
scheduled sync. No scheduler is registered by this patch. Recorded here per
`audit_repo_truth_rules.md` rather than left only in commit history.

**Sources checked:** `docs.kraken.com/api/docs/guides/spot-rest-ratelimits`
(authenticated-tier call-counter system only, does not cover public
endpoints) and `support.kraken.com/hc/en-us/articles/206548367` (states
public endpoints may be called at up to 1 request/second and remain within
limits; OHLC/Trades specifically rate-limited by IP address **and**
currency pair; exceeding the limit triggers temporary throttling).
`rate_limit_verification_status="verified"`.

**Decision:** daily cadence; sequential-only pair calls (never concurrent);
minimum 2 seconds between per-pair calls (2x the verified guideline);
maximum 2 OHLC calls per run (`BTC/USD` + `ETH/USD`); bounded exponential
backoff retries (max 2, jitter, fail-closed once exhausted); explicit
invariant list required before any future task-registration patch (registry
readiness must be active/ready, Kraken evidence must not be unsafe, trading
flags must stay false, task must not share a lock/cursor with any existing
scheduler).

**Built:** `docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.md`,
`docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json`,
`scripts/guards/validate_crypto_data_02a_kraken_scheduler_decision.ps1`.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain
`PARTIAL`, not `CLOSED`):** no scheduler registered; no Windows Scheduled
Task; no daemon recurring job; `kraken.enabled` stays `false`; no registry
flag flipped; no production registry-v2 cutover; no crypto risk policy
activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`CRYPTO-DATA-02A-KRAKEN-SCHEDULER-RATE-LIMIT-DECISION-01` entry; runbook
update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no config flag changed; no config file mutated; no
Kraken API call (only 2 bounded documentation/support page fetches); no DB
connection; no scheduler registered; no daemon job added; no daemon runtime
started; no broker/risk/execution/OMS/runtime/strategy file touched; no
live or paper order submitted; no crypto/futures/options/forex trading
enabled; only the files in this bundle's stated scope changed.

## 68. CRYPTO-DATA-02B-KRAKEN-SCHEDULER-READINESS-CLI-01 Closure Note (maintenance)

Continuing after `§67` (`CRYPTO-DATA-02A`, scheduler rate-limit decision),
this bundle adds `mqk-cli md kraken-scheduler-readiness`, a read-only
operator CLI proving whether a **future** Kraken scheduled sync is
currently allowed by the `02A` policy, the current provider/registry
config, and (optionally) the latest Kraken OHLC evidence. Recorded here per
`audit_repo_truth_rules.md` rather than left only in commit history.

**Truth states:** `active`, `policy_missing`, `policy_invalid`,
`registry_unsafe`, `provider_unsafe`, `trading_flags_unsafe`,
`scheduler_already_registered`, `evidence_unsafe`, `parse_error`,
`backend_unavailable`. `active` never means a scheduler is registered — it
means prerequisites are satisfied for a future, separately authorized
registration patch.

**Test proof:** `core-rs/crates/mqk-cli/tests/scenario_cli_kraken_scheduler_readiness_02b.rs`,
12/12 pass, covering the happy path, missing/invalid policy, unsafe
provider/registry/trading-flag states, evidence required-vs-warning-only
behavior, evidence-JSON safety-claim proof, and no-file-mutation proof.

**Build note:** added `#![recursion_limit = "256"]` to `mqk-cli`'s crate
root — a compiler macro-expansion limit needed for the new command's
`serde_json::json!` evidence envelope; not a runtime/behavior change.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain
`PARTIAL`, not `CLOSED`):** no scheduler registered; no Windows Scheduled
Task; no daemon recurring job; `kraken.enabled` stays `false`; no registry
flag flipped; no production registry-v2 cutover; no crypto risk policy
activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`CRYPTO-DATA-02B-KRAKEN-SCHEDULER-READINESS-CLI-01` entry; runbook update
at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no config flag changed; no config file mutated; no
Kraken API call; no provider/network call; no DB connection; no scheduler
registered; no daemon job added; no daemon runtime started; no
broker/risk/execution/OMS/runtime/strategy file touched; no live or paper
order submitted; no crypto/futures/options/forex trading enabled; only the files in this bundle's stated scope changed.

## 69. CRYPTO-DATA-02C-KRAKEN-SCHEDULER-READINESS-STATUS-SURFACE-01 Closure Note (maintenance)

Continuing after `§68` (`CRYPTO-DATA-02B`, scheduler readiness CLI), this
bundle exposes the same read-only classification through a daemon route
and GUI panel, since Phase B stayed small enough to proceed without
stopping. Recorded here per `audit_repo_truth_rules.md` rather than left
only in commit history.

**Built:** `GET /api/v1/market-data/kraken-scheduler/readiness` (an
independent reimplementation of the CLI's classification, not a shared code
path); new `AppState.kraken_scheduler_policy_path` field (default the
committed `CRYPTO-DATA-02A` policy JSON, override
`MQK_KRAKEN_SCHEDULER_POLICY_PATH`); "Kraken scheduler readiness" read-only
GUI panel on the Ingest screen.

**Test proof:** `core-rs/crates/mqk-daemon/tests/scenario_kraken_scheduler_readiness_route_02c.rs`,
8/8 pass; GUI `npm test -- --run`, 687/687 pass (34 new). `cargo clippy -p
mqk-daemon --lib -- -D warnings` clean after fixing one
`clippy::unnecessary_get_then_check` lint caught during this pass (not a
pre-existing issue — introduced and fixed within this same bundle).
`cargo clippy -p mqk-daemon --all-targets -- -D warnings` still fails only
on the same pre-existing, unrelated `await_holding_lock` errors in
`state/session_controller.rs` already documented in the `CRYPTO-DATA-01AD`
closure note (§62) — confirmed not a regression.

**Manual browser verification not performed, honestly:** this mission's
hard safety rule forbids starting daemon runtime beyond isolated route
tests for this phase, and the Ingest screen requires a live daemon to
render past its bootstrap screen — the same constraint and resolution
already documented for the sibling Kraken OHLCV/registry-readiness panels
(§63, §66).

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain
`PARTIAL`, not `CLOSED`):** no scheduler registered; no Windows Scheduled
Task; no daemon recurring job; `kraken.enabled` stays `false`; no registry
flag flipped; no production registry-v2 cutover; no crypto risk policy
activation; no crypto broker/paper execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`CRYPTO-DATA-02C-KRAKEN-SCHEDULER-READINESS-STATUS-SURFACE-01` entry;
runbook update at `docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation:** no config flag changed; no config file mutated; no
Kraken API call; no provider/network call; no DB connection from the
route; no scheduler registered; no daemon job added; no daemon runtime
started outside isolated Rust tests; no
broker/risk/execution/OMS/runtime/strategy file touched; no live or paper
order submitted; no crypto/futures/options/forex trading enabled; only the
files in this bundle's stated scope changed.

## 70. CRYPTO-DATA-03A-KRAKEN-SCHEDULED-NETWORK-GATE-01 Closure Note (maintenance)

Continuing after `§69` (`CRYPTO-DATA-02C`, scheduler readiness route/GUI),
this patch adds a second, explicit network opt-in env var,
`MQK_ALLOW_KRAKEN_SCHEDULED_SYNC`, to `kraken-ohlc-sync` alongside the
existing manual-smoke gate, plus a fail-closed DB-url-presence check before
any network call. Recorded here per `audit_repo_truth_rules.md` rather than
left only in commit history.

**Built:** pure `kraken_sync_network_gate` function in
`core-rs/crates/mqk-cli/src/commands/md.rs` (4 unit tests), new
`network_authorization_mode` evidence field, new
`scenario_cli_kraken_scheduler_task_gate_03a.rs` integration test file.

**Safety incident disclosed and remediated:** during development of this
patch's tests, a design that removed `MQK_DATABASE_URL` from a test
subprocess's environment did not account for `mqk-cli`'s pre-existing
`dotenvy::from_filename(".env.local")` bootstrap re-populating it inside
the child process. This let two test runs reach a genuine live Kraken
public-OHLC network call and a genuine local paper-DB write
(`postgres://…@127.0.0.1:5440/miniquantdesk_paper`) before the fail-closed
DB-url check could matter. Caught immediately (the tests failed with
unexpected success, not silent success), disclosed to the operator before
any remediation, and cleaned up with the operator's explicit go-ahead: 720
stray `provider_id='kraken'`, `symbol='BTC/USD'` rows were confirmed via
`psql` and deleted; a re-query confirmed zero remain; `oms_outbox` was
confirmed unaffected. The test suite was redesigned so the "env var
recognized" cases are proven by the pure-function unit tests instead of a
subprocess, eliminating the failure mode. Full narrative in the
`CRYPTO-DATA-03A-KRAKEN-SCHEDULED-NETWORK-GATE-01` ledger entry.

**Test proof:** `scenario_cli_kraken_scheduler_task_gate_03a.rs` — 2/2
non-ignored subprocess tests pass, 1 correctly `#[ignore]`d; 4/4 pure unit
tests pass (`cargo test -p mqk-cli --bin mqk-cli kraken_sync_network_gate`).
Regressions: `scenario_cli_kraken_ohlc_sync_db_01zaa` (1/1 non-ignored),
`scenario_cli_kraken_ohlc_dry_run_01w` (7/7), `scenario_cli_kraken_scheduler_readiness_02b`
(12/12) — all pass, unaffected. `cargo clippy -p mqk-cli -p mqk-md -p
mqk-db --all-targets -- -D warnings` — clean.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01`
remain `PARTIAL`, not `CLOSED`):** no scheduler registered; no Windows
Scheduled Task; no daemon recurring job; `kraken.enabled` stays `false`;
no registry flag flipped; no production registry-v2 cutover; no crypto
risk policy activation; no crypto broker/paper execution; no crypto
strategy.

**Full detail, exact evidence citations, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`CRYPTO-DATA-03A-KRAKEN-SCHEDULED-NETWORK-GATE-01` entry; runbook update at
`docs/runbooks/local_crypto_marks_ingest.md`.

**Safety confirmation (final validated state):** no config flag changed;
no config file mutated; zero live Kraken API calls in the committed test
suite; zero DB mutation in the committed test suite's default (non-
`--include-ignored`) run; no scheduler registered; no daemon job added; no
daemon runtime started; no broker/risk/execution/OMS/runtime/strategy file
touched; no live or paper order submitted; no crypto/futures/options/forex
trading enabled; only the files in this bundle's stated scope changed. The
one incident that did occur during development (see above) was fully
remediated before this note was written.

## 71. CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01 Closure Note (maintenance)

Continuing after `§70` (`CRYPTO-DATA-03A`, scheduled-sync network gate),
this patch adds the optional Windows runner (`Run-KrakenOhlcSync.ps1`) and
registration wrapper (`Register-KrakenOhlcSyncTask.ps1`) scripts for a
future Kraken OHLC scheduled sync, plus a 17-check validator. Recorded here
per `audit_repo_truth_rules.md` rather than left only in commit history.

**Built:** `scripts/windows/Run-KrakenOhlcSync.ps1`,
`scripts/windows/Register-KrakenOhlcSyncTask.ps1`,
`scripts/guards/validate_crypto_data_03b_kraken_scheduler_task_scripts.ps1`,
`docs/specs/crypto_data_03b_kraken_scheduler_task_scripts.md`.

**Test proof:** validator — 17/17 checks pass.
`Register-KrakenOhlcSyncTask.ps1 -CheckOnly` and
`Run-KrakenOhlcSync.ps1 -CheckOnly` (with the local paper DB URL set) both
exit 0 with evidence proving zero network/DB/task mutation; a direct
`Get-ScheduledTask` query confirmed no task was left registered after any
validation step. `-Register` was never invoked.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain
`PARTIAL`, not `CLOSED`):** no Windows Scheduled Task registered; no daemon
recurring job; `kraken.enabled` stays `false`; no registry flag flipped; no
crypto trading enablement.

**Full detail, exact evidence citations, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01` entry.

**Safety confirmation:** no config flag changed; no config file mutated; no
Kraken API call; no DB mutation; no scheduled task registered or started;
no daemon job added; no daemon runtime started;
no broker/risk/execution/OMS/runtime/strategy file touched; no live or
paper order submitted; no crypto trading enabled; only the files in this
bundle's stated scope changed.

## 72. CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01 Closure Note (maintenance)

Continuing after `§71` (`CRYPTO-DATA-03B`, scheduler task scripts), this
patch adds the deferred read-only status surface for the
`kraken_ohlc_task_registration.json` task-registration evidence contract:
`GET /api/v1/market-data/kraken-scheduler/task-status` plus a "Kraken
scheduled task status" GUI panel on the Ingest screen. Recorded here per
`audit_repo_truth_rules.md` rather than left only in commit history.

**Built:** `core-rs/crates/mqk-daemon/src/api_types.rs`
(`KrakenSchedulerTaskStatusResponse`), `core-rs/crates/mqk-daemon/src/routes/transport_quality.rs`
(`kraken_scheduler_task_status` handler), `core-rs/crates/mqk-daemon/src/routes.rs`
(route registration), `core-rs/crates/mqk-daemon/tests/scenario_kraken_scheduler_task_status_route_03c.rs`
(13 tests), `core-rs/mqk-gui/src/features/ingest/{types.ts,api.ts,IngestScreen.tsx}`,
`core-rs/mqk-gui/src/features/ingest/__tests__/api.test.ts` (24 new tests).

**Test proof:** `cargo test -p mqk-daemon --test scenario_kraken_scheduler_task_status_route_03c`
— 13/13 pass, covering missing/absent evidence, valid check-only and
register-mode evidence round-trips, malformed JSON, wrong schema_version,
and every fail-closed unsafe-evidence condition. Sibling routes
(`kraken-scheduler/readiness`, `kraken-ohlc/status`) re-verified unaffected
(8/8 and 11/11 pass). `cargo clippy -p mqk-daemon --lib -- -D warnings` —
clean. `npm test -- --run` (mqk-gui) — 711/711 pass. `npm run build`
(mqk-gui) — clean.

**Not resolved (`ASSET-CORE-04`/`CRYPTO-REGISTRY-01`/`CRYPTO-DATA-01` remain
`PARTIAL`, not `CLOSED`):** no Windows Scheduled Task registered by this or
any prior patch; no daemon recurring job; no production registry-v2
cutover; no crypto session/calendar runtime enforcement; no crypto risk; no
crypto paper/live execution; no crypto strategy.

**Full detail, exact evidence citations, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01` entry.

## 73. ASSET-CORE-01E-REGISTRY-V2-RATES-SCHEMA-GAP-01 Closure Note (maintenance)

A session asked to "start `ASSET-CORE-01`" found, on direct inspection at
`HEAD` (`1d51a69a`), that the requested enum-mapping/schema/validator-CLI
work already existed and was `CLOSED_LOCAL` under `ASSET-CORE-01A`/`01B`/`01C`
(and `01D`, and the full `CRYPTO-DATA-01A`..`03C` lineage). Rather than
duplicate it, this patch traced the ledger's "Recommended next slice" chain
to `HEAD`, found every thread blocked by hard session constraints (network
calls, production cutover, trading enablement), then read the registry-v2
schema directly against its own stated seven-category requirement
(equities, ETFs, crypto spot, futures, options, forex, rates/fixed income)
and found rates/fixed income was never modeled anywhere in
`ContractDefinitionV2`.

**Built:** `core-rs/crates/mqk-md/src/instrument_registry_v2.rs` —
`"rate"` added to `CANONICAL_ASSET_CLASSES_V2`;
`ContractDefinitionV2::Rate { issuer, maturity, coupon_bps,
face_value_micros }`; a `"rate"` match arm in `validate_contract_v2`
(fail-closed shape validation, same pattern as `future`/`option`/`crypto`/
`forex`); a `base_rate` test fixture; `rate` folded into the existing
exhaustive multi-class tests (`v2_01`, `v2_07`, `v2_17`, `econ05`, `sug04`);
two new tests (`v2_11b` field violations, `v2_11c` zero-coupon validates).
`core-rs/crates/mqk-daemon/src/routes/system.rs::contract_kind_label` gained
a `Rate { .. } => "rate"` arm (required for the crate to compile against the
new variant; label-only).

**Test proof:** `cargo test -p mqk-md instrument_registry_v2` — 50/50 pass.
`cargo test -p mqk-md` (full crate) — 353 total pass, zero regressions.
`cargo clippy -p mqk-md -p mqk-schemas --all-targets -- -D warnings` —
clean. `cargo check -p mqk-daemon -p mqk-cli` — clean. `cargo test -p
mqk-daemon --test scenario_instrument_registry_v2_status_asset_core_01c` —
13/13 pass (regression, unaffected). `cargo clippy -p mqk-daemon --lib --
-D warnings` — clean.

**Not resolved (`ASSET-CORE-01` remains `PARTIAL`, not `CLOSED`):**
`InstrumentRegistryV2` is still never read by any trading/execution/risk/
OMS/ingestion path; no live non-equity provider network-verified beyond
existing CoinLore/Kraken checks; no production registry-v2 cutover
decision made. Each requires crossing a boundary (execution-path change,
live network call, or an explicit cutover decision) this patch and its
predecessors were barred from crossing.

**Full detail, exact evidence citations, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`ASSET-CORE-01E-REGISTRY-V2-RATES-SCHEMA-GAP-01` entry, and
`docs/specs/asset_core_01e_registry_v2_rates_schema_gap.md`.

**Safety confirmation:** no config flag changed; no config file mutated; no
Kraken API call; no provider/network call of any kind; no DB
connection/mutation; no scheduled task registered, unregistered, or
started; no daemon job added; no daemon runtime started except isolated
Axum route tests; no PowerShell subprocess or CLI execution from the new
route; no broker/risk/execution/OMS/runtime/strategy file touched; no live
or paper order submitted; no crypto trading enabled; no generated
evidence/log/export file staged; only the files in this patch's stated
scope changed.

## 74. ASSET-CORE-01F Closure Note (maintenance)

A follow-up session was asked to audit whether `ASSET-CORE-01` (Unified
Instrument Registry v2) was complete as a foundation, and close it if a
safe schema/model/docs/validator gap remained. Direct re-inspection at
`HEAD` (`e2680ac1`) confirmed `01A`-`01E` (§18-20, §34, §73 above) are all
`CLOSED_LOCAL` and, per §3 of
`docs/specs/asset_core_01f_instrument_registry_v2_completion_audit.md`, the
registry-v2 schema now models all seven asset categories (equity, ETF,
crypto spot, future, option, forex, rate/fixed income) with fail-closed
validation on every non-equity variant. No foundation gap (schema, model,
loader, validator, docs, or test-coverage) was found remaining — `01E`
already closed the last one (rates/fixed income).

**Verdict: `CLOSED_LOCAL / FOUNDATION-COMPLETE`.** `ASSET-CORE-01`'s own
scope — the instrument-registry v2 foundation itself — is done. Production
consumption (any trading/execution/risk/OMS/ingestion path reading
`InstrumentRegistryV2` as truth) was never part of that scope and remains
a separate, explicitly-named boundary; see
`docs/specs/asset_core_01h_instrument_registry_v2_consumption_boundary_decision.md`.
This audit changed no config flag, enabled no trading, made no network or
DB call, and touched no broker/execution/risk/OMS/runtime/strategy code.

**Full detail:** `docs/specs/asset_core_01f_instrument_registry_v2_completion_audit.md`.

## 75. BACKTEST-MULTIPLIER-MARGIN-01-COMPLETION-AUDIT-01 Closure Note (maintenance)

A completion-sweep session asked whether `BACKTEST-MULTIPLIER-MARGIN-01`
(prerequisite #1 named by `ASSET-CORE-01H`) is complete, and whether a safe
backtest-only gap remains. Direct re-inspection at `HEAD` (`6e6f69df`)
confirmed eleven sub-slices — `BACKTEST-MULTIPLIER-MARGIN-01-COMBINED`,
`BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED`, `BACKTEST-ECONOMICS-CONFIG-READY-01`,
`BACKTEST-REPORT-FIXTURE-READY-01-COMBINED`, `BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED`,
`BACKTEST-ECONOMICS-CLI-ENTRY-01-COMBINED`, `BACKTEST-ECONOMICS-DB-CLI-ENTRY-01-COMBINED`,
`BACKTEST-ECONOMICS-DAEMON-JOB-REQUEST-01-COMBINED`, `BACKTEST-ECONOMICS-GUI-REGISTRY-01-COMBINED`,
`BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01-COMBINED`, and `INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED`
— are all `CLOSED_LOCAL`. `mqk backtest csv` and `mqk backtest db` both
support explicit economics flags reaching `metrics.json`/`report.md`/
`manifest.json`; the daemon job route and GUI submit form do too. Direct
code read confirmed `mqk backtest csv-sweep` has **no** economics flags at
all and silently forces every sweep run to default equity economics — the
one concrete, safe, backtest-only gap remaining. No daemon or GUI sweep
feature exists, so there is no gap there. Margin fields remain metadata-only
(`margin_enforced` hardcoded `false` everywhere, read by nothing) and
`mqk-portfolio` (live/paper accounting) was never modified by any sub-slice.

**Verdict: `PARTIAL / SAFE-GAPS-REMAIN`** as of this audit. This session's
follow-on entries (`BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01` and
`BACKTEST-MULTIPLIER-MARGIN-01-CLOSURE-OR-BOUNDARY-DECISION-01`, if present
below) record whether that gap was closed and the resulting final status.
This audit changed no config flag, enabled no trading, made no network or
DB call, and touched no broker/execution/risk/OMS/runtime/strategy/
portfolio code.

**Full detail:** `docs/specs/backtest_multiplier_margin_01_completion_audit.md`.

## 76. BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01 Closure Note (maintenance)

The one safe gap §75 identified — `mqk backtest csv-sweep` silently forcing
every sweep run to default equity economics — is now closed. `csv-sweep`
carries the same opt-in `--contract-multiplier`/`--initial-margin-micros`/
`--maintenance-margin-micros` flags as `csv`/`db`, applied identically to
every sweep combination's `BacktestEngine` via the same
`build_backtest_economics_from_cli_flags` → `.with_economics(...)` pattern.
Because `mqk_artifacts::write_backtest_report`'s manifest-economics merge is
already generic to every caller (not per-call-site special-cased), every
sweep point's `manifest.json`/`metrics.json` now carries truthful economics
with zero `mqk-artifacts` change required. Proven by
`mqk-cli/tests/scenario_cli_backtest_csv_sweep_economics.rs` (4/4 passing:
default-preserves-equity, multiplier-50-reaches-every-point,
margin-metadata-not-enforced, zero-multiplier-fails-closed), with zero
regressions across the full `mqk-cli`/`mqk-backtest`/`mqk-artifacts` test
suites and clean clippy on every touched crate. No daemon route or GUI
change was needed — neither a sweep job route nor a sweep GUI feature
exists anywhere in this repo. No margin enforcement, no `mqk-portfolio`/
broker/risk/runtime/DB/config change.

This closes the last backtest-only gap named in §75. See
`docs/specs/backtest_multiplier_margin_01_closure_decision.md` for the
resulting final parent-label status.

**Full detail:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01` entry.

## 77. BACKTEST-MULTIPLIER-MARGIN-01-CLOSURE-OR-BOUNDARY-DECISION-01 Closure Note (maintenance)

With the last known backtest-only gap closed (§76), this session decided
the parent label's final status: **`CLOSED_LOCAL / BACKTEST-COMPLETE`** for
multiplier-aware backtest economics. Every backtest entry point in this
repo (`csv`, `db`, `csv-sweep`, the daemon job route, the GUI submit form)
supports explicit economics; every artifact surface
(`BacktestReport`/`metrics.json`/`report.md`/`manifest.json`) carries it
truthfully. Margin remains metadata-only (`margin_enforced` hardcoded
`false`, enforced by nothing) — this was never claimed otherwise and is not
claimed otherwise now. `mqk-portfolio` (live/paper accounting) is untouched
by every sub-slice in this lineage; `mqk-backtest` is not a dependency of
`mqk-runtime`, so there is no path for this economics seam to reach
live/paper P&L.

This closes **prerequisite #1** of `ASSET-CORE-01H`'s five-item
production-cutover checklist. It does **not** authorize
`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` — the other four prerequisites
(symbol/instrument-id translation, Gate 0/routing-guard parity, a
live-network-verified non-equity provider, and an explicit operator
enablement decision) remain entirely open.

**Full detail:** `docs/specs/backtest_multiplier_margin_01_closure_decision.md`.

## 78. ROADMAP-COMPLETION-RECONCILE-01 Closure Note (maintenance)

A completion-sweep session reconciled the multi-asset roadmap after closing
`BACKTEST-MULTIPLIER-MARGIN-01` (§75-§77 above). Two stale-relative-to-HEAD
rows were corrected in §5's roadmap table: `ASSET-CORE-04`'s evidence column
now separately names the additive, zero-live-caller `04A`-`04F` economics
model/bridge/aggregation/status scaffold (already committed prior to this
session, previously undistinguished from the still-unchanged live-ledger
gap it sits beside — the live path itself remains single-currency,
multiplier-naive, whole-unit-`i64`-quantity, exactly as before);
`ASSET-CORE-05`'s percentage was bumped from 30% to ~35%, citing the
already-committed `ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED`
entry (read-only session-profile diagnostics) that had not yet been
reflected in the table.

`ASSET-CORE-02`, `ASSET-CORE-03`, and the full `CRYPTO-*` lane were left
unchanged — closing any of them further requires either a live network call
or broker/risk/execution code changes, all forbidden by this session's hard
safety rules; no new evidence was gathered for them this session.

**Next recommended patch:** a symbol/`instrument_id` translation layer
between `InstrumentRegistryV2` and the existing symbol-string-keyed tables
(`md_bars`, outbox rows, portfolio positions) — prerequisite #2 of
`ASSET-CORE-01H`'s five-item production-cutover checklist, now that
prerequisite #1 (`BACKTEST-MULTIPLIER-MARGIN-01`) is closed.
`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` itself is explicitly **not**
recommended next — four of its five prerequisites remain open.

This reconciliation changed no config flag, enabled no trading, made no
network or DB call, and touched no broker/execution/risk/OMS/runtime/
strategy/portfolio code.

**Full detail:** `docs/specs/roadmap_completion_reconcile_01.md`.

## 79. REGISTRY-V2-TRANSLATION-01A-SYMBOL-KEYED-CONSUMER-AUDIT-01 Closure Note (maintenance)

`CLOSED_LOCAL / AUDIT-ONLY`. Names every current symbol-string-keyed
production surface relevant to `ASSET-CORE-01H` prerequisite #2 (`md_bars`,
`oms_outbox` order_json, `mqk-portfolio`/`mqk-reconcile`/`mqk-runtime`
positions/snapshots, backtest/bar lookup CLI surfaces), confirms the current
88-row `config/instruments/equities.json` universe has zero duplicate
`symbol`/`instrument_id` values, and names the minimal translation contract
the next phase (`REGISTRY-V2-TRANSLATION-01B`) must implement. Docs-only —
no code touched, no trading enabled, no DB/network call made. Full detail:
`docs/specs/registry_v2_translation_01a_symbol_keyed_consumer_audit.md`.

## 80. REGISTRY-V2-TRANSLATION-01B-PURE-LOOKUP-LAYER-01 Closure Note (maintenance)

`CLOSED_LOCAL / MODEL-ONLY`. Adds `RegistryV2SymbolTranslationIndex` to
`core-rs/crates/mqk-md/src/instrument_registry_v2.rs`: a pure, fail-closed,
bidirectional lookup between `InstrumentRegistryV2` identity
(`instrument_id`/`symbol`) and the legacy symbol-string key `md_bars`/
outbox/portfolio-positions read. Proven collision-free across the full
88-row converted v1 equity universe, round-trip-correct for AAPL, metadata-
preserving for ETF symbols (SPY/TLT), fail-closed on duplicate/empty
identity fields, slash-symbol-preserving and non-tradable-flagged for the
BTC/USD crypto fixture, typed-miss (not panicking) for unknown lookups, and
deterministic in rebuild ordering — 11 new tests, 60/60 passing, clippy
clean. Not wired into any consumer; no production path reads this index.
Full detail: ledger entry for `REGISTRY-V2-TRANSLATION-01B-PURE-LOOKUP-LAYER-01`.

## 81. REGISTRY-V2-TRANSLATION-01C-VALIDATOR-CLI-AND-REPORT-01 Closure Note (maintenance)

`CLOSED_LOCAL / READ-ONLY-CLI`. Adds `mqk md registry-v2-translation-check`,
a read-only CLI proving `RegistryV2SymbolTranslationIndex` end-to-end
against the real 88-row converted v1 equity universe (100% round-tripped)
and, independently, the committed disabled BTC/USD+ETH/USD crypto v2
fixture. Fails closed (nonzero exit) on a duplicate converted-v1 symbol, a
duplicate v2 `instrument_id`, or an enabled non-equity trading flag — the
last case reported as `truth_state=unsafe_trading_enabled`, distinct from a
plain collision. 8 new subprocess-driven scenario tests, all passing;
clippy clean; no DB/network call; no consumer wired. Full detail: ledger
entry for `REGISTRY-V2-TRANSLATION-01C-VALIDATOR-CLI-AND-REPORT-01`.

## 82. REGISTRY-V2-TRANSLATION-01D-CLOSURE-AND-ROADMAP-RECONCILE-01 Closure Note (maintenance)

`CLOSED_LOCAL / DOCS-ONLY`. Decides `ASSET-CORE-01H` prerequisite #2 (an
explicit `instrument_id`/symbol translation layer) is now `CLOSED_LOCAL`
for its additive foundation scope, given `01A`'s audit, `01B`'s pure
translation index, and `01C`'s CLI/report proof. Confirms zero production
paths consume `InstrumentRegistryV2` (the translation index has exactly two
callers: its own unit tests and the read-only CLI). Updates the
production-cutover checklist: prerequisites #1-#2 now satisfied; #3 (Gate
0/routing-guard parity), #4 (live-network non-equity provider proof), and
#5 (explicit operator enablement) remain open.
`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` itself remains explicitly not
recommended. Docs-only — no code touched, no trading enabled, no DB/network
call made. Full detail: `docs/specs/registry_v2_translation_01d_closure_decision.md`.

## 83. REGISTRY-V2-GATE-PARITY-01A-CURRENT-GATE-AUDIT-01 Closure Note (maintenance)

`CLOSED_LOCAL / AUDIT-ONLY`. Audits Gate 0's current location/behavior
(`mqk-daemon/src/routes/strategy.rs::validate_strategy_signal`, `AS-01`
through `AS-12`) and the broker-submit routing guard's current
location/behavior (`mqk-execution/src/gateway.rs::enforce_gates`, `G01`
through `G08`) against direct repo evidence. Confirms neither gate reads
`InstrumentRegistryV2` today. Names both asset-class vocabularies
(`mqk_schemas::AssetClass`'s five variants vs. `CANONICAL_ASSET_CLASSES_V2`'s
six strings, the extra being `"rate"`), and defines the exact parity
contract `REGISTRY-V2-GATE-PARITY-01B`/`01C` must prove: equity allowed,
every other `CANONICAL_ASSET_CLASSES_V2` value rejected, empty/unknown/`etf`
fail closed. Decides plural aliases (`"futures"`/`"options"`) are **not**
accepted by the parity helper, matching `CANONICAL_ASSET_CLASSES_V2`'s own
strictness (`validate_registry_v2` already rejects them). Docs-only — no
code touched, no trading enabled, no DB/network call made. Full detail:
`docs/specs/registry_v2_gate_parity_01a_current_gate_audit.md`.

## 84. REGISTRY-V2-GATE-PARITY-01B/01C/01D Closure Note (maintenance)

`CLOSED_LOCAL / REGRESSION-TEST-ONLY`. Closes `ASSET-CORE-01H` prerequisite
#3. `01B` added a pure, fail-closed `registry_v2_gate_asset_class` helper
(`core-rs/crates/mqk-md/src/instrument_registry_v2.rs`) classifying any
`InstrumentRegistryV2.asset_class` string into the same allow-equity/
reject-non-equity/fail-closed-unknown decision Gate 0 and the broker-submit
routing guard already enforce; 12 unit tests cover all six
`CANONICAL_ASSET_CLASSES_V2` values, plural-alias rejection (`"futures"`/
`"options"` are deliberately not accepted), `"etf"` (instrument_kind, not
asset_class), and empty/unknown fail-closed cases. `01C` added 20
regression tests — `scenario_registry_v2_gate0_parity_01c.rs` (4 tests
against the live `POST /api/v1/strategy/signal` route) and
`scenario_registry_v2_routing_guard_parity_01c.rs` (8 tests against the
live `BrokerGateway::submit`, requiring a new `mqk-md` *dev*-dependency in
`mqk-execution`'s `Cargo.toml`) — proving the helper's decisions match both
gates' actual behavior exhaustively, including documenting that `"rate"`
(no `mqk_schemas::AssetClass` counterpart) can never even be constructed
as a `BrokerSubmitRequest`. Neither gate was modified; zero production
paths consume the helper. `01D` decides prerequisite #3 is `CLOSED_LOCAL`
and updates the production-cutover checklist: prerequisites #1-#3 now
satisfied; #4 (live-network non-equity provider proof) and #5 (explicit
operator enablement) remain open. Docs+tests-only in terms of behavior — no
runtime/broker/risk/OMS/config/DB/network change, no trading enabled. Full
detail: `docs/specs/registry_v2_gate_parity_01d_closure_decision.md`.

## 85. REGISTRY-V2-LIVE-PROVIDER-01A-CURRENT-NON-EQUITY-PROVIDER-AUDIT-01 Closure Note (maintenance)

`AUDIT-ONLY`. Audits every current non-equity provider/data proof candidate
against prerequisite #4 of `ASSET-CORE-01H`'s production-cutover checklist
(at least one non-equity market-data provider live-network-verified
end-to-end into `md_bars`). Finds Kraken (`config/providers/providers.json`'s
`kraken` entry, `core-rs/crates/mqk-md/src/providers/kraken.rs`) is the
safest and most complete first candidate: a real, tested
`HistoricalProvider` adapter with an existing DB-writing CLI command
(`mqk md kraken-ohlc-ingest`) that already writes to `md_bars` via the
canonical provider-ingest path, no credentials required, and a network
call already fail-closed behind `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` — but
that command has never been run in `network_smoke` mode against the live
endpoint. CoinLore is ticker-only and cannot structurally produce `md_bars`
rows; TwelveData/Alpaca have no implemented non-equity code path and would
require new credentials; Alpha Vantage/Polygon/yfinance have no adapter
code at all. Only `BTC/USD`/`ETH/USD` at `1D` are eligible (the only
symbols/timeframe Kraken's adapter and the registry-v2 fixture support).
Docs-only — no code touched, no network call, no DB access, no trading
enabled. Does not close prerequisite #4. Full detail:
`docs/specs/registry_v2_live_provider_01a_current_non_equity_provider_audit.md`.

## 86. REGISTRY-V2-LIVE-PROVIDER-01B/01C/01D Closure Note (maintenance)

`CLOSED_LOCAL / BOUNDARY-DECISION-ONLY`. Closes the *boundary decision* for
`ASSET-CORE-01H` prerequisite #4 — explicitly not prerequisite #4 itself,
which requires a live network call this and every prior session's hard
safety rules forbid. `01B` names Kraken/`BTC/USD`+`ETH/USD`/`1D` as the
selected first proof; `mqk md kraken-ohlc-ingest` (with
`MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`, no `--input-file`) as the only future
allowed command; an isolated proof/test database (never paper/live) as the
only allowed `MQK_DATABASE_URL` target; the exact verbatim operator
authorization phrase required before any future network call; the exact
proof fields required to close prerequisite #4
(`network_call_made=true`, `db_write=true`, `md_bars_write=true`,
`bars_completed>0`); the fields that must stay false (`kraken.enabled`,
both crypto fixture rows' enablement flags, no scheduler registration); and
leaves prerequisite #5 explicitly untouched. `01C` adds a purely local,
no-network, no-DB preflight guard
(`scripts/guards/validate_registry_v2_live_provider_01c_preflight.ps1`, 15
checks) proving `01B` contains every required element — including the
authorization phrase byte-for-byte — and that this bundle's own diff
touched no forbidden source/config file. `01D` decides the boundary
decision is `CLOSED_LOCAL` while prerequisite #4 itself remains `OPEN`, and
updates the production-cutover checklist accordingly. No network call, DB
access, or trading enablement occurred in any of `01B`-`01D`. Full detail:
`docs/specs/registry_v2_live_provider_01d_boundary_closure_decision.md`.

## 87. REGISTRY-V2-KRAKEN-LIVE-PROVIDER-PROOF-01 Closure Note (maintenance)

`CLOSED_LOCAL / LIVE-PROOF-COMPLETE`. Closes `ASSET-CORE-01H` prerequisite
#4. After the operator gave the exact authorization phrase required by
`REGISTRY-V2-LIVE-PROVIDER-01B`/`01D`, this patch ran the exact command
those docs named — `mqk-cli md kraken-ohlc-ingest --timeframe 1D`, once
each for `BTC/USD` and `ETH/USD`, with `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`
and `MQK_DATABASE_URL` pointed at the isolated local `mqk_test` database
(port 5434, confirmed distinct from the paper database at port 5440) — no
code changes, since the command and its network/DB gates already existed.
Both runs made exactly one live HTTP GET each to Kraken's public,
credential-free OHLC endpoint and wrote 720 real completed bars each to
`md_bars` via the unmodified provider-ingest write path, with truthful
`provider_id="kraken"` metadata, confirmed independently via direct `psql`
query. Post-proof checks confirmed the paper database has zero rows for
either symbol, `config/providers/providers.json` and the crypto
registry-v2 fixture are byte-identical to their pre-proof state (both
remain fully disabled), and no Kraken scheduled task exists. Evidence JSON
written to git-ignored `exports/live_provider_proof/`, not staged.
Prerequisite #4 is now `CLOSED`; prerequisite #5 (explicit operator
enablement) remains explicitly `OPEN` and was not inferred from this
proof's success. `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` remains
blocked on prerequisite #5 alone. Full detail:
`docs/specs/registry_v2_kraken_live_provider_proof_01_closure_decision.md`.

## 88. REGISTRY-V2-INSTRUMENT-ENABLEMENT-01-BTC-USD-DECISION-01 Closure Note (maintenance)

`CLOSED_LOCAL / DECISION-ONLY`. Addresses `ASSET-CORE-01H` §5 prerequisite
#5 after explicit operator authorization for a decision-only review naming
`BTC/USD`. Records the operator's decision (`BTC/USD` named as the first
non-equity instrument for eventual `enabled=true` status) without
implementing it. Confirms by direct source read that zero production
paths read `InstrumentRegistryV2` today, so the flag change (if made)
would affect no trading/execution/risk/broker/OMS behavior — only
read-only status/GUI surfaces. Surfaces a schema constraint:
`validate_registry_v2` fail-closed rejects the whole registry file if a
non-equity instrument has `enabled=true` without also setting the
test-only `allow_enabled_non_equity_for_testing` escape hatch, which the
schema's own bail message documents as carrying no production path. The
*decision* for prerequisite #5 now exists; the *implementation* (actual
config change) is deliberately deferred to a separate, explicit follow-up
authorization, per the operator's "stop after the enablement decision
evidence" instruction. `config/instruments/instruments_v2.crypto_local_marks.example.json`
remains byte-identical to its pre-review state — `BTC/USD.enabled`,
`paper_trading_enabled`, `live_trading_enabled` all still `false`. No
trading enabled, no orders routed, no broker/risk/runtime code touched.
Full detail:
`docs/specs/registry_v2_instrument_enablement_01_btc_usd_decision.md`.

## 89. ASSET-CORE-05-PER-INSTRUMENT-SESSION-ROUTING-01-COMBINED Closure Note (maintenance)

`CLOSED_LOCAL / MODEL-AND-PARITY-COMPLETE` for this bundle's own scope
(phases `05G`-`05J`; `ASSET-CORE-05` overall remains `PARTIAL /
PRODUCTION-CONSUMPTION-OPEN`). Audited the existing `ASSET-CORE-05A`-`05C`
session-profile surfaces and found a pure per-instrument router
(`resolve_session_profile_for_instrument_metadata`) and classifier
(`instrument_session_state_for_profile`) already existed, composed
inline at each route call site; the one honest gap was that `"rate"` (a
schema-valid `InstrumentRegistryV2` asset class) silently fell through
to the generic `Unknown`/"unrecognized asset_class" catch-all alongside
truly garbage strings. Closed that gap, relocated the duplicated
classifier into one canonical `pub fn`, and added a new composed
`route_instrument_session_for_metadata` pure helper — none of it called
by any route, trading, admission, risk, broker, or runtime path. Added
11 focused tests proving every canonical v2 asset class routes
correctly, equity/ETF matches real production `NyseWeekdaysProvider`
truth at four fixed timestamps, and non-equity always reports
model-only/never-production-matching. All pre-existing sibling
regression suites (`ASSET-CORE-05B`/`05C` route tests, `scenario_asset_class_scope_b8`,
GUI/daemon contract gate) pass unchanged. Production trading/admission
behavior, config flags, and non-equity enablement are all unchanged.

**Full detail, exact test names, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `ASSET-CORE-05G`/`05H`/`05I`/`05J`
entries; closure decision:
`docs/specs/asset_core_05j_session_routing_closure_decision.md`.

## 90. ASSET-CORE-02-03-COMPLETION-SWEEP-01-COMBINED Closure Note (maintenance)

Re-audited `ASSET-CORE-02` and `ASSET-CORE-03` (§36/§37 above already
recorded their respective foundation patches) and found the prompt's assumed
starting state was half-stale: `OrderIntentV2`/`ExecutionIntentV2` were
already hardened and validated, and — contrary to this doc's own §141 note —
a graduated per-asset-class static policy model
(`mqk_execution::asset_risk_policy`) already existed. The two genuinely
closable gaps were: (1) `ASSET-CORE-02` had zero bracket/OCO representation
anywhere; (2) `ASSET-CORE-03`'s policy model was not operator-surfaced by any
daemon route. Closed both: `BracketLegs` models a parent take-profit/
stop-loss pair on `OrderIntentV2`, validated and always reported as
`DisabledAssetClass`/`bracket_oco_model_only_not_executable` regardless of
asset class (no execution path anywhere supports multi-leg submission);
`GET /api/v1/system/asset-risk-policy` exposes the existing policy table
read-only, requiring no DB/broker/provider. `ASSET-CORE-02` is
`CLOSED_LOCAL / MODEL-COMPLETE` and `ASSET-CORE-03` is `CLOSED_LOCAL /
POLICY-BOUNDARY-COMPLETE`, both for their own stated scope only — zero
production callers were added, Gate 0 and the broker-submit routing guard
are unchanged and regression-proven, and no non-equity trading was enabled.
A combined `ASSET-CORE-02-03` daemon status route (this bundle's conditional
Phase D) was deliberately skipped: the one candidate route would have had to
hardcode roadmap-label strings (e.g. "MODEL-COMPLETE") into daemon code,
which is a stale-snapshot risk rather than a live-truth surface, and the
one genuine operator-facing need (policy visibility) was already served by
the `ASSET-CORE-03B` route above. True graduated live risk enforcement
(margin/multiplier/NAV actually blocking orders) still requires
`ASSET-CORE-04` live accounting, not yet wired into any enforcement path.

**Full detail, exact test names, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`ASSET-CORE-02-03A-CURRENT-COMPLETION-AUDIT-01`,
`ASSET-CORE-02B-ORDER-INTENT-V2-SAFE-GAP-CLOSURE-01`, and
`ASSET-CORE-03B-ASSET-RISK-POLICY-SAFE-GAP-CLOSURE-01` entries; closure
decision: `docs/specs/asset_core_02_03e_closure_decision.md`.

## 91. MARKET-HOURS-PROOF-SWEEP-01 Closure Note (maintenance)

A real NYSE regular-session market-hours sweep (2026-07-09) closed two
previously-observation-blocked items via live, non-fixture-timestamp
wall-clock proof. Lane 1: `AUTON-NO-TRADE-02` (and parent
`AUTON-NO-TRADE-01`) — the autonomous paper daemon's own session
controller started a real run under its normal default (legacy) session
source, and the strategy engine genuinely evaluated a no-signal condition
(`flat_below_threshold`, move_bps=-19 vs threshold_bps=20) against live
Alpaca-synced bars; zero orders were attempted, DB-and-route-confirmed.
Both `AUTON-NO-TRADE-02` and parent `AUTON-NO-TRADE-01` are now
`CLOSED_LOCAL`. Lane 2 (non-interleaved, separate daemon process, later
in the same session): `ASSET-CORE-05K` — confirmed
`MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` (temporary, process-scoped,
never persisted) produces `candidate_v2_parity_state=matched` against
legacy at real market-open wall-clock time. `ASSET-CORE-05K` is
`CLOSED_LOCAL / WALL-CLOCK-PARITY-ONLY`; parent `ASSET-CORE-05` remains
`PARTIAL / PRODUCTION-CONSUMPTION-OPEN` — this proof is wall-clock parity
only, not a production cutover, and does not change the §143 row's ~38%
estimate.

**Full detail, exact evidence, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s `AUTON-NO-TRADE-02`/`02B`/`02C`
and `ASSET-CORE-05K-V2-EQUITY-ACTIVE-MARKET-HOURS-PROOF-01` entries;
closure decisions: `docs/specs/auton_no_trade_02c_market_hours_closure_decision.md`,
`docs/specs/asset_core_05k_v2_equity_active_market_hours_proof.md`,
`docs/specs/market_hours_proof_sweep_01e_closure_decision.md`.

## 92. ASSET-CORE-04-LIVE-LEDGER-SECTION-CLOSURE-01-COMBINED Closure Note (maintenance)

`ASSET-CORE-04-LIVE-LEDGER-SECTION-CLOSURE-01-COMBINED` audited and
deepened the evidence behind row 142's existing `PARTIAL` / dual-axis
verdict — it did not move the percentage or status. Phase A
(`ASSET-CORE-04A-CURRENT-LIVE-LEDGER-BOUNDARY-AUDIT-01`) re-derived the
live-accounting/economics-scaffold boundary from a workspace-wide
caller-map grep (not from trusting the `04A`-`04F` modules' own doc
comments), confirming zero production callers of the economics scaffold
anywhere in `mqk-execution`, `mqk-risk`, `mqk-runtime`, or the GUI, and
that `OrderIntentV2`'s fractional-capable `QtyMicros` type is never
referenced by the broker-submit gateway. Phase B
(`ASSET-CORE-04B-LIVE-ACCOUNTING-INVARIANT-PROOF-01`) added the one
cross-module numeric proof that did not already exist: a new test file
(`scenario_asset_core_04_live_ledger_invariants.rs`) drives real fills
through the live FIFO ledger and independently recomputes the same
position through the economics scaffold, asserting exact agreement —
closing the gap between the scaffold's doc-comment claim of equivalence
and an actual assertion of it, without modifying any production file.
Phase C and Phase D were both skipped as duplicative: the honesty fields
Phase C would have added already exist on `PortfolioEconomicsStatusResponse`
(`04D`/`04F`), and the readiness classifier Phase D would have added
already exists as `GET /api/v1/system/asset-risk-policy/status`
(`ASSET-CORE-03B`) — see the closure decision doc for the full reasoning.

**Not resolved (row 142's `PARTIAL` verdict stands unchanged):** live
accounting is still whole-unit/single-currency/multiplier-naive/no-margin;
the economics scaffold still has zero production callers; wiring either
into live accounting, risk enforcement, or order routing remains a
distinct, production-behavior-changing patch, explicitly not started here.

**Full detail, exact evidence, and validation commands:**
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`ASSET-CORE-04A-CURRENT-LIVE-LEDGER-BOUNDARY-AUDIT-01` and
`ASSET-CORE-04B-LIVE-ACCOUNTING-INVARIANT-PROOF-01` entries; closure
decision: `docs/specs/asset_core_04_live_ledger_closure_decision.md`;
boundary audit: `docs/specs/asset_core_04_live_ledger_boundary_audit.md`.
