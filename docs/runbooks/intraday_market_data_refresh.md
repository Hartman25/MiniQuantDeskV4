# Intraday Market-Data Refresh — Runbook

## Purpose

`scripts/windows/Refresh-IntradayMarketData.ps1` keeps `md_bars` current
during a paper session by periodically topping off completed intraday bars
from a provider. `scripts/windows/Start-PaperTradingSmoke.ps1` STEP 5B only
runs a **one-shot** market-data prep at startup — it does not keep bars
fresh for the rest of a market-hours session. Left unattended, the strategy
runs out of fresh bars and `DATA-FRESHNESS-READINESS-GATE-01` correctly
fails closed (`intraday_bar_stale`) once the one-shot prep's bars age past
the configured max staleness (default `MQK_INTRADAY_BAR_MAX_AGE_SECS` or
900 seconds). This is **expected, correct, fail-closed behavior** — the gap
this runbook closes is operator workflow, not gate weakening.

## When to use

Any market-hours paper smoke session intended to run longer than the
freshness-gate window (default ~15 minutes) needs a continuous refresh
mechanism running alongside it. There are two ways to get one, from least to
most integrated:

### Option 1 — run the refresher yourself, in a separate terminal

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 `
    -Symbols AAPL -Timeframe 5m -IntervalSeconds 300 -DurationSeconds 1800
```

This is always safe to run alongside `Start-PaperTradingSmoke.ps1` — it only
touches `md_bars` via the same `mqk-cli md sync-provider` path any operator
can invoke directly. It never touches `oms_outbox`, `oms_inbox`,
`broker_order_map`, `runs`, or `sys_arm_state`, and never calls a broker
order endpoint.

### Option 2 — let the smoke script start it for you

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 `
    -StartIntradayRefreshLoop
```

`-StartIntradayRefreshLoop` is explicit opt-in and off by default — the
smoke script never starts provider refresh network activity unless this
flag is passed. When set, STEP 8C (after daemon identity verification,
before the WS-continuity wait) launches
`Refresh-IntradayMarketData.ps1` as a separate hidden PowerShell process
using `MQK_STRATEGY_SYMBOL` and a `5m` timeframe, for
`-IntradayRefreshIntervalSeconds` (default 300s) /
`-IntradayRefreshDurationSeconds` (default 1800s). The child process writes
its own evidence to `exports/market_data/intraday_refresh_*.json`, exactly
as it does when run standalone — untracked, never staged.

### Verifying the refresher is actually working

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 -CheckOnly
```

or, with the daemon running:

```text
GET /api/v1/market-data/intraday-refresh/status
```

which reports `truth_state` (`active` / `no_evidence` / `backend_unavailable`
/ `parse_error`), `stale_or_missing_evidence`, `all_passed`, and per-symbol
`passed` from the most recent evidence file.

To make the smoke script itself refuse to proceed to runtime start unless
that route proves fresh evidence, pass:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 `
    -StartIntradayRefreshLoop -RequireIntradayRefresh
```

`-RequireIntradayRefresh` is also explicit opt-in and off by default. STEP
14C (after autonomous paper-status triage, before STEP 15 runtime start)
fails closed with an actionable `INTRADAY_REFRESH_BLOCKED_*` code and the
exact remediation command if `truth_state != active`,
`stale_or_missing_evidence = true`, or `all_passed != true`. This is an
earlier, more actionable preflight check — it does not replace or weaken
`DATA-FRESHNESS-READINESS-GATE-01`, which still evaluates every
strategy-dispatch tick regardless of this flag.

## Hard rules

- No network calls unless `-StartIntradayRefreshLoop` and/or
  `-RequireIntradayRefresh` are explicitly passed, or the operator runs
  `Refresh-IntradayMarketData.ps1` directly themselves.
- Never edits `.env.local`.
- Never persists secrets.
- Never stages generated evidence
  (`exports/market_data/intraday_refresh_*.json` stays untracked).
- Never widens the freshness threshold or marks stale data fresh.
- Never forces strategy dispatch from stale bars.
- No broker/order/OMS/outbox/inbox/arm-state writes from the refresh path.
