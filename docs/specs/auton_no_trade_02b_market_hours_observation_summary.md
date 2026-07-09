# AUTON-NO-TRADE-02B — Market-Hours Observation Summary

## Status

`AUTON-NO-TRADE-02B: CLOSED_LOCAL`

## Window

- Lane: `AUTON-NO-TRADE-02`, legacy/default session source (`MQK_RUNTIME_SESSION_SOURCE`
  unset).
- Real NYSE regular session, 2026-07-09 (Thursday, non-holiday).
- Daemon started via `scripts\windows\Start-PaperTradingSmoke.ps1` (canonical
  operator startup path). The script's STEP 9B multi-symbol watchlist gate
  failed (`schema_version=''`, 0 symbols — this repo's `AAPL` config is not
  provisioned as a `watchlist-v2` multi-symbol entry) and the script exited
  before STEP 10+. This gate is internal to the smoke script
  (`MULTI-SYMBOL-SMOKE-RUNNER-PREFLIGHT-GATE-01`), not a core daemon safety
  gate — STEP 1–9 had already completed successfully: Docker/paper-Postgres
  verified, `.env.local` loaded, migrations applied, market-data prep passed,
  daemon built and started (PID 29036), identity verified
  (`daemon_mode=paper`, `adapter_id=alpaca`, `live_routing_enabled=false`),
  Alpaca WS continuity reached `live`.
- The daemon's own autonomous session controller (not the smoke script, which
  had already exited) subsequently armed and started a run on its normal
  30-second tick cadence, per its documented behavior in
  `docs/runbooks/autonomous_paper_ops.md` §1 ("the session controller starts
  and stops execution runs automatically").
- Observation window: `run_id=1d005ad4-bec5-54b8-9291-c0a932626a1a` running
  2026-07-09T15:00:28Z through at least 2026-07-09T15:12:30Z (~12 minutes of
  continuous live observation at the time of this doc), confirmed via
  live route polls and direct read-only DB queries against
  `mqk-paper-postgres` (paper DB, port 5440).

## Answers

**1. Did a canonical paper order attempt occur?**
No. `GET /api/v1/execution/orders` returned `[]`. `GET /api/v1/execution/flow`
returned `rows: []` for `run_id=1d005ad4-...`. DB-verified:
`select count(*) from oms_outbox where run_id='1d005ad4-...'` = 0;
`select count(*) from oms_inbox where run_id='1d005ad4-...'` = 0.

**2. If no, what durable explanation exists?**
The strategy engine ticked and genuinely evaluated market conditions. One
`strategy_signal_evaluations` row exists for this run
(`evaluation_id=c76fcb96-f1d1-532e-8b6b-8a0482b4d2ee`,
`ts_utc=2026-07-09T15:04:58.865726Z`, `strategy_id=intraday_scalper`,
`symbol=AAPL`, `timeframe=5m`, `source=mqk-daemon.execution_loop` — a real
daemon-produced row, not a test fixture): `decision_stage=strategy_evaluated`,
`reason_code=flat_below_threshold`, `signal_generated=false`,
`signal_qty=0`. The route-level `strategy_decision_diagnostics` on
`/api/v1/autonomous/readiness` shows the underlying numbers:
`move_bps=-19` vs `threshold_bps=20` (`gap_to_threshold_bps=1`) —
price displacement over the lookback window was one basis point short of
the strategy's entry threshold. This is a genuine no-trade result, not an
error or a blocked gate.

After that single evaluation, `autonomous_no_trade_diagnostics` recorded
11 rows for this run (`NO_ACTIVE_RUN_PENDING_START` →
`STRATEGY_NOT_TICKED` × 4 → `NO_SIGNAL_GENERATED` × 6, roughly one per
minute from 15:01 to 15:10 UTC), all with `paper_order_attempted=false`,
`live_order_attempted=false`. Later polls additionally show
`intraday_bar_stale` (`DATA-FRESHNESS-READINESS-GATE-01`) once the single
market-data top-off performed during startup aged past the 900s freshness
ceiling — expected fail-closed behavior given no continuous intraday
ingest loop is running in this observation window (the smoke script that
would normally keep refreshing bars had already exited at STEP 9B). This
does not change the answer to Q1/Q2: the no-trade result was established
before staleness became a factor, and no order was ever attempted at any
point in the window.

**3. Which route proves it?**
`GET /api/v1/execution/orders`, `GET /api/v1/execution/flow`,
`GET /api/v1/execution/signal-evaluations`,
`GET /api/v1/autonomous/no-trade-diagnostics`,
`GET /api/v1/autonomous/readiness` (`strategy_decision_diagnostics`).

**4. Which DB table proves it?**
`oms_outbox` (0 rows for this run), `oms_inbox` (0 rows for this run),
`strategy_signal_evaluations` (1 row, `signal_generated=false`),
`autonomous_no_trade_diagnostics` (11 rows, all
`paper_order_attempted=false`), `runs` (`status=RUNNING`,
`started_at_utc`/`armed_at_utc`/`running_at_utc` populated,
`stopped_at_utc`/`halted_at_utc` NULL), `sys_arm_state`
(`state=ARMED`).

**5. Did live routing stay false?**
Yes. `system/status.live_routing_enabled=false` on every poll across the
window, `kill_switch_active=false`, `risk_halt_active=false`,
`integrity_halt_active=false`.

**6. Did any live order occur?**
No. Expected and confirmed — see Q1/Q4.

**7. Did any threshold/gate/config change?**
No. `MQK_RUNTIME_SESSION_SOURCE` was left unset (legacy) throughout this
lane. No strategy threshold, risk, or gate config was modified. The only
mutating operator actions taken were those the canonical startup script
itself performs as documented (arm/baseline-adopt, all logged above) and
the daemon's own autonomous session controller starting the run — no
manual `start-system`, no forced order route was called.

**8. Is parent `AUTON-NO-TRADE-01` closable from this evidence?**
Not from this doc alone — see
`docs/specs/auton_no_trade_02c_market_hours_closure_decision.md` for the
closure decision, which composes this summary with
`AUTON-NO-TRADE-02A`'s prior read-only audit.

## Non-blocking correction to the Phase A schema audit

`information_schema.columns`, queried live against `mqk-paper-postgres` at
approximately 2026-07-09T15:09Z, proves the following columns **do** exist
in the current schema:

- `runs.armed_at_utc`, `runs.running_at_utc`, `runs.stopped_at_utc`,
  `runs.halted_at_utc`, `runs.last_heartbeat_utc`
- `oms_outbox.claimed_at_utc`

This directly contradicts the "forbidden stale column" list carried in
`docs/runbooks/market_hours_proof_sweep_01.md` (inherited from the
`AUTON-NO-TRADE-02A` preflight audit's schema assumptions). The queries in
this observation (see `runs` query above, which used these columns
successfully) relied on the corrected, live-verified schema — not the
runbook's stale list.

This is a documentation-accuracy gap, not an `AUTON-NO-TRADE-02` blocker,
and is **not** corrected in `docs/runbooks/market_hours_proof_sweep_01.md`
in this phase: doing so would put text into the runbook that the Phase A
validator's forbidden-column check (`scripts/guards/validate_market_hours_proof_sweep_01.ps1`
check [9]) rejects, and that guard script is outside this phase's allowed-file
scope to correct alongside it. Flagged for `MARKET-HOURS-PROOF-SWEEP-01E`
or a dedicated follow-up patch.

## Safety confirmation

- No live orders submitted.
- No paper order forced (no order-submitting route was called by the
  operator at any point; the single evaluation and all order tables are
  daemon-produced, not manually inserted).
- No provider/broker behavior changed beyond the documented, pre-existing
  startup script's normal market-data top-off (TwelveData + Alpaca REST,
  both already required by the canonical startup runbook).
- No config persisted beyond the normal `.env.local`-driven startup (no
  `MQK_RUNTIME_SESSION_SOURCE` override was used in this lane).
- No gate weakened, no strategy threshold changed.
- No fabricated data — all rows cited above are read directly from the
  live daemon routes and the paper DB, with `source` fields distinguishing
  daemon-produced rows from prior test fixtures.
- Generated evidence (`exports/market_hours_proof_sweep/auton_no_trade_02/`,
  `exports/smoke/`, `exports/market_data/`) is untracked/generated only and
  was not staged.
