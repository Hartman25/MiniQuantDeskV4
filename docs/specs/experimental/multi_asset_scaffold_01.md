# MULTI-ASSET-SCAFFOLD-01 — Future Asset-Type Architecture Scaffold

**Status: BACKLOG / NOT EXECUTABLE**
**Lane: EXP (research only; not operational truth)**
**Activation gate: NONE YET — all asset classes disabled until explicit future promotion**

---

## Purpose

MULTI-ASSET-SCAFFOLD-01 defines the architectural seams and requirements for
extending MiniQuantDesk V4 beyond US equities into additional asset classes:
crypto, futures, options, and forex.

This document is a **planning and architecture reference only**.
No execution code for any of these asset classes exists or will be added until
each class passes its own promotion gate (defined below).
No routing changes are made by this document.

---

## Hard Boundaries

- All non-equity asset types are **disabled by default** and must remain so until
  explicitly promoted by a named future patch.
- Any future code for these asset classes must be gated behind a
  `MQK_ASSET_CLASS_<NAME>_ENABLED=true` flag that is absent in all default configs.
- No broker adapter changes may be merged that enable multi-asset routing without
  a corresponding promotion gate patch that is reviewed separately.
- The current paper+alpaca equity execution path must not be touched by any
  multi-asset scaffold work.
- No DB migrations for multi-asset until a specific asset class reaches Stage 3
  (paper execution wiring).

---

## Asset Capability Matrix

### Current live/paper-capable asset class

| Asset Class | Status | Broker Adapter | Paper-Ready | Live-Ready |
|-------------|--------|----------------|-------------|------------|
| **US Equities** | **ACTIVE** | Alpaca (paper+live) | Yes (AAPL/5m smoke) | No (pending live readiness review) |

### Future asset classes (all DISABLED / BACKLOG)

| Asset Class | Lane ID | Status | Broker Adapter | Paper-Ready | Live-Ready |
|-------------|---------|--------|----------------|-------------|------------|
| Crypto (spot) | CRYPTO-SCAFFOLD-01 | BACKLOG | Alpaca Crypto (not wired) | No | No |
| Futures | FUTURES-SCAFFOLD-01 | BACKLOG | TBD | No | No |
| Options | OPTIONS-SCAFFOLD-01 | BACKLOG | TBD | No | No |
| Forex | FOREX-SCAFFOLD-01 | BACKLOG | TBD | No | No |

---

## Per-Asset-Class Requirements

### US Equities (current; reference)

- **Instrument identity:** Ticker + exchange (e.g. `AAPL:NASDAQ`)
- **Session model:** NYSE/NASDAQ regular hours; `ExchangeSourcedCalendarProvider` seam exists
- **Order types:** Market, limit, stop-limit (Alpaca v2)
- **Position model:** Long equity shares; no leverage by default
- **Margin model:** Cash account (no margin by default)
- **Fees/slippage model:** Alpaca zero-commission; slippage from fill_quality_telemetry
- **Data requirements:** OHLCV bars (1D / 5m / configurable); via TwelveData + CSV backup
- **Broker adapter:** `mqk-broker-alpaca`; WS + REST inbound lanes; fully wired
- **Risk gates:** capital_allocation_policy; max_daily_loss; reconcile clean; halt gate
- **Reconcile requirements:** fill_quality_telemetry; drift detection; 180s grace window
- **Promotion gate status:** Paper smoke proven; live pending separate review

---

### Crypto (Spot) — CRYPTO-SCAFFOLD-01

**Status: DISABLED / BACKLOG**

- **Instrument identity:** Base/quote pair + exchange (e.g. `BTC/USD:Coinbase`)
- **Session model:** 24/7; no exchange calendar required; funding-rate windows relevant
- **Order types:** Market, limit, stop-limit; taker/maker fee distinction important
- **Position model:** Long crypto position in base currency units; fractional shares
- **Margin model:** Spot only for initial scope; perpetual futures are separate lane
- **Fees/slippage model:** Taker fee typically 0.1–0.5%; slippage significant at large size
- **Data requirements:** OHLCV tick/bar data; WebSocket L2 order book for spread monitoring
- **Broker adapter requirements:**
  - New adapter or Alpaca Crypto API extension (not wired)
  - WS and REST inbound lanes needed (same contract as `mqk-broker-alpaca`)
  - Requires separate credential set (`ALPACA_CRYPTO_*` or equivalent)
- **Risk gates required before paper:**
  - 24/7 session requires different deadman/watchdog model
  - Spread typically wider than equities; requires tighter spread gate
  - Custody/exchange risk not present in equities (counterparty risk model needed)
  - No `ExchangeSourcedCalendarProvider` needed but need session window re-definition
- **Reconcile requirements:** Same durable fill/drift model; funding-rate adjustment not in scope for v1
- **Promotion gate:**
  1. Adapter unit tests (pure/in-process)
  2. Paper execution on Alpaca Crypto sandbox (if available)
  3. 30-day paper run with evidence review
  4. Operator sign-off
- **Future patch lane:** CRYPTO-SCAFFOLD-01 → CRYPTO-PAPER-01 → CRYPTO-LIVE-01

---

### Futures — FUTURES-SCAFFOLD-01

**Status: DISABLED / BACKLOG**

- **Instrument identity:** Root symbol + expiry + exchange (e.g. `ES:H25:CME`)
- **Session model:** CME Globex nearly 24/5; Sunday gap; multiple session types (RTH/ETH)
- **Order types:** Market, limit, stop, MOC, MOO; spread/combo orders out of scope for v1
- **Position model:** Notional contract-based; 1 ES contract = 50x index; signed P&L per tick
- **Margin model:** Initial + maintenance margin required; margin model must be wired before any execution
- **Fees/slippage model:** Per-contract exchange fees; NFA fee; slippage significant at market
- **Data requirements:** Continuous contract data or expiry-aware rollover; tick-level for intraday
- **Broker adapter requirements:**
  - New adapter required (Interactive Brokers, Tradovate, NinjaTrader, or similar)
  - No current adapter; significant development scope
  - Separate API credentials required
- **Risk gates required before paper:**
  - Margin model must be implemented and tested before any order routing
  - Roll-date handling required to avoid accidental position in expired contract
  - Overnight margin changes require risk re-evaluation at session boundary
  - P&L settlement model (daily mark-to-market) different from equity fill model
- **Reconcile requirements:** Daily MTM settlement; open contracts vs closed fills; distinct from equity model
- **Promotion gate:**
  1. Margin model specification and implementation
  2. Roll-date calendar and expiry-aware instrument identity
  3. Adapter unit tests
  4. Paper execution on futures simulator
  5. 60-day paper run with evidence review
  6. Operator sign-off + risk review
- **Future patch lane:** FUTURES-SCAFFOLD-01 → FUTURES-MARGIN-01 → FUTURES-PAPER-01

---

### Options — OPTIONS-SCAFFOLD-01

**Status: DISABLED / BACKLOG**

- **Instrument identity:** Underlying + expiry + strike + call/put (e.g. `AAPL:2025-01-17:185:C`)
- **Session model:** Equity options: market hours; Index options: slightly extended; weekly expirations
- **Order types:** Limit preferred (wide spreads make market orders high risk); multi-leg spread orders future scope
- **Position model:** Long/short calls and puts; defined-risk spreads future scope; assignment risk for short options
- **Margin model:** Defined-risk vs undefined-risk; RegT/PM margin; significant complexity; out of scope for v1 beyond documentation
- **Fees/slippage model:** Per-contract fee ($0.50–$0.65 typical); wide bid/ask on far-OTM; liquidity critical
- **Data requirements:** Options chain snapshots; Greeks (delta, gamma, theta, vega); IV surface; not available from current TwelveData + CSV pipeline without extension
- **Broker adapter requirements:**
  - Alpaca supports basic options (Phase 1 rollout); API extension needed
  - Options-specific order type (multi-leg) would require additional adapter work
  - Separate options data feed likely needed
- **Risk gates required before paper:**
  - Greeks model must be wired for risk sizing (delta-equivalent position sizing)
  - Expiration risk: must handle expiry events (exercise, assignment, worthless expiry)
  - Short option margin requirement must be enforced before any sell writes
  - IV crush risk not modeled in current reconcile framework
- **Reconcile requirements:** Contract-level reconcile; exercise/assignment events distinct from fill events; OCC settlement
- **Promotion gate:**
  1. Options chain data pipeline established
  2. Greeks model specification
  3. Expiry event handler
  4. Paper execution on Alpaca options sandbox
  5. 30-day paper run on long options only (defined risk)
  6. Separate review for short options (undefined risk)
  7. Operator sign-off
- **Future patch lane:** OPTIONS-SCAFFOLD-01 → OPTIONS-DATA-01 → OPTIONS-PAPER-LONG-01

---

### Forex — FOREX-SCAFFOLD-01

**Status: DISABLED / BACKLOG**

- **Instrument identity:** Currency pair (e.g. `EUR/USD`, `GBP/JPY`); standard 5-decimal precision
- **Session model:** Approximately 24/5 (open Sun 5pm ET, close Fri 5pm ET); major session windows (London, NY, Tokyo, Sydney)
- **Order types:** Market, limit, stop; pip-value-aware sizing
- **Position model:** Notional currency amount; P&L in quote currency then converted to USD; pip P&L model distinct from equity share model
- **Margin model:** Retail leverage (50:1 CFTC max for majors in US; lower for minors); significant leverage risk
- **Fees/slippage model:** Spread-based (no commission); raw spread + markup; varies by liquidity provider and session
- **Data requirements:** Tick data or 1-minute OHLC; session-aware bar construction; rollover (swap) rates for overnight positions
- **Broker adapter requirements:**
  - No current adapter; OANDA, Interactive Brokers Forex, or similar
  - FIX protocol or REST; WebSocket tick feed
  - Separate API credentials; US forex brokers require NFA registration for the firm
- **Risk gates required before paper:**
  - Leverage model must be implemented and respected before any order routing
  - Rollover/swap must be accounted for overnight positions
  - Spread-based cost model must be reflected in fill quality telemetry
  - FX hedging/netting rules (US: FIFO-only) must be implemented
- **Reconcile requirements:** Realized P&L in USD (converted from quote currency); rollover credit/debit separate line items
- **Promotion gate:**
  1. FX pip/lot sizing model
  2. Leverage model specification and implementation
  3. Rollover model
  4. Adapter unit tests
  5. Paper execution on broker simulator
  6. 30-day paper run with evidence review
  7. Operator sign-off + regulatory review (US NFA compliance)
- **Future patch lane:** FOREX-SCAFFOLD-01 → FOREX-LEVERAGE-01 → FOREX-PAPER-01

---

## Cross-Cutting Architecture Requirements

These requirements apply before any non-equity asset class can be wired for execution.

### MULTI-ASSET-ROUTING-GUARD-01

**Status: SHIPPED** (`ff2ae59`; broker-submit asset-class reject gate, `GateRefusal::AssetClassDisabled`, 8 tests). See `docs/audits/multi_asset_completion_audit.md` §2/§5.

A mandatory code gate that must exist before any multi-asset routing lands:

- A static test that proves unsupported asset classes are rejected at the
  order dispatch boundary.
- The gate must enumerate all currently-disabled asset classes and assert each
  produces a hard rejection (not a silent pass-through).
- Example: a unit test that passes `InstrumentType::Crypto` to the broker
  dispatch function and asserts it returns `Err(AssetClassDisabled)`.
- This guard must be in place before any adapter code for non-equity assets
  is merged.

### DISABLED-ASSET-GATE-TESTS-01

**Status: SHIPPED** (`6fe1697`; outbox-payload-level rejection of disabled asset classes). See `docs/audits/multi_asset_completion_audit.md` §2/§5.

- A test file (e.g. `scenario_disabled_asset_gates.rs`) that statically proves:
  - Crypto orders are rejected
  - Futures orders are rejected
  - Options orders are rejected
  - Forex orders are rejected
  - Only `InstrumentType::UsEquity` (or equivalent) passes through to the
    current Alpaca equity adapter

### ASSET-CAPABILITY-MATRIX-01

**Status: SHIPPED** (`424f0de`; `GET /api/v1/system/metadata`, backend-complete and contract-gated; not yet GUI-rendered). See `docs/audits/multi_asset_completion_audit.md` §2/§5.

A live machine-readable capability matrix (JSON or TOML) that records:

- Per-asset-class: `enabled`, `paper_ready`, `live_ready`, `broker_adapter`
- This matrix is read at daemon startup and surfaces `capability_matrix` in
  `/api/v1/system/metadata`
- Any request to route an order for a disabled asset class is blocked at the
  gate and logged as an invariant violation

---

## What Must NOT Change Until Promotion Gates Are Met

| Invariant | What is protected |
|-----------|-------------------|
| Current OMS outbox/inbox tables | No multi-asset columns added until an asset class reaches Stage 3 |
| Current broker adapter (`mqk-broker-alpaca`) | No crypto/futures/options/forex routing code added |
| Current `intraday_scalper` | No changes to strategy logic |
| Current reconcile model | No multi-asset fill types added |
| Current capital policy | No multi-asset budget entries added |
| Default config files | No `ASSET_CLASS_*_ENABLED=true` entries ever appear by default |
| GUI readiness claims | No GUI panels claim multi-asset support until fully wired and tested |

---

## Future Patch Lane IDs (status)

**Maintenance note (`LEDGER-MULTI-ASSET-RECONCILE-01`):** this table's heading was originally "not yet created" — three lane IDs below have since shipped (status column added). Active multi-asset roadmap tracking now lives in `docs/audits/multi_asset_completion_audit.md` and `MiniQuantDesk_Master_Patch_Ledger_v2.md` §19; this document remains the per-asset-class architecture/requirements reference.

| Lane ID | Description | Prerequisite | Status |
|---------|-------------|--------------|--------|
| ASSET-CAPABILITY-MATRIX-01 | Machine-readable matrix in daemon metadata | This document | SHIPPED (`424f0de`) |
| MULTI-ASSET-ROUTING-GUARD-01 | Static reject gate for disabled asset classes | Before any non-equity adapter code | SHIPPED (`ff2ae59`) |
| DISABLED-ASSET-GATE-TESTS-01 | Test coverage for all disabled rejections | MULTI-ASSET-ROUTING-GUARD-01 | SHIPPED (`6fe1697`) |
| CRYPTO-SCAFFOLD-01 | Crypto spot instrument model + adapter spec | ASSET-CAPABILITY-MATRIX-01 | BACKLOG |
| FUTURES-SCAFFOLD-01 | Futures margin model + roll-date spec | ASSET-CAPABILITY-MATRIX-01 | BACKLOG |
| OPTIONS-SCAFFOLD-01 | Options chain data + Greeks model spec | ASSET-CAPABILITY-MATRIX-01 | BACKLOG |
| FOREX-SCAFFOLD-01 | FX pip/lot/leverage model spec | ASSET-CAPABILITY-MATRIX-01 | BACKLOG |
| CRYPTO-PAPER-01 | Paper execution wiring for crypto | CRYPTO-SCAFFOLD-01 + 30d evidence | BACKLOG |
| FUTURES-PAPER-01 | Paper execution wiring for futures | FUTURES-SCAFFOLD-01 + margin model | BACKLOG |
| OPTIONS-PAPER-LONG-01 | Paper execution for long options | OPTIONS-SCAFFOLD-01 + chain data | BACKLOG |
| FOREX-PAPER-01 | Paper execution for forex | FOREX-SCAFFOLD-01 + leverage model | BACKLOG |

---

## Explicit Non-Goals

- Does not change any currently-active trading path.
- Does not add broker adapters for non-equity assets.
- Does not add API keys or credential requirements.
- Does not mutate DB schema.
- Does not create GUI panels for multi-asset.
- Does not imply any delivery timeline.
- Does not scope or commit to any broker choice for futures/options/forex.
