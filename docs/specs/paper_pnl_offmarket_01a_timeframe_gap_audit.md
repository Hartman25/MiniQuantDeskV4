# PAPER-PNL-OFFMARKET-01A — Timeframe Gap Audit

Patch group: `PAPER-PNL-OFFMARKET-COMPLETION-01-COMBINED`, Phase A.
Docs-only audit. No code change. No DB mutation. No order submitted. No
provider/broker/network call. No daemon restart.

## 1. Current HEAD

`d8a3da46` (`docs: close paper pnl visibility`), branch `main`, clean
tracked tree (only the pre-existing allowed untracked
`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and `smoke_logs/`).

## 2. Proof-02 real AAPL context

`PAPER-TRADE-LIFECYCLE-PROOF-02` produced a real autonomous paper trade:

- `AAPL qty=3 avg_price=314.81`
- paper order submitted naturally (no forced/manual submission)
- broker ack/fill occurred via the normal broker inbound lane
- position/accounting updated from that fill
- `live_routing_enabled=false` throughout
- no live orders, no forced paper orders

## 3. Current Paper-PNL-01 result

`PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED` (commits `bb8f8552` →
`99729e6b` → `44b79e89` → `0404c6bc` → `d8a3da46`) added a mark/unrealized-
P&L seam to `/api/v1/portfolio/positions` and `/api/v1/portfolio/summary`:

- `compute_broker_positions_pnl` resolves a mark from the latest
  *completed* `md_bars` close for each non-flat broker-snapshot position,
  combined with the position's own `avg_price` via
  `mqk_portfolio::unrealized_pnl_micros`.
- `pnl_truth_state` / `pnl_unavailable_reason` distinguish `active`,
  `flat`, `mark_unavailable`, and `db_unavailable` — never fabricated.
- Final status recorded in the ledger: **PARTIAL / DAILY-PNL-BASELINE-OPEN**
  — `daily_pnl` remains `null` with reason
  `no_day_start_equity_baseline_in_schema` because no day-start /
  previous-close equity baseline exists anywhere in this repo's schema.

## 4. Current real DB readback (from Phase D of the prior patch group)

`paper_pnl_operator_visibility_01d_readback_proof.md` §3 recorded, via
read-only `docker exec mqk-paper-postgres psql` queries against
`miniquantdesk_paper`:

```text
select distinct timeframe, count(*) from md_bars where symbol='AAPL' group by timeframe;
  timeframe | count
 -----------+-------
  5m        |  6111
```

- AAPL has **6111 rows at `timeframe='5m'`**.
- AAPL has **zero rows at `timeframe='1D'`**.
- Latest completed `5m` bar: `end_ts=1783707900`
  (`2026-07-10T18:25:00Z`), `close_micros=314860000` (`$314.86`),
  `is_complete=true`.

## 5. Why the hardcoded `"1D"` produces a truthful — but unhelpful — answer

`core-rs/crates/mqk-daemon/src/routes/portfolio.rs:43` hardcodes:

```rust
const DEFAULT_POSITIONS_PNL_TIMEFRAME: &str = "1D";
```

`compute_broker_positions_pnl` (same file, lines 89–188) always queries
`mqk_db::fetch_recent_completed_bars_for_strategy(pool, &p.symbol,
DEFAULT_POSITIONS_PNL_TIMEFRAME, 1)`. Against the real AAPL position, this
finds zero completed `1D` rows, so `bars.last()` is `None` and the route
correctly (not fabricated, but not maximally useful) reports:

```text
pnl_truth_state = "mark_unavailable"
pnl_unavailable_reason = "no_completed_md_bars_row_for_symbol"
```

even though a real, completed mark **does** exist at `timeframe="5m"`.

## 6. The safe fix: optional `timeframe` query param

Add an optional `timeframe` query parameter to `GET
/api/v1/portfolio/positions` and `GET /api/v1/portfolio/summary`, mirroring
the pattern `/api/v1/portfolio/live-weights` already uses
(`LiveWeightsParams { timeframe: Option<String> }`,
`DEFAULT_LIVE_WEIGHTS_TIMEFRAME = "1D"`, trim + blank-defaults-to-default):

- Default (no query param, or blank `?timeframe=`) remains `"1D"` —
  byte-for-byte the current behavior, fully backward compatible.
- `?timeframe=5m` threads `"5m"` into
  `fetch_recent_completed_bars_for_strategy` instead of the hardcoded
  constant, so the real AAPL position resolves against its actual `5m`
  ingestion cadence.
- `mark_source` becomes `format!("md_bars:{timeframe}:close")` using the
  *selected* timeframe (already the exact string-building pattern
  `portfolio_live_weights` uses), so the response is self-describing about
  which timeframe produced the mark.

## 7. Why query-param selection is safer than changing the default

- Changing `DEFAULT_POSITIONS_PNL_TIMEFRAME` to `"5m"` would silently
  change behavior for every existing caller and for any symbol/account
  whose real ingestion cadence *is* daily — a hidden default change is
  exactly the kind of non-obvious behavior CLAUDE.md's determinism
  invariant warns against.
- A query param requires the caller (operator, GUI, or test) to explicitly
  opt into a different resolution timeframe, and the response's own
  `mark_source` states which timeframe was actually used — no ambiguity,
  no silent default drift, and zero risk to any caller who does not pass
  the parameter.
- This exactly mirrors the precedent already shipped and proven for
  `/api/v1/portfolio/live-weights` (`PORTFOLIO-LIVE-WEIGHTS-01`).

## 8. Why `daily_pnl` remains out of scope

`daily_pnl` requires a day-start or previous-session-close *equity*
baseline, which is an entirely different data problem from *which mark
timeframe to query for the current price*: no such baseline exists
anywhere in this repo's schema today (repeated repo-wide grep, zero
matches — see `paper_pnl_operator_visibility_01a_current_truth_audit.md`
§Q7). Selecting a `timeframe` for the current mark does not create, imply,
or approximate that baseline. Building it is Phase D of this bundle:
**design-only**, no schema migration, no baseline-capture code, in this
patch group.

## 9. Safety invariants for this fix

- No trading behavior changes — no order, fill, ack, cancel, or OMS state
  transition is touched by this fix.
- No provider/broker/network calls in tests — all proof is DB-backed
  (local `mqk-paper-postgres`) or fully in-process route calls via
  `tower::ServiceExt::oneshot`, matching the existing
  `scenario_paper_pnl_operator_visibility_01.rs` convention.
- No orders submitted, forced, or simulated by this patch or its tests.
- No DB migration — `md_bars` already has a `timeframe` column; this fix
  only changes which value is passed as a query argument.
- No fabricated marks/P&L — every mark still comes from a real completed
  `md_bars` row; `mark_unavailable`/`db_unavailable` still fire whenever no
  real row exists at the selected timeframe.
