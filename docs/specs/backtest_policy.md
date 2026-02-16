# Backtest Policy (V4)

Three methods:
1) Parity Backtester (event-driven, OMS/ledger parity) — **promotion source of truth**
2) Vectorized Research Backtester — fast iteration, non-promotable
3) Replay/Parity tools — verify live vs deterministic replay

## 1) Parity Backtester (promotion-bound)
Must reuse:
- strategy interface (intents)
- OMS semantics
- SimBroker execution model
- fill-driven ledger
- audit envelope

Rules:
- chronological streaming (no full-data access in strategy context)
- conservative ambiguity for promotion
- export artifacts (manifest, audit, orders, fills, equity curve, metrics)

Promotion requires:
- no lookahead violations
- data integrity gates
- stress profiles (>= slippage_x2)
- robustness (walk-forward + purged CV)

## 2) Vectorized Research Backtester
Purpose: rapid sweeps and iteration.
Allowed approximations: simplified fills/costs.
Any promotable candidate must pass parity backtest.

## 3) Lookahead/leakage
Accessing data with ts_close_utc > runtime clock is a hard failure.

## 4) Corp actions & survivorship
v0 default: RAW series + fixed watchlist allowed but must declare limitation.
Broader universes require survivorship membership datasets.

## 5) Determinism
Same data snapshot + config hash + seed + git hash => identical parity output.
