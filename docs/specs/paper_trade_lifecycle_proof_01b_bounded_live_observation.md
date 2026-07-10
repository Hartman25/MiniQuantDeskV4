# Paper Trade Lifecycle Proof — 01B — Bounded Live Observation

Patch ID: `PAPER-TRADE-LIFECYCLE-PROOF-01B-BOUNDED-LIVE-SMOKE-OBSERVATION-01`
Parent bundle: `PAPER-TRADE-LIFECYCLE-PROOF-01-COMBINED`

Docs-only change (this file + ledger). The observation itself exercised
the existing canonical paper path (daemon start, intraday refresh loop,
autonomous runtime) exactly as already proven safe by
`PAPER-SMOKE-FOLLOWUP-01D`/`01E`. No orders were submitted, forced, or
simulated. No thresholds, gates, or config were changed. No live routing.

## 1. Smoke command used

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4

powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Start-PaperTradingSmoke.ps1 `
  -StartIntradayRefreshLoop `
  -IntradayRefreshIntervalSeconds 300 `
  -RequireIntradayRefresh `
  -WatchSeconds 1800
```

Exactly the invocation selected in `01A`. Ran to completion, printing
`=== Startup complete ===` with no `exit 1` at any step.

## 2. Observation start/end time

- Daemon started (STEP 7): `2026-07-10T11:10:12-05:00` (`16:10:12` UTC).
- Runtime run `2f5e0619-df6b-5907-a0f1-ad019b2dfb57` created/armed/running:
  `2026-07-10 16:10:42` UTC (per `runs` table `started_at_utc` /
  `armed_at_utc` / `running_at_utc`, all within the same second-scale
  window).
- Watcher loop (STEP 16) ran the full `1800`s window, last tick at
  `[+1799s]`, ending approximately `2026-07-10T11:40:41-05:00`
  (`16:40:41` UTC).
- Evidence capture (route probes + DB readback, this phase) performed
  `2026-07-10T11:44-11:46 CDT` (`16:44-16:46` UTC), with the daemon and
  run still live (`runs.status = RUNNING`, no `stopped_at_utc`) at
  capture time.

## 3. Was intraday refresh active?

Yes. STEP 8C started `Refresh-IntradayMarketData.ps1` as a background
process (PID 33080) with `-IntervalSeconds 300 -DurationSeconds 1800`,
confirmed in the smoke log:

```text
[SMOKE] OK: Intraday refresh loop started (PID 33080); symbol=AAPL timeframe=5m interval=300s duration=1800s.
```

## 4. Did `/market-data/intraday-refresh/status` pass?

Mixed — it passed once, then failed for the rest of the observed window.

- At STEP 14C (immediately before runtime start), the gate passed:
  `truth_state=active all_passed=true stale_or_missing_evidence=false`
  (per smoke log).
- At evidence-capture time (`~16:44 UTC`, after the full watch window),
  the same route returned:

```json
{
  "truth_state": "active",
  "all_passed": false,
  "reason": "fail-closed: AAPL [latest_bar_stale_after_refresh: stale by 2152s (threshold=900s); provider_returned_stale_intraday_data]",
  "symbols": [{
    "symbol": "AAPL",
    "gate": "FAIL",
    "latest_completed_bar_ts": "2026-07-10 16:00:00",
    "latest_completed_bar_age_secs": 2152,
    "max_allowed_age_secs": 900,
    "freshness_truth_state": "stale_after_refresh",
    "reason_code": "provider_returned_stale_intraday_data"
  }]
}
```

The refresh loop's periodic top-offs (every 300s for 1800s) never
ingested a completed bar newer than `2026-07-10 16:00:00` UTC — TwelveData
did not return a fresher completed 5m bar for AAPL for the remainder of
the observed window, so staleness grew monotonically from the moment of
the STEP 14C pass onward. Raw evidence:
`exports/paper_trade_lifecycle_proof_01/market-data_intraday-refresh_status.json`
(untracked).

## 5. Did the strategy evaluate?

Yes, once. `strategy_signal_evaluations` for run
`2f5e0619-df6b-5907-a0f1-ad019b2dfb57`:

```text
ts_utc                  | strategy_id      | decision_stage    | reason_code         | signal_generated | bars_loaded | latest_bar_ts_utc
2026-07-10 16:15:13 UTC | intraday_scalper | pre_dispatch_gate | intraday_bar_stale  | f                 | 30          | 2026-07-10 16:00:00 UTC
```

This single evaluation occurred ~33 seconds after `running_at_utc`
(`16:10:42`). At that moment the latest completed bar (`16:00:00`) was
`913`s old — 13 seconds past the `900`s `DATA-FRESHNESS-READINESS-GATE-01`
threshold, a near-miss failure at the gate boundary. No further
evaluation rows were recorded for this run for the remainder of the
30-minute watch window, consistent with the per-tick freshness gate
continuing to block dispatch before it could reach strategy re-evaluation
(staleness only grew from `913`s toward the `2152`s seen at
evidence-capture time, per §4).

Only one row for this run exists in the table; no additional rows appear
between `16:15:13` and the end of the watch window.

## 6. Was a signal generated?

No. `signal_generated=false`, `signal_qty=null`, `signal_side=null` on
the single evaluation row. `decision_stage=pre_dispatch_gate` means the
strategy did not reach its 20bps-threshold evaluation at all — the
freshness gate blocked dispatch before strategy logic ran.

## 7. Did risk evaluate?

No. Risk evaluation is downstream of strategy signal generation in the
canonical chain; since no signal was generated, risk was never invoked.
No risk-denial reason code appears anywhere in this run's diagnostics.

## 8. Was a paper order submitted?

No.

```sql
select outbox_id, run_id, status, created_at_utc, claimed_at_utc, sent_at_utc
from oms_outbox
where run_id = '2f5e0619-df6b-5907-a0f1-ad019b2dfb57'
order by created_at_utc desc limit 50;
-- (0 rows)
```

Corroborated by `GET /api/v1/execution/flow` (`rows: []`) and
`GET /api/v1/execution/summary` (`active_orders: 0, pending_orders: 0,
dispatching_orders: 0`).

## 9. Was broker ack/fill received?

No — there was nothing to acknowledge.

```sql
select inbox_id, run_id, event_kind, broker_order_id, received_at_utc, applied_at_utc
from oms_inbox
where run_id = '2f5e0619-df6b-5907-a0f1-ad019b2dfb57'
order by received_at_utc desc limit 50;
-- (0 rows)
```

## 10. Did position/accounting update?

No. `GET /api/v1/portfolio/positions` returned `rows: []`
(`snapshot_state=active`, `snapshot_source=external`). No position
change occurred.

## 11. Was realized/unrealized P&L visible?

No change to observe. `GET /api/v1/portfolio/summary`:

```json
{
  "account_equity": 1000055.81,
  "cash": 1000055.81,
  "long_market_value": 0.0,
  "short_market_value": 0.0,
  "daily_pnl": null,
  "buying_power": 1000055.81
}
```

Cash/equity unchanged from the pre-existing broker-baseline-adopted
state (STEP 11 in the smoke log reported `positions=0 orders=0`). No P&L
was generated because no trade occurred.

## 12. Did live routing stay false?

Yes, throughout. `GET /api/v1/system/status` at evidence-capture time:
`"live_routing_enabled": false`. Every watcher tick line in the smoke
log (120 ticks over 1800s) also shows `live_routing=false`. STEP 8
confirmed `daemon_mode=paper adapter_id=alpaca live_routing_enabled=false`
at startup.

## 13. Did any live order occur?

No. `autonomous_no_trade_diagnostics.live_order_attempted = false` on
every row for this run. Zero live broker calls of any kind — the daemon
ran in `paper` mode against the Alpaca paper endpoint only.

## 14. Was any paper order forced?

No. This phase called no manual order-submit, cancel, replace, or
flatten endpoint at any point. The only network/DB activity performed
directly by this phase (outside the smoke script's own natural behavior)
was read-only: `Invoke-RestMethod` GETs and read-only `psql` queries.

## 15. Exact DB rows/routes proving result

- `runs`: one row, `run_id=2f5e0619-df6b-5907-a0f1-ad019b2dfb57`,
  `status=RUNNING`, `started_at_utc=2026-07-10 16:10:42+00`.
- `strategy_signal_evaluations`: one row for this run, `16:15:13 UTC`,
  `decision_stage=pre_dispatch_gate`, `reason_code=intraday_bar_stale`.
- `oms_outbox`: 0 rows for this run.
- `oms_inbox`: 0 rows for this run.
- `sys_arm_state`: `sentinel_id=1 state=ARMED reason=(empty)
  updated_at_utc=2026-07-10 16:10:16+00`.
- Routes: `GET /api/v1/system/status`, `/system/preflight`,
  `/autonomous/readiness`, `/market-data/intraday-refresh/status`,
  `/execution/signal-evaluations`, `/autonomous/no-trade-diagnostics`,
  `/execution/summary`, `/execution/flow`, `/execution/orders`,
  `/portfolio/positions`, `/portfolio/summary`, `/alerts/active` — all
  captured to `exports/paper_trade_lifecycle_proof_01/*.json`
  (untracked, not staged; raw JSON available for independent
  verification but not part of this commit).

## 16. Durable no-trade reason

**Blocker 1 (data-freshness reliability window), reproduced live.**
`DATA-FRESHNESS-READINESS-GATE-01` passed once at STEP 14C
(immediately pre-runtime-start) but the very first per-tick strategy
evaluation 33 seconds into the run already found the latest completed
AAPL/5m bar 913 seconds old — 13 seconds past the 900-second threshold.
The 300-second-interval intraday refresh loop then ran for the remaining
~27 minutes of the watch window without ever ingesting a bar newer than
`2026-07-10 16:00:00` UTC, so staleness grew to 2152 seconds by
evidence-capture time. This is not a code gap — the freshness gate
(`core-rs/crates/mqk-runtime` orchestrator, per-tick) correctly fails
closed exactly as designed rather than dispatching on stale data. The
observed condition is that TwelveData's real-world completed-bar
publish cadence for AAPL/5m did not keep pace with the 900-second
staleness ceiling during this specific window, consistent with the
operational condition already named as Blocker 1 in
`paper_trading_shortest_path_01c_minimum_blocker_chain.md` (that prior
finding attributed staleness to sandbox-clock-vs-provider-clock skew;
this run's near-miss-then-growing-staleness pattern is consistent with
provider publish lag as an alternate or compounding mechanism — both are
the same class of operational/timing condition, not a code defect).

No paper order occurred. This is an acceptable, durable, route-and-DB
proven no-trade result per this bundle's closure standard.
