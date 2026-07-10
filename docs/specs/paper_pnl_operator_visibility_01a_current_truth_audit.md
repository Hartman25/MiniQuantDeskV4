# PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01A — Current Truth Audit

Patch group: `PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED`.
Scope: read-only audit of the null-P&L gap exposed by
`PAPER-TRADE-LIFECYCLE-PROOF-02-FAST-MARKET-HOURS-RETRY-COMBINED`. No source
files are edited in this phase. No provider/broker/network call. No DB
connection.

## 1. Proof-02 live result summary

`docs/specs/paper_trade_lifecycle_proof_02_fast_market_hours_retry.md`
recorded a real, naturally-generated autonomous paper trade:

- `AAPL` buy signal generated naturally (§4).
- Paper order submitted naturally, no forced/manual submit call (§10, §17).
- Broker ack (x2) and fill occurred via `oms_inbox` (§11).
- `GET /api/v1/portfolio/positions` returned
  `{"symbol":"AAPL","qty":3,"avg_price":314.81,"broker_qty":3,"drift":null}`
  — i.e. `AAPL qty=3 avg_price=314.81` (§12).
- `GET /api/v1/portfolio/summary` returned `cash=999111.38`,
  `long_market_value=944.43`, `account_equity=1000057.12` (§12).
- `live_routing_enabled=false` at every observed point (§15). No live orders
  (§16). No forced paper orders (§17). No strategy threshold/gate/config
  changes (§18).

## 2. Exact P&L null gap

Per proof-02 §13 (the one lifecycle gap this run exposed):

- `portfolio/positions.mark_price = null`
- `portfolio/positions.unrealized_pnl = null`
- `portfolio/summary.daily_pnl = null`

Position and cash/equity figures updated correctly; no mark-to-market or P&L
value is surfaced on either primary operator route for this fill.

## 3. Current route/source findings

**Q1/Q2 — which routes return these surfaces:**

- `GET /api/v1/portfolio/positions` → `portfolio_positions()` in
  [core-rs/crates/mqk-daemon/src/routes/portfolio.rs:72](../../core-rs/crates/mqk-daemon/src/routes/portfolio.rs)
  → `PortfolioPositionsResponse` / `PortfolioPositionRow` in
  [core-rs/crates/mqk-daemon/src/api_types/portfolio_snapshot.rs](../../core-rs/crates/mqk-daemon/src/api_types/portfolio_snapshot.rs).
- `GET /api/v1/portfolio/summary` → `portfolio_summary()` in
  `routes/portfolio.rs:34` → `PortfolioSummaryResponse` in
  `core-rs/crates/mqk-daemon/src/api_types.rs:940`.

Both routes are sourced from `st.broker_snapshot`
(`mqk_schemas::BrokerSnapshot`) — the broker-account-derived, in-memory-only
snapshot (`snapshot_source` = `"synthetic"` in paper mode, `"external"` for
Alpaca REST). This is a **different** data source than
`/api/v1/portfolio/live-weights` and `/api/v1/portfolio/economics/status`,
which read `st.execution_snapshot` (`mqk_runtime::observability::ExecutionSnapshot`,
the runtime's own ledger-derived snapshot).

**Q3 — where are the null fields set:**

- `routes/portfolio.rs:102-103`, inside `portfolio_positions()`'s per-row
  map: `mark_price: None, unrealized_pnl: None` — set unconditionally, never
  computed from any input.
- `routes/portfolio.rs:49`, inside `portfolio_summary()`: `daily_pnl: None`
  — set unconditionally.
- The API-type doc comments in `portfolio_snapshot.rs:16-22` document this
  as deliberate: *"mark-to-market data is not present in the broker
  snapshot"* — true of `mqk_schemas::BrokerSnapshot` itself, but the daemon
  already has a separate mark source available (next section) that this
  route never consults.

**Q4/Q5 — existing mark-source pattern from md_bars:**

Yes. `portfolio_live_weights()` (`routes/portfolio.rs:458-560`) and
`portfolio_economics_status()` (`routes/portfolio.rs:1036-1294`) both already
resolve a mark per non-flat symbol via
`mqk_db::fetch_recent_completed_bars_for_strategy(pool, symbol, &timeframe, 1)`,
taking `bars.last()` (the latest *completed* bar) and building a
`mqk_portfolio::PositionMark { mark_price_micros: bar.close_micros,
mark_ts_utc: Some(bar.end_ts), source: format!("md_bars:{timeframe}:close")
}`. Default `timeframe` is `"1D"` (`DEFAULT_LIVE_WEIGHTS_TIMEFRAME`). Missing
bars never fabricate a price — they surface `missing_mark = true` /
`missing_marks` truth state instead. This is the established, reusable mark
source.

**Q6 — existing pure valuation helper:**

`mqk_portfolio::compute_portfolio_weights` (`mqk-portfolio/src/valuation.rs`)
is pure and computes per-symbol `market_value_micros` (`signed_qty *
mark_price_micros`) and NAV/weight — but it has **no cost-basis concept at
all**. `PositionWeightInput` carries only `symbol` + `signed_qty`, no
`avg_price`. It cannot produce unrealized P&L (which requires `mark -
avg_price`, not just `mark * qty`).

`mqk_portfolio::accounting.rs` computes **realized** P&L via FIFO lot
matching (`buy_fifo`/`sell_fifo`, `realized_pnl_micros`) — a different
computation (crystallized on fill, not mark-to-market) and not reusable for
unrealized P&L either.

**Conclusion: no existing pure helper computes unrealized P&L from
`(signed_qty, avg_price, mark_price)`. Phase B adds one.**

**Q7 — is daily P&L computable without fabricating a baseline:**

No. Grepped `core-rs/crates/mqk-db`, `core-rs/crates/mqk-daemon`,
`core-rs/crates/mqk-portfolio` for `day_start`, `previous_close`,
`prev_close`, `opening_equity`, `start_of_day` — zero matches anywhere in
the repo. There is no day-start equity snapshot, no previous-session-close
mark persisted, and no schema column carrying either. Computing `daily_pnl`
would require fabricating one of these inputs, which is prohibited.

**Q8 — truthful unavailable reason for daily P&L:**

`daily_pnl` stays `null`. Phase C adds an explicit
`daily_pnl_unavailable_reason` string field (e.g.
`"no_day_start_equity_baseline"`) so the route says why, rather than leaving
an unexplained `null` next to now-populated `unrealized_pnl` fields.

**Q9 — which fields CAN be computed now, from real repo data, without a new
migration:**

- `portfolio/positions[].mark_price` — from `md_bars` latest completed close
  for the position's symbol, same source as `live-weights`.
- `portfolio/positions[].unrealized_pnl` — `(mark_price - avg_price) *
  qty`, using the position's existing `avg_price`/`qty` (already parsed from
  `BrokerPosition.avg_price`/`.qty` strings) and the md_bars mark.
- `portfolio/summary.unrealized_pnl` (new, additive field) — sum of the
  above across all positions, only when every non-flat position has a mark
  (same all-or-nothing truth-state discipline `compute_portfolio_weights`
  already uses for NAV).
- `portfolio/summary.daily_pnl` — **stays unavailable**; no baseline exists
  (Q7).

## 4. No trading behavior changes

This audit is read-only. No strategy, risk, gate, OMS, broker, or reconcile
code is touched. No file outside `docs/specs/` and `scripts/guards/` is
created or modified in this phase.

## 5. No provider/broker calls in tests

Planned Phase B/C tests operate on pure functions and in-process route
handlers against an injected `AppState`/`BrokerSnapshot`/DB pool fixture
(same pattern as `gui_contract_portfolio_positions_active_snapshot` in
`scenario_gui_daemon_contract_gate.rs`). No network, no live provider, no
live/paper broker call is made by any test.

## 6. Chosen mark source

`md_bars` latest completed close at `timeframe="1D"` (default), fetched via
`mqk_db::fetch_recent_completed_bars_for_strategy` — identical source and
function already used by `/api/v1/portfolio/live-weights` and
`/api/v1/portfolio/economics/status`. No new mark source is introduced.

## 7. Fields that can be computed now vs. must stay unavailable

| Field | Computable now | Source |
|---|---|---|
| `portfolio/positions[].mark_price` | Yes | `md_bars` latest completed close |
| `portfolio/positions[].unrealized_pnl` | Yes | `(mark - avg_price) * qty` |
| `portfolio/summary.unrealized_pnl` (new) | Yes, when all non-flat positions have a mark | sum of position-level unrealized P&L |
| `portfolio/summary.daily_pnl` | No | no day-start/previous-close baseline exists anywhere in the repo (Q7) |

## 8. Safety confirmation for this phase

- No live orders: N/A, no order path touched.
- No forced paper orders: N/A, no order path touched.
- No strategy threshold changes: N/A, no strategy code touched.
- No provider calls in tests: confirmed — no test exists yet in this phase;
  Phase B/D tests are specified to use in-process fixtures only.
