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
| `ASSET-CORE-02` Multi-Asset Order Intent Model | PARTIAL | 25% | L | `ASSET-CORE-01` | `OrderIntentV2`/`ExecutionIntentV2` exist, explicitly unwired (`RESEARCH-NON-EQ-01`); no bracket/OCO anywhere | 7 | Wiring this is itself a scope-reviewed patch per its own code comment. |
| `ASSET-CORE-03` Asset-Aware Risk Router | PARTIAL | 35% | L | `ASSET-CORE-01`, `ASSET-CORE-04` | Two tested fail-closed gates exist (Gate 0, routing guard); zero graduated per-class policy behind them | 8 | Most mature of the five — finish what's started. |
| `ASSET-CORE-04` Multi-Asset Portfolio Ledger | MISSING (effectively) | 20% | XL | none | `cash_micros` single-currency; FIFO P&L multiplier-naive; no margin/NAV-by-asset-class | 14 | Highest-risk patch in this entire roadmap — touches live capital accounting invariants. Needs its own scenario-test proof standard per `audit_repo_truth_rules.md`. |
| `ASSET-CORE-05` Market Calendar & Session Provider | PARTIAL | 30% | M | none | `MarketCalendarProvider` trait + 3 providers exist and are fail-closed/pluggable; `MarketSessionState` enum is NYSE-vocabulary-coupled (no 24/7, no Globex RTH/ETH, no FX session windows) | 5 | Cheaper than it looks — the hard part (trait, fail-closed contract, fallback) is already built. |

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
| `CRYPTO-REGISTRY-01` Crypto asset registry | MISSING | 5% | S | `ASSET-CORE-01` | `AssetClass::Crypto` + `ContractSpec::Crypto` exist; zero pairs registered | 15 | Cheapest non-equity asset class to stand up — spot only, no margin, no expiry. |
| `CRYPTO-DATA-01` 24/7 market ingestion | MISSING | 10% | M | `CRYPTO-REGISTRY-01`, `ASSET-CORE-05`, `PROVIDER-SWAP-CONTRACT-01` | `providers.json` already *declares* TwelveData crypto capability (unverified, unimplemented) | 16 | |
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
| `BACKTEST-MULTIPLIER-MARGIN-01` | MISSING | 0% | L | `ASSET-CORE-01` | `mqk-portfolio/src/accounting.rs` fill math is always `qty * price_delta`; backtest engine never reads `ContractSpec` multiplier fields | 6 | Hard prerequisite — no futures/options backtest result can be trusted without this. |
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
