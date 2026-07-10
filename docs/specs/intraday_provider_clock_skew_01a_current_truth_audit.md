# INTRADAY-PROVIDER-CLOCK-SKEW-01A — Current Truth Audit

`INTRADAY-PROVIDER-CLOCK-SKEW-OPERATOR-GUARD-01-COMBINED`, Phase A.

**Current HEAD:** `d86e46ab` (branch `main`).

## 1. What this patch is

This is a diagnostic / operator-visibility patch. It exists to surface provider
publish-lag and freshness-headroom risk to the operator *before* and *during* a
paper-trade proof run. It is not a strategy patch, not a threshold patch, and
not a gate-weakening patch — `DATA-FRESHNESS-READINESS-GATE-01` must remain
exactly as strict as it is today.

## 2. Summary of the two live proof windows that hit stale provider data

**`PAPER-SMOKE-FOLLOWUP-01E`** — `Start-PaperTradingSmoke.ps1 -StartIntradayRefreshLoop
-IntradayRefreshIntervalSeconds 300 -IntradayRefreshDurationSeconds 650 -WatchSeconds 300
-SkipGui` ran multiple refresh-loop iterations against real AAPL/5m data.
`GET /api/v1/market-data/intraday-refresh/status` reported `all_passed=false`,
`reason_code="provider_returned_stale_intraday_data"` for the whole watch window.
`DATA-FRESHNESS-READINESS-GATE-01` correctly fired `bar_data_stale`/`intraday_bar_stale`
on every 5-minute dispatch tick; zero trades attempted.

**`PAPER-TRADE-LIFECYCLE-PROOF-01B-BOUNDED-LIVE-SMOKE-OBSERVATION-01`** — run
`2f5e0619-df6b-5907-a0f1-ad019b2dfb57`, a 1800-second live watch during NYSE
regular hours, `Start-PaperTradingSmoke.ps1 -StartIntradayRefreshLoop
-IntradayRefreshIntervalSeconds 300 -RequireIntradayRefresh -WatchSeconds 1800`.
`STEP 14C` (`-RequireIntradayRefresh`) passed once immediately before runtime
start (`truth_state=active`, `all_passed=true`, `stale_or_missing_evidence=false`).
33 seconds after runtime start, the first per-tick strategy evaluation hit
`intraday_bar_stale` — the latest AAPL/5m completed bar age was 913s against the
900s cap. TwelveData never delivered a fresher completed AAPL/5m bar for the
remaining ~27 minutes of the window. By evidence capture the observed age had
grown to 2152s. Result: `live_routing_enabled=false` throughout, no signal
generated, no risk evaluation, zero `oms_outbox` rows, zero `oms_inbox` rows,
no position/accounting update, no live order, no forced paper order, no
threshold/gate/config changed. Closed as
`PAPER-TRADE-LIFECYCLE-PROOF-01-COMBINED: CLOSED_LOCAL / PARTIAL / DATA-FRESHNESS-BLOCKED`.

## 3. Exact observed values from the latest run (`2f5e0619-df6b-5907-a0f1-ad019b2dfb57`)

- Latest completed AAPL/5m bar age at first tick evaluation (33s after runtime
  start): **913s**, against a **900s** cap (`MQK_INTRADAY_BAR_MAX_AGE_SECS` default).
- Age at evidence capture, later in the window: **2152s**.
- No newer completed AAPL/5m bar was delivered by the provider for the
  remaining ~27 minutes of the 1800s window.

## 4. The freshness gate worked correctly

`DATA-FRESHNESS-READINESS-GATE-01` (`evaluate_md_freshness_status` /
`evaluate_md_freshness_status_for_symbols` in
[market_data_freshness.rs](../../core-rs/crates/mqk-daemon/src/market_data_freshness.rs))
computes bar age **live**, per dispatch tick, as `age_secs = (now_ts -
latest_end_ts).max(0)` — never from a cached/frozen value. It fired the
correct fail-closed outcome (`bar_data_stale` / `intraday_bar_stale`,
`signal_generated=false`) on every tick for the whole window. This is the
system behaving exactly as designed: truth was unavailable (no fresh provider
bar), so the system denied, never optimistically passed. This patch must not
change that behavior in any way.

## 5. This patch must not weaken the gate

No gate weakening. No strategy threshold changes. No forced paper orders. No
live routing. `MQK_INTRADAY_BAR_MAX_AGE_SECS` is not touched. Stale bars are
never marked fresh. This is a read-only diagnostic layer on top of already-fail-closed
behavior.

## 6. Current refresh evidence fields

`scripts/windows/Refresh-IntradayMarketData.ps1` writes
`exports/market_data/intraday_refresh_<ts>.json` (`schema_version:
"intraday-refresh-v1"`) with, per symbol: `latest_completed_bar_age_secs` (a
**snapshot** computed once at refresh time via `Get-StalenessSeconds`, not
live), `max_allowed_age_secs`, `freshness_truth_state`, `reason_code`, `passed`,
plus provider provenance (`provider_source`, `provider_success`,
`provider_rows_read`, etc.). Top level carries `produced_at_utc`, `mode`,
`source`, `timeframe`, `all_passed`, `reason`.

**Key finding:** because `latest_completed_bar_age_secs` is a snapshot taken at
`produced_at_utc`, and the daemon's own dispatch-tick freshness check computes
age live against wall-clock `now()`, real bar age grows by exactly the elapsed
wall-clock time between evidence production and the first (or any) dispatch
tick. This is the precise mechanism behind the 2026-07-10 run: the evidence
was fresh (age below 900s) at `produced_at_utc`, but by the time the first
tick ran 33 seconds later, true age had crossed 900s. This is clock-skew in
the trivial sense — no provider clock is actually skewed; it is elapsed real
time between an evidence snapshot and a live gate check that the operator
currently has no visibility into.

## 7. Current daemon status route fields

`GET /api/v1/market-data/intraday-refresh/status`
([transport_quality.rs:553](../../core-rs/crates/mqk-daemon/src/routes/transport_quality.rs)) reads
the latest `intraday_refresh_*.json` evidence file (alphabetically-last
filename = chronologically latest) and relays it through
`IntradayRefreshStatusResponse` / `IntradayRefreshSymbolStatus`
([api_types.rs:4760](../../core-rs/crates/mqk-daemon/src/api_types.rs)). It
already **conservatively recomputes** a per-symbol verdict from evidence
fields (`parse_refresh_symbol`): if `provider_success=true` but
`latest_completed_bar_age_secs > max_allowed_age_secs`, it forces
`passed=false`, `freshness_truth_state="stale_after_refresh"`,
`reason_code="provider_returned_stale_intraday_data"` (or
`"latest_bar_stale_after_refresh"` for non-intraday timeframes) — proven by
`IRS-10`/`IRS-11` in
[scenario_intraday_md_refresher_operator_surface_01.rs](../../core-rs/crates/mqk-daemon/tests/scenario_intraday_md_refresher_operator_surface_01.rs).
Route-level `all_passed` is forced `false` whenever any symbol's recomputed
`passed` is `false`, even if the evidence file's own `all_passed` claims
`true`. `stale_or_missing_evidence` flags evidence older than 24h
(`INTRADAY_EVIDENCE_STALE_SECS = 86_400`) using `produced_at_utc`.

**Gap found:** the route does **not** account for elapsed wall-clock time
since `produced_at_utc` when evaluating a symbol's freshness. It relays the
evidence's own `latest_completed_bar_age_secs` verbatim (a snapshot), so a
symbol that was fresh with only a few seconds of headroom at refresh time
still reports `passed=true` / `fresh_after_refresh` seconds or minutes later,
even though the *true*, current bar age (evidence age + elapsed time since
`produced_at_utc`) may have already crossed `max_allowed_age_secs`. No
`freshness_headroom_secs`, `staleness_overage_secs`, `near_expiry`,
`proof_window_risk`, or `operator_action` field exists today at either the
per-symbol or top level.

## 8. Current smoke-script preflight behavior

`Start-PaperTradingSmoke.ps1` `STEP 14C` (`-RequireIntradayRefresh`,
[Start-PaperTradingSmoke.ps1:1477](../../scripts/windows/Start-PaperTradingSmoke.ps1))
calls the status route once and fails closed (`exit 1`, before STEP 15 starts
runtime) unless `truth_state=="active"`, `stale_or_missing_evidence!=true`,
and `all_passed==true`. **Gap found:** it performs no headroom check — a
response with `all_passed=true` but only a few seconds of margin before
`max_allowed_age_secs` passes STEP 14C exactly the same as a response with
600s of margin. This is exactly the scenario that produced the 2026-07-10
run's 33-second-later failure: the preflight check passed, then true age
crossed the cap almost immediately after runtime start.

## 9. Exact safe patch plan for Phases B–D

- **Phase B:** add a pure, evidence-only classifier (no provider/DB/network
  calls) that computes `freshness_headroom_secs` /
  `staleness_overage_secs` / `near_expiry` / `proof_window_risk` /
  `operator_action` per symbol, accounting for elapsed time since
  `produced_at_utc` so the reported headroom reflects the *current* moment,
  not the evidence snapshot moment. Wire it into
  `transport_quality.rs`/`api_types.rs` additively (existing fields
  untouched). Extend `scenario_intraday_md_refresher_operator_surface_01.rs`
  with new fixture-only tests.
- **Phase C:** conditional — only needed if Phase B does not already expose
  the new fields on the existing route response. Given Phase B is being done
  directly on `transport_quality.rs`/`api_types.rs` (the same route), Phase C
  is expected to be a no-op/skip.
- **Phase D:** teach `Start-PaperTradingSmoke.ps1 -RequireIntradayRefresh` to
  read the new headroom field(s) and fail closed (with an actionable message)
  when evidence is fresh but headroom is below a parameterized
  `-MinFreshnessHeadroomSeconds` (default 120s) — a script-preflight-only
  guard that never touches daemon dispatch-tick gate behavior.

## 10. Non-goals

- No threshold changes (`MQK_INTRADAY_BAR_MAX_AGE_SECS` untouched).
- No strategy logic changes.
- No forced paper orders, no manual paper order submission.
- No provider/broker/network calls in tests — all new tests are fixture-file-only.
- No live routing enablement.
- No trading behavior changes of any kind.
- No DB migration, no DB mutation.
- No new provider network behavior in production scripts (existing refresh
  path is unchanged; it remains operator-triggered exactly as it is today).
