# PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01E — Closure Decision

Patch group: `PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED`, Phase E
(final). Docs-only.

## 1. Is `PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED` closed?

```text
PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED: PARTIAL / DAILY-PNL-BASELINE-OPEN
PAPER-TRADE-LIFECYCLE-PROOF-02: PNL-SEAM-CLOSED-BY-PAPER-PNL-01 (mark_price / unrealized_pnl); DAILY-PNL-BASELINE-OPEN
```

`mark_price` and `unrealized_pnl` on both `/api/v1/portfolio/positions` and
`/api/v1/portfolio/summary` are **closed**: code committed
(`44b79e89`), tests committed and passing against the real local paper DB
(`scenario_paper_pnl_operator_visibility_01.rs`, 9/9). `daily_pnl` stays
**open**: it cannot be truthfully computed with this repo's current schema
(§5), and closing it would require a new day-start-equity-baseline capture
mechanism, which is out of this patch group's scope.

## 2. Exact P&L fields now active

- `portfolio/positions[].mark_price` — active whenever a completed `md_bars`
  bar exists for the position's symbol at the queried timeframe (currently
  hardcoded `"1D"`) and a DB pool is configured.
- `portfolio/positions[].unrealized_pnl` — active under the same condition;
  `(mark_price - avg_price) * qty`.
- `portfolio/positions[].pnl_truth_state` / `.pnl_unavailable_reason` /
  `.mark_source` — new, always present, explain the above whenever P&L is
  not `"active"`.
- `portfolio/summary.unrealized_pnl` — aggregate of the above, active only
  when every position's own P&L is computable.
- `portfolio/summary.pnl_truth_state` / `.pnl_unavailable_reason` — new,
  mirror the aggregation outcome.

## 3. Mark source used

Latest completed `md_bars` close at `timeframe="1D"` (hardcoded constant
`DEFAULT_POSITIONS_PNL_TIMEFRAME`), via
`mqk_db::fetch_recent_completed_bars_for_strategy` — the identical
source/function `/api/v1/portfolio/live-weights` already used. No new mark
source was introduced. Phase D's DB readback found this paper account's
real `AAPL` data only has `5m` bars, not `1D` — see §6 for the follow-up
this implies.

## 4. What happens when marks are unavailable

- `qty == 0`: `pnl_truth_state = "flat"`, `unrealized_pnl = 0.0`, no DB or
  mark lookup performed at all.
- No DB pool configured: `pnl_truth_state = "db_unavailable"`,
  `pnl_unavailable_reason = "no_db_pool_configured"`.
- DB present, no completed bar for the symbol at the queried timeframe:
  `pnl_truth_state = "mark_unavailable"`,
  `pnl_unavailable_reason = "no_completed_md_bars_row_for_symbol"`.
- `avg_price` string fails to parse: `pnl_truth_state = "mark_unavailable"`,
  `pnl_unavailable_reason = "avg_price_unparseable"` (defensive; not hit by
  any real broker-snapshot data observed).

`mark_price` and `unrealized_pnl` are `null` in every non-`"active"` case.
Nothing is ever fabricated.

## 5. Is daily P&L truly computed or still unavailable?

Still unavailable. `daily_pnl` is unconditionally `null` on
`/api/v1/portfolio/summary`, with a new `daily_pnl_unavailable_reason`
field (`"no_day_start_equity_baseline_in_schema"` when a broker snapshot is
present, `"no_broker_snapshot"` when it is not). Phase A's repo-wide grep
for `day_start`/`previous_close`/`prev_close`/`opening_equity`/
`start_of_day` across `mqk-db`, `mqk-daemon`, `mqk-portfolio` found zero
matches — there is no schema column or in-memory concept anywhere in this
repo that could supply a truthful baseline. Computing `daily_pnl` would
require fabricating one, which `CLAUDE.md`'s no-fabricated-truth invariant
prohibits.

## 6. Did any trading behavior change?

No. Zero strategy, risk, OMS, broker, reconcile, or gate code was touched
across Phases A-D. `scenario_paper_pnl_operator_visibility_01.rs`'s PPV-09
test proves both routes make zero writes to `oms_outbox`. Both routes
remain pure reads of `st.broker_snapshot` plus a read-only `md_bars`
lookup.

## 7. Were any provider/broker/network calls made in tests?

No. All Phase B/C tests are either pure-function unit tests (no IO) or
in-process `axum` route calls against an injected `AppState`. The DB-backed
tests (PPV-05 through PPV-09) connect only to the local
`mqk-paper-postgres` container via `MQK_DATABASE_URL` — no provider, no
broker adapter, no network call to any external host.

## 8. Were any orders submitted?

No. No route in this patch group's scope has order-submission authority.
No manual, forced, live, or paper order was submitted at any phase.

## 9. Was any generated evidence staged?

No. `git status --porcelain` before and after this session's commits shows
only the pre-existing allowed untracked paths
(`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`, `smoke_logs/`) plus
this session's own tracked commits under `docs/specs/`,
`scripts/guards/`, and `core-rs/crates/{mqk-portfolio,mqk-daemon}/`. No
`exports/` artifact was created or staged.

## 10. What is the next best patch?

Two independent, smaller follow-ups this closure surfaced but does not
itself perform:

1. Add optional `timeframe` query-param support to
   `/api/v1/portfolio/positions` and `/summary` (mirroring
   `/live-weights`), or reconsider the hardcoded `"1D"` default — Phase D's
   DB readback found this paper account's real intraday data is `5m`-only
   for `AAPL`, so the `"1D"` default currently reports `"mark_unavailable"`
   for a position that does have a resolvable mark at `"5m"`.
2. If an operator explicitly wants `daily_pnl` computed, a new patch would
   need to design and introduce a day-start/previous-close equity baseline
   capture mechanism (a schema/architecture decision, not a wiring fix) —
   out of scope for a visibility-only patch.

## Safety confirmation

- No live orders: confirmed, zero across all phases.
- No forced paper orders: confirmed, zero across all phases.
- No strategy threshold changes: confirmed.
- No gate weakening: confirmed — no gate code touched.
- No fabricated marks or P&L: confirmed — every mark traces to a real
  `md_bars` row; every P&L traces to `mqk_portfolio::unrealized_pnl_micros`
  applied to that mark plus the position's real `avg_price`; `daily_pnl`
  stays honestly `null` rather than being estimated.
- No generated evidence staged: confirmed (§9).
- No `.env.local` edit: confirmed.
- No config flag change: confirmed.
- No DB migration: confirmed — no migration file was added; all Phase C
  code reads existing `md_bars`/`BrokerSnapshot` fields.
