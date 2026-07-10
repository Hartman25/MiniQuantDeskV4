# PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01D — Real DB / Route Readback Proof

Patch group: `PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED`, Phase D.
Read-only DB queries via `docker exec mqk-paper-postgres psql` and read-only
`GET` calls against the daemon already running on this machine. No DB
mutation, no order submit, no daemon restart, no code change in this phase.

## 1. Schema discovery

`information_schema.columns` for `md_bars`, `oms_outbox`, `oms_inbox`,
`runs` confirmed the column names Phase A/B/C code already assumed
(`md_bars.close_micros`/`end_ts`/`is_complete`, `oms_outbox.outbox_id`,
`runs.run_id`/`status`) — no surprises, no correction needed to
Phase A-C code.

## 2. Proof-02 run still present

```text
run_id=15cf4309-210b-5406-8ed8-46377e093195  status=STOPPED
started_at_utc=2026-07-10 18:31:01.324149+00
stopped_at_utc=2026-07-10 18:49:18.481407+00
```

Matches `paper_trade_lifecycle_proof_02_fast_market_hours_retry.md` exactly
(the run whose `AAPL qty=3 avg_price=314.81` fill exposed the P&L gap this
patch group closes).

## 3. AAPL md_bars: only `5m`, zero `1D` rows

```sql
select distinct timeframe, count(*) from md_bars where symbol='AAPL' group by timeframe;
--  timeframe | count
-- -----------+-------
--  5m        |  6111
```

Zero rows at `timeframe='1D'` for `AAPL`. Latest completed `5m` bar:

```text
symbol=AAPL timeframe=5m end_ts=1783707900 (2026-07-10T18:25:00Z)
close_micros=314860000 ($314.86) is_complete=true
```

**Finding:** `/api/v1/portfolio/positions` and `/api/v1/portfolio/summary`
(Phase C) hardcode `DEFAULT_POSITIONS_PNL_TIMEFRAME = "1D"`, matching
`/api/v1/portfolio/live-weights`'s own default. Against this paper
account's actual ingestion (5-minute intraday bars only, no daily bars),
querying at `"1D"` finds zero completed rows for `AAPL`, so the route
truthfully reports `pnl_truth_state="mark_unavailable"` /
`pnl_unavailable_reason="no_completed_md_bars_row_for_symbol"` for the real
position — not a lie, but not the most useful answer available, since a
mark **does** exist at `"5m"`.

Hand-computed (no code path touched) what the route *would* return if it
resolved against the `5m` bar instead of `1D`:

```text
mark_price_micros = 314_860_000
avg_price_micros  = 314_810_000
qty               = 3
unrealized_pnl_micros = (314_860_000 - 314_810_000) * 3 = 150_000
unrealized_pnl = $0.15
```

This is exactly the formula Phase B's `unrealized_pnl_micros` computes and
Phase C's PPV-05/PPV-06 DB-backed tests already proved correct against a
seeded bar — the arithmetic is not in question, only which `timeframe` the
route queries by default.

## 4. Live daemon readback (pre-patch binary, no restart performed)

A daemon is currently running on this machine (`http://127.0.0.1:8899`).
This session made no source rebuild-and-restart of that process — restarting
live daemon infrastructure is outside this patch's stated scope (code +
tests only) and was not authorized. The running process therefore still
serves the pre-Phase-C binary. Read-only `GET` calls against it:

```json
GET /api/v1/portfolio/positions
{"snapshot_state":"active","rows":[{"symbol":"AAPL","qty":3,"avg_price":314.81,
"mark_price":null,"unrealized_pnl":null,"broker_qty":3,"drift":null}],
"snapshot_source":"external"}

GET /api/v1/portfolio/summary
{"has_snapshot":true,"truth_state":"active","account_equity":1000057.12,
"cash":999111.38,"long_market_value":944.43,"daily_pnl":null}
```

Confirms: the real `AAPL qty=3 avg_price=314.81` position is still present
in the live broker snapshot, `mark_price`/`unrealized_pnl`/`daily_pnl` are
still `null` on the unpatched binary — unchanged from proof-02, as expected
since this binary predates this patch group. This is evidence of the "before"
state only, not a test of Phase C's new code.

## 5. Standing code-level proof

Since the live daemon was not restarted, the closure evidence that the
Phase C code path is correct comes from
`scenario_paper_pnl_operator_visibility_01.rs`'s DB-backed tests (PPV-05
through PPV-09), which ran for real against this same `mqk-paper-postgres`
instance (not skipped — `MQK_DATABASE_URL` in `.env.local` points at it)
using a seeded completed bar at the position's queried timeframe, and
produced exactly the expected positive/negative P&L, mark-unavailable
truth state, and zero `oms_outbox` writes. The code is proven; only the
*live, unpatched process's* answer for the real `AAPL` position under its
actual `1D`-empty/`5m`-populated market data is what remains unproven
without a rebuild + restart this phase does not perform.

## 6. Conclusion

- Real position + real DB: **present**, confirmed via `runs`/broker-snapshot readback.
- Real completed mark for `AAPL`: **present**, but only at `timeframe="5m"`, not the route's `"1D"` default.
- Code correctness: **proven** via DB-backed scenario tests against this same database.
- Live-route replay against the real `AAPL` position with the patched binary: **pending** — requires either a daemon rebuild+restart (out of this phase's scope) or a follow-up patch adding timeframe selection to these two routes.

No DB mutation. No order submitted. No daemon restart. No code change in
this phase.
