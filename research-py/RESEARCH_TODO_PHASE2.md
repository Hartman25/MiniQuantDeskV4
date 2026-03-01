# Research Phase 2 TODO (Deferred Work)

This file tracks the pieces we are intentionally NOT implementing yet.
Goal: keep research modular and only emit artifacts that backtest/execution can consume.

## Guiding rule
Research produces **inputs** (artifacts + manifests).  
Backtest/execution consumes them.  
No backtest/execution code is imported or invoked from research.

---

## A) Options (schema + adapters + pipeline)

### A1. Postgres schema (options)
- options_underlyings
- options_contracts (root, expiration, strike, right, multiplier, OCC sym)
- options_chain_snapshots (asof_utc, underlying, dte window, strike window)
- options_quotes (bid/ask/last, sizes, ts_utc)
- options_ohlcv (if needed)
- options_greeks / implied_vol (ts_utc, model, inputs snapshot id)

### A2. Data adapters (research-py)
- options_postgres.history_chain(...)
- options_postgres.quotes(...)
- strict schema detection (no guessing units)

### A3. Research pipeline stages (options)
- universe builder for options
- feature builder for options (IV rank, skew, term structure, liquidity)
- target builder (contracts + weights + risk caps)
- deterministic “contract_id” canonicalization

### A4. Artifacts (options)
- intent.json (already stubbed)
- options_universe.csv
- options_features.csv
- options_targets.csv
- manifest.json references all of the above

---

## B) Futures (schema + adapters + pipeline)

### B1. Postgres schema (futures)
- futures_contracts (symbol root, exchange, multiplier, tick size)
- futures_roll_calendar / roll_rules
- futures_continuous_series (front month, back-adjusted, etc.)
- futures_ohlcv

### B2. Data adapters (research-py)
- futures_postgres.history(...)
- futures_postgres.continuous_series(...)

### B3. Research pipeline stages (futures)
- universe builder (contract selection + roll behavior)
- features (term structure, carry proxy, volatility, trend)
- targets

---

## C) Execution/backtest integration (artifact contracts only)

### C1. Shared “artifact contract”
Define a stable file contract consumed by BOTH backtest and execution:
- run_id (stable)
- asof_utc
- policy_name
- asset_class
- instruments / targets
- constraints / risk metadata

### C2. Canonical IDs (must be stable forever)
- EQUITY::SPY
- OPTION::<root>::<YYYYMMDD>::<C/P>::<strike_micros>
- FUTURE::<root>::<YYYYMM>::<exchange>

### C3. Data co-location plan (filesystem)
Decide a single “data root” layout (see next patches):
- data/
  - market/        # md_bars and other market data imports
  - research/      # research run outputs + manifests
  - backtest/      # backtest outputs
  - execution/     # paper/live trade logs + fills + broker snapshots
  - shared/        # cross-module contracts, schemas, IDs

---

## D) Risk & gates (research-side only)
- Minimum bars gate is implemented for equities; extend per asset class.
- Add explicit “data freshness” gates per asset class.
- Add “policy schema validation” per policy version.

---

## E) Packaging / CLI
- Add `mqk-research validate-policy --policy ...`
- Add `mqk-research explain-run --run-dir ...`
- Add `mqk-research list-runs --out runs/`