# Paper Trade Lifecycle Proof — 02 — Fast Market-Hours Retry

Patch ID: `PAPER-TRADE-LIFECYCLE-PROOF-02-FAST-MARKET-HOURS-RETRY-COMBINED`

Live observation/proof patch. No trading behavior changed. No live orders.
No paper order was forced or manually submitted. No threshold, gate, or
config changed. Docs-only commit.

## 1. HEAD

`c093299a` (unchanged before/during/after this patch — this bundle
commits only docs). Verified via `git log --oneline -1` and
`git diff --check` before commit.

## 2. Market-hours wall-clock window

- Preflight (Phase A) began: `2026-07-10 18:24:55Z` (13:24:55 CDT /
  14:24:55 ET).
- Regular session close: `20:00:00Z` (16:00:00 ET) same day — confirmed
  by `runtime_session_source.legacy_session_state` flipping to
  `after_hours` and `autonomous/readiness.session_window_state` flipping
  to `outside_window` only after this proof's window closed.
- Retry run (`run_id 15cf4309-210b-5406-8ed8-46377e093195`) armed/running
  at `18:31:01Z` (13:31:01 CDT).
- The one full order lifecycle this run produced completed at
  `18:35:32Z`–`18:35:34Z` (13:35:32–13:35:34 CDT) — well inside the
  regular session, ~24 minutes before close.
- Watch loop (STEP 16, 1800s/every 15s) ran from run start through
  `+1797s` (~`19:01:00Z`), i.e. it spanned past close only in wall-clock
  polling terms, not in trading terms — the market was still open for
  the entire portion of the window in which any dispatch activity
  occurred.

## 3. Exact commands run

First attempt (failed a fail-closed preflight gate; superseded by
retry):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Start-PaperTradingSmoke.ps1 `
  -StartIntradayRefreshLoop -IntradayRefreshIntervalSeconds 300 `
  -RequireIntradayRefresh -MinFreshnessHeadroomSeconds 120 -WatchSeconds 1800
```

Retry (the run this doc reports on):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Start-PaperTradingSmoke.ps1 `
  -StartIntradayRefreshLoop -IntradayRefreshIntervalSeconds 300 `
  -RequireIntradayRefresh -MinFreshnessHeadroomSeconds 120 -WatchSeconds 1800
```

Both invocations are identical to the recommended retry command; the
second was a plain re-run after an orphaned intraday-refresh loop
process from the first attempt was stopped (`Stop-Process -Id 31436`)
to avoid two overlapping refresh loops hitting the same provider.

## 4. `WatchSeconds` used and why

`1800`. At preflight time (`18:24:55Z`), close (`20:00:00Z`) was ~95
minutes away — over the 30-minute threshold in the mission's selection
table, so the largest safe value applied.

## 5. Did the repaired guard allow start?

- First attempt: **No.** STEP 14C's one-time synchronous check hit the
  intraday-refresh route mid-cycle, before the just-started refresh
  loop's first successful pass had landed fresh data
  (`all_passed=false`, `reason_code=provider_returned_stale_intraday_data`,
  staleness at that instant `2152s` vs `900s` threshold). This is the
  guard working correctly and fail-closed at that instant — not a
  defect. Per the mission's hard rule ("if the script refuses to start
  ... do not bypass it"), the script's own `exit 1` was honored; no
  bypass was attempted.
- Retry (~7 minutes later, after the already-running refresh loop had
  completed another cycle): **Yes.**
  `truth_state=active all_passed=true stale_or_missing_evidence=false`,
  `proof_window_ready=true proof_window_risk=low`, satisfying
  `-RequireIntradayRefresh` and `-MinFreshnessHeadroomSeconds 120`.

## 6. Intraday refresh status: before / mid-preflight / at retry start

| Field | Before any run (`18:15Z`, stale daemon) | After daemon rebuild, before retry (`18:28:04Z`) | At retry STEP 14C (`~18:3x Z`) |
|---|---|---|---|
| `truth_state` | `active` | `active` | `active` |
| `all_passed` | `false` | `true` | `true` |
| `proof_window_ready` | *(field absent — stale binary)* | `true` | `true` |
| `proof_window_risk` | *(absent)* | `medium` | `low` |
| `freshness_headroom_secs` | *(absent)* | `358` | ≥`120` (gate passed) |
| `staleness_overage_secs` | *(absent)* | `null` | — |
| `near_expiry` | *(absent)* | `false` | — |
| `evidence_elapsed_secs` | *(absent)* | `57` | — |
| `effective_latest_completed_bar_age_secs` | *(absent)* | `542` | — |
| `operator_action` | *(absent)* | `null` | — |

Note: the "before" column reflects a daemon process that had been
running since before commit `526c9c0f` (`daemon: recompute intraday
proof-window age`, committed `2026-07-10 13:16:56 -0500`) landed — it
predates the `01F` repair and therefore lacks the new fields entirely.
This was caught at Phase A preflight (process start time `11:10:12 AM`
vs. fix commit time `13:16:56`) and self-corrected by the smoke
script's own STEP 1 (stale-process kill) + STEP 6 (`cargo build -p
mqk-daemon --release`) — not a manual workaround.

## 7. Did strategy evaluate?

Yes. `strategy_signal_evaluations` (DB), `run_id=15cf4309-...`:

```text
ts_utc=2026-07-10 18:35:32.227815+00  strategy_id=intraday_scalper
symbol=AAPL  timeframe=5m  decision_stage=strategy_evaluated
reason_code=signal_long  reason="move_bps >= threshold_bps"
bars_loaded=30  latest_bar_ts_utc=2026-07-10 18:25:00+00
```

Corroborated live by `autonomous/readiness.strategy_decision_diagnostics`:
`move_bps=23`, `threshold_bps=20`, `decision=signal_long`.

## 8. Was a signal generated?

Yes. Same row: `signal_generated=true signal_qty=3 signal_side=buy`.

A second, later evaluation at `18:40:31Z` hit `decision_stage=pre_dispatch_gate`
/ `reason_code=intraday_bar_stale` (the bar had not advanced past
`18:25:00Z` by then) — this is `DATA-FRESHNESS-READINESS-GATE-01`
correctly blocking a *subsequent* dispatch tick once real-time data
flow degraded (see §14); it did not affect the trade already completed
in §9-§12.

## 9. Was risk evaluated?

Inferred yes, not directly observed at a dedicated risk-decision table
for this symbol/tick: `sys_risk_denial_events` has zero rows for the
entire session (no denial), and `oms_outbox` shows a row created
16ms after the signal (`18:35:32.248112Z`, signal at `18:35:32.227815Z`),
consistent with `execution_rules.md`'s orchestrator phase ordering
(outbox write happens only after risk clears in the dispatch path).
No standalone "risk evaluation passed" event row exists in this schema
to cite directly.

## 10. Was a paper order submitted naturally?

Yes. `oms_outbox`, `run_id=15cf4309-...`:

```text
outbox_id=21  status=ACKED
created_at_utc=2026-07-10 18:35:32.248112+00
claimed_at_utc=2026-07-10 18:35:33.054059+00
sent_at_utc=2026-07-10 18:35:33.211686+00
dispatch_attempt_count=0  last_dispatch_error=(none)
```

No manual/forced order-submit endpoint was ever called — the entire
session's route calls from this operator were read-only `GET`s and
read-only `psql SELECT`s.

## 11. Did broker ack/fill occur?

Yes, both. `oms_inbox`, `run_id=15cf4309-...`:

```text
inbox_id=61  event_kind=ack   broker_order_id=50f5f5d8-3ad9-41ec-8514-767b47290f01  received=18:35:33.330899Z  applied=18:35:33.37214Z
inbox_id=62  event_kind=ack   broker_order_id=50f5f5d8-3ad9-41ec-8514-767b47290f01  received=18:35:33.354812Z  applied=18:35:33.379081Z
inbox_id=63  event_kind=fill  broker_order_id=50f5f5d8-3ad9-41ec-8514-767b47290f01  received=18:35:33.82487Z   applied=18:35:34.373379Z
```

`GET /api/v1/execution/orders` confirms: `internal_order_id=2a445578-da8c-5313-83fe-0a17c0523330`,
`side=buy`, `requested_qty=3`, `filled_qty=3`, `current_status=Filled`.

## 12. Did position/accounting state update?

Yes. `GET /api/v1/portfolio/positions`:

```json
{"symbol":"AAPL","qty":3,"avg_price":314.81,"broker_qty":3,"drift":null}
```

`GET /api/v1/portfolio/summary`: `cash=999111.38` (down from the paper
account's starting cash), `long_market_value=944.43`,
`account_equity=1000057.12`. `broker_qty` matches internal `qty`
(no drift flagged).

## 13. Was realized/unrealized P&L visible to the operator?

**No — this is the one lifecycle gap this run exposes.**
`portfolio/positions.mark_price` and `.unrealized_pnl` are both `null`;
`portfolio/summary.daily_pnl` is `null`. The position and cash/equity
figures updated correctly (§12), but no mark-to-market or P&L value is
surfaced on either primary operator route for this fill. This is the
same pre-existing gap already named in
`paper_trade_lifecycle_proof_01d_closure_decision.md` §4/§9, now
confirmed against a real (not merely hypothetical) filled position for
the first time.

## 14. Secondary finding: WS gap-detected after the trade (informational, non-blocking)

Roughly 18 minutes after the trade (`+1402s` from run start, ~`18:54Z`),
the watcher observed `alpaca_ws_continuity` flip to `gap_detected` and
stay there through the end of the 1800s window, with `runtime_status`
remaining `idle` (no further dispatch, no further orders). Separately,
at `+1092s` (~`18:49:08Z`) a `deadman=expired` condition halted the run;
the smoke script's own documented repair (`disarm-execution` →
`clear-halted-run`, attempt 1 of 1) cleared it automatically per its
built-in logic — this is the script's designed self-heal path, not an
operator bypass of any gate, and `kill_switch_active=false`/
`live_routing_enabled=false` were reverified after. Per
`broker_rules.md`, `GapDetected` is correctly treated as terminal for
the session and blocks further dispatch; no recovery was attempted.
Neither event affected the trade in §7-§12, which had already fully
completed (signal → order → ack → fill → position update) roughly 18
minutes earlier. `runs.status=STOPPED` as of `18:49:18Z` for this
run_id; `runtime_status=idle` at final capture, matching the after-close
system state.

## 15. Did live routing stay false?

Yes, at every observed point: STEP 8 daemon-identity check, all 120
watcher ticks (`live_routing=false` on every line), and the final
`GET /api/v1/system/status` capture (`live_routing_enabled: false`).

## 16. Were any live orders attempted?

No. `daemon_mode=paper adapter_id=alpaca` for the daemon's entire
lifetime this session; zero live-order routes were ever called.

## 17. Was any paper order forced?

No. Every order-lifecycle event in §10-§11 was produced by the
autonomous session controller's own dispatch path from a naturally
generated strategy signal — this operator never called a manual
submit/cancel/replace/flatten endpoint.

## 18. Was any threshold/gate/config changed?

No. `MICRO_MOVE_BPS`/`threshold_bps=20`,
`DATA-FRESHNESS-READINESS-GATE-01`'s 900s threshold,
`MinFreshnessHeadroomSeconds=120`, Gate 0, the routing guard, and
`.env.local` are all unchanged. No source file was edited by this
bundle.

## 19. Exact route/DB evidence

- Routes: `exports/paper_trade_lifecycle_proof_02_20260710_151534/*.json`
  (12 files, all `OK`, zero `.error.json`).
- DB: `runs`, `strategy_signal_evaluations`, `sys_risk_denial_events`,
  `oms_outbox`, `oms_inbox`, `sys_arm_state` queried read-only via
  `docker exec mqk-paper-postgres psql` (see §7, §9-§12 above for the
  exact rows cited).
- Generated evidence path:
  `exports/paper_trade_lifecycle_proof_02_20260710_151534/` — confirmed
  untracked and `.gitignore`-covered (`git check-ignore -v` on a file in
  that path matched `.gitignore:29:exports/`); `git status --porcelain`
  before this commit shows only the two pre-existing allowed untracked
  paths (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`,
  `smoke_logs/`) plus this run's new `smoke_logs/*.log` files, which
  fall under the already-allowed `smoke_logs/` prefix.

## 20. Final lifecycle classification

| # | Stage | Status |
|---|---|---|
| 1 | Authoritative current market data | `CLOSED_LIVE` |
| 2 | Feature calculation | `CLOSED_LIVE` |
| 3 | Strategy evaluation | `CLOSED_LIVE` |
| 4 | Signal generated and recorded | `CLOSED_LIVE` |
| 5 | Risk evaluation | `CLOSED_LIVE` (inferred from outbox creation + zero denials; no standalone risk-pass event table exists to cite directly) |
| 6 | Paper order submitted | `CLOSED_LIVE` |
| 7 | Broker acknowledgment | `CLOSED_LIVE` |
| 8 | Fill received | `CLOSED_LIVE` |
| 9 | Position/accounting state updated | `CLOSED_LIVE` |
| 10 | Realized/unrealized P&L updated | `PARTIAL` — position/cash updated, but `mark_price`/`unrealized_pnl`/`daily_pnl` all `null` on both primary routes |
| 11 | Full lifecycle visible to operator | `PARTIAL` — order/position/summary routes are accurate and non-fabricated, but P&L fields are not populated and `execution/flow` returns `truth_state=no_active_run` once the run ended |

```text
PAPER-TRADE-LIFECYCLE-PROOF-02: PARTIAL / ORDER-FILL-POSITION-PNL-SEAM-FOUND
```

This is a genuine escalation from `PAPER-TRADE-LIFECYCLE-PROOF-01`'s
`PARTIAL / DATA-FRESHNESS-BLOCKED` result: the repaired freshness guard
(`INTRADAY-PROVIDER-CLOCK-SKEW-01F`) let a real signal reach dispatch,
and the full order → ack → fill → position chain closed live for the
first time in this proof series. The remaining gap is narrowly scoped
to P&L visibility, not data freshness or order execution.

## 21. Exact next patch recommendation

`PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED` — per the mission's
own decision table ("If order/fill occurs but P&L visibility is
partial"), and directly evidenced by §13: a real filled position now
exists with `avg_price=314.81`/`qty=3`, giving a concrete non-null
input to compute unrealized P&L against, yet `mark_price`,
`unrealized_pnl`, and `daily_pnl` all render `null` on the routes an
operator would actually check.

## Safety confirmation

- No live orders: confirmed, zero.
- No forced paper orders: confirmed — the one order that occurred was
  dispatched naturally by the autonomous session controller from a
  real signal; zero manual submit/cancel/replace/flatten calls.
- No strategy threshold changes: confirmed.
- No gate weakening: confirmed — `DATA-FRESHNESS-READINESS-GATE-01`
  correctly blocked a later tick (§8) and the WS gap-detected state
  correctly blocked further dispatch (§14); neither was bypassed.
- No fabricated data: confirmed — every fact above traces to a route
  response captured this session or a direct DB `SELECT`.
- No generated evidence staged: confirmed (§19).
- No `.env.local` changes: confirmed.
- No config flag changes: confirmed.
