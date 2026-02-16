# Testing Strategy (V4)

Goal: high confidence with low bloat.

We avoid “test sprawl” by proving **invariants** with a small number of high‑leverage tests.
Target total: **~80–130 tests** for Phase 1.

---

## 1) Test Philosophy

Trading systems fail in a few predictable ways:
- duplicate orders / non-idempotent execution
- ledger drift (PnL/positions wrong)
- lookahead/leakage (fake performance)
- reconciliation desync (silent divergence)
- missing protective stops
- kill switches not triggering
- nondeterministic backtests/replays
- config/secrets leaks

So we test **these invariants**, not implementation details.

Rule:
> If a test cannot be named after an invariant, don’t add it.

---

## 2) The “Diamond” Test Shape (not a pyramid)

### A) Unit tests (20–40)
Only **pure deterministic functions**:
- config canonicalization + hashing
- FIFO lot math / PnL math
- drawdown math
- ambiguity resolution helper
- idempotency key generation
- client_order_id prefix rules

No DB, no network, no wall-clock time.

### B) Contract tests (20–40)
Prove interface contracts across implementations (table-driven):
- BrokerAdapter contract: SimBroker vs AlpacaStub
- ExecutionModel contract: default + stress overlays
- Strategy contract: deterministic, no lookahead
- Event envelope completeness
- Outbox/inbox idempotency

### C) Golden scenario tests (10–20)
End-to-end “slices” asserting:
- orders
- fills
- ledger/equity curve
- audit hashchain
- deterministic replay

Each scenario covers multiple subsystems.

### D) Failure-injection tests (10–20)
Break things on purpose:
- drop broker ACKs
- duplicate fills
- reorder events
- simulate broker/account desync
- stale data streams
- DB restart mid-run
- corrupt one audit line → hashchain fails

---

## 3) Naming and Organization

### Naming
Use prefixes:
- `unit_*`
- `contract_*`
- `scenario_*`
- `invariant_*`
- `fault_*`

Examples:
- `invariant_no_duplicate_orders_on_restart`
- `invariant_protective_stop_must_exist`
- `scenario_gap_through_stop_fills_at_open`
- `fault_reject_storm_disarms_and_halts_new`

### Folder structure (recommended)
- `tests/unit/`
- `tests/contract/`
- `tests/scenario/`
- `tests/fault/`
- `tests/fixtures/` (shared data)

---

## 4) Table-driven Tests (how we prevent 700 tests)

Prefer one test file with a case table.

Examples:
- `contract_execution_model_cases.rs`
  - MARKET next-open
  - LIMIT touch
  - STOP touch
  - GAP through STOP
  - same-bar ambiguity
- `fault_kill_switch_cases.rs`
  - stale data
  - reject storm
  - desync
  - drawdown
  - missing stop

One test file, many cases, minimal boilerplate.

---

## 5) Fixtures Policy

Keep a small canonical fixture set (5–10 datasets) and reuse:
- `bars_trending.csv`
- `bars_choppy.csv`
- `bars_gap_down.csv`
- `bars_outlier.csv`
- `broker_snapshot_clean.json`
- `broker_snapshot_desync.json`
- `fills_duplicates.jsonl`

Scenarios should reuse fixtures instead of minting new ones.

---

## 6) Determinism and Golden Files

For parity backtests and replay:
- store “golden” expected artifacts for a small number of scenarios:
  - `expected_orders.csv`
  - `expected_fills.csv`
  - `expected_equity_curve.csv`
  - `expected_metrics.json`

Golden tests must compare:
- exact rows (stable ids)
- exact metrics within tolerances if floating point

If a change updates goldens:
- require explicit “golden update” commit message
- attach rationale in PR/commit notes

---

## 7) Minimal Test Set by Patch (budget)

### PATCH 01–04 (spine: DB, config, events, outbox/inbox)
~15–25 tests total:
- migrations apply clean
- config merge precedence
- config hash stable
- secrets excluded
- event envelope required fields
- outbox idempotency unique constraint
- inbox dedupe unique constraint

### PATCH 05–07 (OMS/ledger/data)
~25–40 tests total:
- OMS idempotency + cancel/replace lineage
- FIFO lot math, partial closures
- equity = cash + Σ unrealized invariant
- no-lookahead enforcement
- gap/outlier/stale gate behaviors

### PATCH 08–12 (parity backtest + execution + stops + reconcile + arming)
~25–45 tests total:
- 8–12 golden scenarios
- 6–10 fault injections
- replay determinism for at least 2 scenarios

### PATCH 13–15 (replay, promotion, dual engine)
~10–20 tests total:
- replay parity
- promotion gate enforcement
- engine namespace isolation
- reconciliation engine scoping

Target total: **~80–130**.

---

## 8) What NOT to Test

Avoid:
- “does function X call function Y”
- mocking-heavy tests that prove only wiring
- one test per trivial getter/setter
- tests that duplicate coverage already provided by scenario tests

If you need to mock: ask if the layer should be a contract test instead.

---

## 9) CI Strategy (lean)

CI should run fast:
- default: unit + contract + a small scenario subset
- nightly/optional: full scenario + stress sweeps

Promotion pipeline requires:
- full scenario suite
- determinism checks
- stress profiles

---

## 10) Definition of “Done” for a Patch

A patch is done when:
- acceptance criteria satisfied
- relevant invariants covered by at least one test (prefer scenario/contract)
- no new redundant tests added

---

## 11) Rule to Prevent Test Sprawl

Any new test must answer:
1) Which invariant does this prove?
2) Why isn’t it already proven by an existing scenario/contract test?
3) Can it be folded into a table-driven test case instead?

If it can be folded into an existing test, do that.
