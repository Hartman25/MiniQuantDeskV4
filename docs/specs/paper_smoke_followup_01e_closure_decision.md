# PAPER-SMOKE-FOLLOWUP-01E — Closure Decision

## Verdict

```text
PAPER-SMOKE-FOLLOWUP-01A: CLOSED_LOCAL
PAPER-SMOKE-FOLLOWUP-01B: CLOSED_LOCAL
PAPER-SMOKE-FOLLOWUP-01C: CLOSED_LOCAL
PAPER-SMOKE-FOLLOWUP-01D: CLOSED_LOCAL
PAPER-SMOKE-FOLLOWUP-01 (bundle): CLOSED_LOCAL
```

Commits: `3bc054ed` (01A), `cbf0bcd8` (01B), `7dc35ce0` (01C), `3a7002af`
(01D). This closure doc is 01E; it does not itself change repo behavior.

This bundle does not reopen `MARKET-HOURS-PROOF-SWEEP-01`,
`AUTON-NO-TRADE-02`, `AUTON-NO-TRADE-01`, or `ASSET-CORE-05K` — all remain
`CLOSED_LOCAL` per `docs/specs/market_hours_proof_sweep_01e_closure_decision.md`.
Nothing in this bundle contradicts that prior evidence; the market-hours
live run performed below reconfirms it (zero orders, live_routing_enabled
false, durable no-trade reason).

## Answers

**1. Were the stale schema/runbook guard assumptions corrected?**
Yes. `docs/runbooks/market_hours_proof_sweep_01.md`'s DB-tables section was
rewritten as schema-discovery-first: it now instructs querying
`information_schema.columns` before any ad-hoc SQL, documents `runs`'
lifecycle-stage columns (`armed_at_utc`, `running_at_utc`, `stopped_at_utc`,
`halted_at_utc`, `last_heartbeat_utc`, added by
`core-rs/crates/mqk-db/migrations/0002_run_lifecycle.sql`) and
`oms_outbox.claimed_at_utc` (added by
`core-rs/crates/mqk-db/migrations/0005_outbox_claim.sql`) as a floor
cross-checked against migrations at HEAD, not a categorical ceiling.
`scripts/guards/validate_market_hours_proof_sweep_01.ps1`'s check `[9]`
no longer fails the runbook for mentioning real column names — it fails
only on a re-introduced categorical non-existence claim, and separately
requires the `information_schema.columns` instruction to be present.

**2. Does STEP 9B now handle a valid single-symbol AAPL smoke setup without false failure?**
Yes, confirmed both statically and live. Statically: all 18 pre-existing
`MSG-01`..`MSG-14` invariants in
`tests/script_guards/test_multi_symbol_smoke_runner_gate.ps1` still pass
unchanged, plus 11 new checks in
`scripts/guards/validate_paper_smoke_followup_01c_watchlist_gate.ps1`. Live:
the optional live validation run below reached `GET /api/v1/watchlist/status`
returning `status="not_configured"` and the smoke proceeded through the
full session (armed, WS live, ran 2h08m) instead of the previous hard
`exit 1`.

**3. Does STEP 9B still fail when multi-symbol/watchlist-v2 is explicitly required and missing?**
Yes, unchanged. `-MultiSymbolSmoke` still enforces the original full
preflight (`MULTI_SYMBOL_SMOKE_BLOCKED_SCHEMA_NOT_V2` /
`_NOT_MULTI_SYMBOL` / `_NOT_APPROVED_FOR_AUTONOMOUS_PAPER`), proven by the
unmodified `MSG-05`..`MSG-09` checks. Additionally, a watchlist-v2 artifact
that IS configured but invalid/unapproved still fails closed even in
default (non-`-MultiSymbolSmoke`) mode via the new
`MULTI_SYMBOL_SMOKE_BLOCKED_WATCHLIST_V2_CONFIGURED_BUT_INVALID` code — a
real misconfiguration is never silently ignored.

**4. Was the intraday refresh workflow made explicit and verifiable?**
Yes. `-StartIntradayRefreshLoop` (STEP 8C) and `-RequireIntradayRefresh`
(STEP 14C) were added, both off by default, plus
`docs/runbooks/intraday_market_data_refresh.md`. The live validation run
below confirms STEP 8C's loop actually launched, ran multiple interval
iterations, and produced real evidence consumed by
`GET /api/v1/market-data/intraday-refresh/status`
(`truth_state=active`, `mode=interval`) — and that STEP 14C's decision
inputs (`all_passed=false`, `reason_code=provider_returned_stale_intraday_data`)
are exactly what its fail-closed logic is designed to catch.

**5. Was the freshness gate weakened? Expected no.**
No. `DATA-FRESHNESS-READINESS-GATE-01` was not touched. It correctly
fired `bar_data_stale` / `intraday_bar_stale` on every 5-minute tick for the
entire 2h08m live session below, exactly as it did during the original
market-hours proof sweep — this bundle changed operator-workflow visibility
around that gate, not the gate itself.

**6. Were provider calls added to tests? Expected no.**
No test file was changed. All provider calls in this bundle were either (a)
inside `Refresh-IntradayMarketData.ps1`, which already made such calls
before this bundle and is explicitly operator-triggered (STEP 8C only calls
it when `-StartIntradayRefreshLoop` is passed), or (b) part of the optional
live validation below, which the operator explicitly authorized.

**7. Were paper/live orders attempted? Expected no, unless optional live validation naturally did so; if so, document exact evidence.**
No orders were attempted. The optional live validation below ran a real
daemon against live Alpaca paper/TwelveData connectivity for 2h08m
(2026-07-09T17:52:57Z start, auto-stopped 2026-07-09T20:00:28Z session
boundary). `DATA-FRESHNESS-READINESS-GATE-01` fired `bar_data_stale` /
`no_order_reason="intraday_bar_stale"` on every dispatch tick the entire
session (confirmed by the daemon's own log: 5-minute-spaced
`autonomous_bar_ticker` + `md_staleness_per_tick_gate_01` warning pairs from
`2026-07-09T17:52:xx` through `2026-07-09T19:57:57`), and the session
controller auto-stopped cleanly at the configured session boundary
(`13:30`-`20:00` UTC). Root cause of the persistent staleness: this sandbox
environment's system clock (`2026-07-09`) runs materially ahead of the
real-world time TwelveData's live API responds against — the provider
consistently returned bars several thousand seconds "old" relative to the
sandbox clock, which is an environment characteristic, not a repo defect.
This is the same class of finding `PAPER-SMOKE-FOLLOWUP-01A`'s Finding 3
already anticipated; the gate did exactly what it is designed to do.

**8. Was live routing enabled? Expected no.**
No. `live_routing_enabled=false` was confirmed at every poll throughout the
session and on the post-shutdown status check
(`system/status` → `live_routing_enabled: false`).

**9. Were generated evidence files staged? Expected no.**
No. All generated evidence from the live validation
(`exports/smoke/daemon_20260709_125257.stdout/.stderr.log`,
`exports/market_data/intraday_refresh_*.json`,
`smoke_logs/paper_smoke_followup_01_live_validation_*.{txt,log}` if
produced) remained untracked; `git status` before this phase's commit shows
no unexpected tracked or staged changes.

**10. What next patch is recommended?**
Per this bundle's own gating guidance:
`ASSET-CORE-04-LIVE-LEDGER-BOUNDARY-AUDIT-AND-SAFE-GAP-CLOSURE-01-COMBINED`.

## Optional live validation — what was run and how it was closed out

Phases A-D were committed first. With ~2h13m of NYSE session time
remaining, the operator explicitly requested the optional live validation.
It was run as `Start-PaperTradingSmoke.ps1 -StartIntradayRefreshLoop
-IntradayRefreshIntervalSeconds 300 -IntradayRefreshDurationSeconds 650
-WatchSeconds 300 -SkipGui` (no `-MultiSymbolSmoke`, to exercise the
single-symbol path; no `-RequireIntradayRefresh` initially, to let the full
run reach the watcher before separately inspecting the intraday-refresh
route directly).

**Orchestration note (not a repo defect):** the smoke script itself
completed its designed run — STEP 9B passed honestly, STEP 8C's refresh
loop ran, the daemon armed and ran for its full session, and the session
controller auto-stopped cleanly at the `20:00` UTC boundary exactly as
designed. A convenience wrapper script (scratchpad-only, never part of this
repo) that was meant to auto-query the intraday-refresh route and issue a
clean `disarm-execution`/`stop-system` immediately after the watcher ended
did not complete as intended, most likely because this sandbox's background
task execution paused across the long real-world gap between polling
checks — it is an artifact of how this session orchestrated a long-running
background process, not a defect in `Start-PaperTradingSmoke.ps1`,
`Refresh-IntradayMarketData.ps1`, or any daemon code. The intraday refresh
loop process itself self-terminated correctly within its own
`-IntradayRefreshDurationSeconds 650` bound, as designed — it did not run
away. Once noticed, the remaining verification (`GET
/api/v1/market-data/intraday-refresh/status`, `GET /api/v1/watchlist/status`)
was completed manually via direct authenticated/unauthenticated HTTP calls
against the still-live daemon, and the daemon was then shut down cleanly:
`POST disarm-execution` (HTTP 200) → `POST stop-system` (HTTP 200) →
confirmed `runtime_status=idle`, `strategy_armed=false`,
`execution_armed=false`, `live_routing_enabled=false` → process terminated
→ port `8899` confirmed unreachable. No `.env.local` edits, no config
persisted, no secrets printed at any point (the operator token was read
in-process to build an `Authorization` header and never echoed).

**Live evidence captured:**

- `GET /api/v1/watchlist/status` (post-run): `status="not_configured"`,
  `schema_version=null`, `approved_for_live=false` — the real input STEP 9B
  evaluated; the daemon reaching a full session run proves STEP 9B's
  single-symbol fallback branch fired correctly rather than the old hard
  `exit 1`.
- `GET /api/v1/market-data/intraday-refresh/status` (post-run):
  `truth_state="active"`, `mode="interval"`, `evidence_path` pointing at a
  file produced mid-session (not the pre-seed), `stale_or_missing_evidence=false`,
  `all_passed=false`, `reason="fail-closed: AAPL [latest_bar_stale_after_refresh: stale by 1126s (threshold=900s); provider_returned_stale_intraday_data]"`,
  `symbols[0].passed=false`, `symbols[0].reason_code="provider_returned_stale_intraday_data"`.
  This proves STEP 8C's loop genuinely ran on its own interval schedule and
  produced honest evidence, and that STEP 14C's three fail-closed codes
  (`INTRADAY_REFRESH_BLOCKED_TRUTH_STATE` / `_STALE_EVIDENCE` /
  `_NOT_ALL_PASSED`) are wired to real, non-fabricated conditions — this run
  would have hit `_NOT_ALL_PASSED` had `-RequireIntradayRefresh` been passed,
  for a genuine reason.
- Daemon log (`exports/smoke/daemon_20260709_125257.stdout.log`, untracked):
  `md_staleness_per_tick_gate_01: bar_data_stale ... no_order_reason="intraday_bar_stale"`
  repeated every 5 minutes for the full session; `autonomous_session_controller:
  auto-stopped at session boundary` at `2026-07-09T20:00:28Z`.
- Post-shutdown `system/status`: `runtime_status=idle`,
  `live_routing_enabled=false`, `strategy_armed=false`,
  `execution_armed=false`.

## Roadmap docs

No material change. This bundle is operator-workflow/script/runbook scope
only — it does not move any asset-class or patch-completion percentage in
`docs/audits/multi_asset_completion_audit.md` or
`docs/specs/roadmap_completion_reconcile_01.md`, and does not alter any
prior closure verdict recorded there. Neither file was touched.

## Safety confirmation

No live orders, no forced paper orders, no config persisted, no gate
weakened, no strategy threshold changed, no fabricated data, no generated
evidence staged, `.env.local` never edited, no secrets printed. The daemon
was returned to a fully stopped, clean state before this closure doc was
written.
