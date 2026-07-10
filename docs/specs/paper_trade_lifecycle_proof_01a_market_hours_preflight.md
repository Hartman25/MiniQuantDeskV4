# Paper Trade Lifecycle Proof — 01A — Market-Hours Preflight

Patch ID: `PAPER-TRADE-LIFECYCLE-PROOF-01A-MARKET-HOURS-PREFLIGHT-01`
Parent bundle: `PAPER-TRADE-LIFECYCLE-PROOF-01-COMBINED`

Docs-only. No trading behavior changed. No live orders. No paper orders.
No network calls beyond `git`/local filesystem inspection. No DB mutation.

## 1. Current HEAD

```text
a1d0e6a1 docs: close shortest path to paper trading audit
```

Confirmed via `git log --oneline -80` and `git branch --show-current` (branch
`main`). Working tree confirmed clean for tracked files: `git diff
--name-only` and `git diff --cached --name-only` both empty. Only allowed
untracked entries present per `git ls-files --others --exclude-standard`:
`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and `smoke_logs/*` (21
pre-existing untracked evidence files from prior bundles, none of which are
part of this patch).

## 2. Current wall-clock time

```text
Fri Jul 10 10:54:04 CDT 2026
```

CDT is UTC-5; NYSE regular session (09:30-16:00 ET / 13:30-20:00 UTC) is
tracked in ET (UTC-4 during EDT). 10:54 CDT = 11:54 AM ET. This is a
Friday with no observed NYSE holiday on 2026-07-10.

## 3. Is the market still open?

Yes at time of writing. 11:54 AM ET is inside the 09:30-16:00 ET regular
session, with approximately 4 hours of regular-session time remaining
before the 16:00 ET close. This satisfies the "enough active time
remaining" precondition for a bounded observation window.

## 4. Exact smoke invocation selected

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4

powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Start-PaperTradingSmoke.ps1 `
  -StartIntradayRefreshLoop `
  -IntradayRefreshIntervalSeconds 300 `
  -RequireIntradayRefresh `
  -WatchSeconds 1800
```

`WatchSeconds 1800` (30 minutes) selected: matches the "suggested 1800 if
30+ minutes remain" default in the mission brief, and with ~4 hours of
session time remaining there is ample headroom for the watch window plus
evidence capture and DB readback after it completes. This is the same
invocation shape already validated by `PAPER-SMOKE-FOLLOWUP-01D`/`01E`
(per `paper_trading_shortest_path_01c_minimum_blocker_chain.md` §1,
Blocker 1) — no new script behavior, no new flags beyond what the script
already exposes.

## 5. Evidence folder path selected

```text
exports/paper_trade_lifecycle_proof_01/
```

This directory is not currently tracked by git (verified: no `exports/`
entries appear in `git ls-files --others --exclude-standard`, and
`exports/` is expected to be gitignored consistent with prior bundles'
use of the same pattern). Raw script stdout/stderr logs, if separately
captured, go to `smoke_logs/` alongside existing untracked entries from
prior bundles. Neither location will be staged.

## 6. Confirmation: observation-only

This patch observes what the existing canonical paper-trading path does
under live market data. It does not submit, force, simulate, or fabricate
any order, signal, ack, fill, position, or P&L event. All evidence comes
from the daemon's own natural execution of the already-proven smoke
script plus read-only route/DB probes.

## 7. Confirmation: no forced orders, no threshold changes, no live routing

- No manual order-submit endpoint will be called at any phase.
- `MICRO_MOVE_BPS` and all other `intraday_scalper` strategy constants
  remain unchanged (`core-rs/crates/mqk-strategy/src/engines/intraday_scalper.rs`
  not touched).
- No risk gate, OMS transition, broker adapter, or DB write path is
  modified.
- `Start-PaperTradingSmoke.ps1` enforces `daemon_mode == paper` and
  refuses if `live_routing_enabled=true` (per script header, "Hard rules
  enforced by this script"); this patch adds no override of that guard.
- `.env.local` is not edited. No temporary env overrides are persisted.

## 8. Expected possible outcomes

1. Current data stale — `RequireIntradayRefresh` fails closed with
   `provider_returned_stale_intraday_data` (or equivalent reason code)
   before runtime start, per Blocker 1 in
   `paper_trading_shortest_path_01c_minimum_blocker_chain.md`.
2. Strategy evaluates but stays flat — signal-evaluations show
   `decision_stage` reaching evaluation with `signal_generated=false`
   because no ≥20bps move coincided with the window (Blocker 2).
3. Signal generated and risk denies — a signal crosses the threshold but
   a risk gate blocks submission; `oms_outbox` shows no row, or
   `autonomous_no_trade_diagnostics` records a risk-denial reason code.
4. Signal generated and paper order submitted — `oms_outbox`/`oms_inbox`
   show a naturally-produced row; this is the first live proof of
   Blocker 3.
5. Ack/fill/position/P&L observed — full downstream lifecycle closes if
   (4) occurs and the broker acknowledges/fills within the watch window.

Any of these five outcomes is an acceptable, truthful result for this
bundle per its closure standard — a durable no-trade explanation is not
a failure.

## 9. Safety confirmation

No live orders. No paper orders submitted by this phase (Phase A performs
no daemon start, no network call, no DB mutation — it is inspection and
doc-drafting only). No trading behavior changed. No config flag changed.
No generated evidence staged.
