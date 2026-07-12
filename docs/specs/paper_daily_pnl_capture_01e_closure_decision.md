# Paper Daily P&L Baseline Capture — 01E Closure Decision

Patch group: `PAPER-DAILY-PNL-BASELINE-CAPTURE-AND-OPERATOR-CLOSURE-01-COMBINED`,
Phase E.

## 1. Is `PAPER-DAILY-PNL-BASELINE-CAPTURE-AND-OPERATOR-CLOSURE-01-COMBINED` closed?

```text
PAPER-DAILY-PNL-BASELINE-CAPTURE-AND-OPERATOR-CLOSURE-01-COMBINED: CLOSED_LOCAL
```

Yes. An explicit, authenticated, operator-controlled capture path writes a
real `sys_account_equity_baseline` row from the daemon's current
broker/account snapshot; `/api/v1/portfolio/summary.daily_pnl` becomes
`"active"` once that row exists for the required prior trading day; the
route remains honestly `"baseline_unavailable"` without one. All of this
is proven by 22 DB-backed scenario tests against the real local
`mqk-paper-postgres` (port 5440), not by session memory or a prior
conversation claim.

## 2. What capture route/action was added?

`POST /api/v1/ops/action {"action_key":"capture-account-equity-baseline",
"reason":"...", "trading_date":"YYYY-MM-DD"}` — a new arm on the existing
`ops_action` dispatcher (`core-rs/crates/mqk-daemon/src/routes/control_plane.rs`),
not a dedicated route. `OpsActionRequest` gained one new optional field
(`trading_date: Option<String>`); `OperatorActionResponse` gained one new
optional field (`captured_baseline: Option<CapturedAccountEquityBaselineSnapshot>`),
both in `core-rs/crates/mqk-daemon/src/api_types.rs`.

## 3. Is capture explicit and operator-controlled?

Yes. There is no timer, tick, session-boundary detector, or any other
automatic trigger anywhere in the daemon that calls this action. It only
ever runs in response to an authenticated `POST /api/v1/ops/action` call
naming this exact `action_key`.

## 4. Is capture authenticated?

Yes. `/api/v1/ops/action` is registered on the daemon's `operator`
sub-router (`core-rs/crates/mqk-daemon/src/routes.rs`), which is wrapped
in `token_auth_middleware` — identical to `arm-execution`,
`flatten-paper-positions`, and every other mutating `ops/action` arm.
Proven by `PDBC-01`: under `OperatorAuthMode::TokenRequired`, a missing or
wrong bearer token is refused with 401 before any handler gate runs.

## 5. What date validation is enforced?

`trading_date` must be present, must parse as a strict `"YYYY-MM-DD"`
date, and must be a real NYSE trading day per the existing
`NyseWeekdaysProvider` seam (`core-rs/crates/mqk-daemon/src/state/market_calendar.rs`),
probed at 18:00 UTC — the identical convention `resolve_daily_pnl`'s
`most_recent_trading_day_before` already uses for the read side. Proven
by `PDBC-05` (missing), `PDBC-06` (malformed), and `PDBC-07` (a real,
independently-verified Saturday) — all three refused before any DB write.

## 6. What DB row is written?

Exactly one row in `sys_account_equity_baseline`, via the pre-existing
`mqk_db::upsert_account_equity_baseline` helper (idempotent by
`trading_date`, last-write-wins) — no new DB function, no new migration.
Proven idempotent by `PDBC-09`: capturing the same `trading_date` twice,
with different equity values, still leaves exactly one row (holding the
second call's values).

## 7. What provenance is recorded?

`equity_micros`/`cash_micros` parsed from the daemon's real, in-memory
`AppState.broker_snapshot.account.{equity,cash}` (never fabricated — a
parse failure fails closed as `unparseable_account_values` rather than
defaulting to `0`); `currency` from the same snapshot;
`captured_at_utc = Utc::now()` at the moment of the accepted request;
`captured_by = "operator:capture-account-equity-baseline"` (a fixed
constant, not free-text operator input, so the audit-ID seed below stays
reproducible); `broker_snapshot_source` from the daemon's real
`BrokerSnapshotTruthSource`; and a deterministic `audit_event_id`
(`Uuid::new_v5(&Uuid::NAMESPACE_DNS, "mqk.account-equity-baseline.v1|
{trading_date}|{equity_micros}|{cash_micros}|{currency}|{captured_by}|
{broker_snapshot_source}")`), mirroring the `request-mode-change` arm's
`intent_id` precedent exactly. Proven reproducible by `PDBC-11`: two
calls with identical inputs return the identical `audit_event_id`.

## 8. Does summary `daily_pnl` become active after capture?

Yes. Proven by `PDBC-12` (positive daily P&L), `PDBC-13` (negative), and
`PDBC-14` (exactly zero) — in each case, a real capture is followed by a
real `GET /api/v1/portfolio/summary` call, and `daily_pnl_truth_state ==
"active"` with `daily_pnl == current_equity - captured_baseline_equity`
to the cent.

## 9. Does summary stay unavailable without capture?

Yes, unchanged from `PAPER-DAILY-PNL-BASELINE-01-COMBINED`. Proven by
`PDBC-15`: with no capture and no residual row in the lookback window,
`daily_pnl_truth_state` stays `"baseline_unavailable"` and `daily_pnl`
stays `null`. `PDBC-16` additionally proves the summary route itself
never writes a baseline row across repeated calls — the read side and
write side remain strictly separated.

## 10. Does a read-only baseline route exist?

Yes: `GET /api/v1/portfolio/account-equity-baseline?trading_date=YYYY-MM-DD`
(public, no auth — matches every other read-only `/api/v1/portfolio/*`
route), handler `portfolio_account_equity_baseline_status`
(`core-rs/crates/mqk-daemon/src/routes/portfolio.rs`). It distinguishes
`"invalid_request"`, `"db_unavailable"`, `"not_found"`, `"query_failed"`,
and `"active"` (full provenance) — proven by `PDBC-18` through `PDBC-22`,
including a full capture-then-read-back loop confirming the surfaced
`audit_event_id` matches exactly what the capture call itself returned.

## 11. Was any baseline fabricated?

No. Every written value traces to a real caller-supplied `trading_date`
and the daemon's real in-memory `broker_snapshot` at the moment of the
call. A parse failure on the broker-reported equity/cash strings fails
closed (`unparseable_account_values`) rather than substituting `0` or any
other default.

## 12. Was any historical baseline backfilled?

No. Each capture call writes exactly one row, for exactly the one
`trading_date` the caller supplies. No code path iterates over, infers,
or guesses any other date.

## 13. Were any provider/broker/network calls made in tests?

No. All 22 tests in `scenario_paper_daily_pnl_baseline_capture_01.rs`
either require no DB at all (`PDBC-01`, `PDBC-02`, `PDBC-18`..`PDBC-20`)
or connect only to the local `mqk-paper-postgres` container (port 5440)
already running on this machine. No test constructs a broker adapter or
provider client.

## 14. Were any live/paper orders submitted?

No. `PDBC-10` proves a successful capture call writes zero
`oms_outbox`/`oms_inbox` rows. No test in this patch group constructs a
runtime, orchestrator, or broker adapter, and the capture/read-only
handlers touch no order-lifecycle table.

## 15. Were any thresholds/gates/config changed?

No. No strategy, risk-gate, freshness-gate, session-gate, or config-flag
code was touched anywhere in this patch group. No `.env.local` edit.

## 16. Was any DB migration added?

No. `0045_account_equity_baseline.sql` (already committed by the prior
`PAPER-DAILY-PNL-BASELINE-01-COMBINED` bundle) already provides every
column this bundle needed — confirmed by Phase A's re-read of
`core-rs/crates/mqk-db/src/account_equity_baseline.rs` before any code was
written.

## 17. What exact next market-hours proof should be run?

```text
PAPER-TRADE-LIFECYCLE-PROOF-03-PNL-VISIBILITY-VERIFY-COMBINED
```

Rebuild and restart the daemon with this bundle's binary. During market
hours, with a real paper position live: (a) call `POST /api/v1/ops/action
{"action_key":"capture-account-equity-baseline", "reason":"...",
"trading_date":"<yesterday's real trading day>"}` and confirm `accepted:
true` with real provenance; (b) call `GET /api/v1/portfolio/summary` and
confirm `daily_pnl_truth_state == "active"` with a real, sane `daily_pnl`
value; (c) call `GET /api/v1/portfolio/account-equity-baseline?trading_date=...`
and confirm the same provenance is readable back. Confirm `unrealized_pnl`
behavior remains exactly as previously proven (unaffected by this
bundle).

## 18. What exact next off-market completion bundle is recommended?

```text
PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-AUDIT-AND-CLOSURE-01-COMBINED
```

## Final statuses

```text
PAPER-DAILY-PNL-BASELINE-CAPTURE-AND-OPERATOR-CLOSURE-01-COMBINED: CLOSED_LOCAL
PAPER-DAILY-PNL-BASELINE-01-COMBINED: CAPTURE-SEAM-CLOSED-BY-CAPTURE-01
PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED: DAILY-PNL-CAPTURE-AND-READ-SIDE-CLOSED
PAPER-PNL-OFFMARKET-COMPLETION-01-COMBINED: CLOSED_LOCAL (unchanged)
PAPER-TRADE-LIFECYCLE-PROOF-02: PNL-SEAM-CLOSED-FOR-MARK-UNREALIZED-AND-DAILY-PNL-READINESS
```

## Full patch-group commit chain

Phase A `298c57a9` (design) → Phase B `6aef1cda` (operator capture action
+ tests) → Phase C `5e727013` (capture -> summary read-side integration
proof) → Phase D `c3ddab5d` (read-only baseline surface) → Phase E (this
entry, closure).

## Built

`docs/specs/paper_daily_pnl_capture_01a_current_truth_action_design.md`,
`scripts/guards/validate_paper_daily_pnl_capture_01a_design.ps1`,
`core-rs/crates/mqk-daemon/src/api_types.rs` (updated),
`core-rs/crates/mqk-daemon/src/routes/control_plane.rs` (updated),
`core-rs/crates/mqk-daemon/src/routes/control.rs` (updated, mechanical
field addition only),
`core-rs/crates/mqk-daemon/src/routes/portfolio.rs` (updated),
`core-rs/crates/mqk-daemon/src/routes.rs` (updated),
`core-rs/crates/mqk-daemon/tests/scenario_paper_daily_pnl_baseline_capture_01.rs`
(new, 22 tests),
`docs/specs/paper_daily_pnl_capture_01e_closure_decision.md` (this file),
`docs/specs/roadmap_completion_reconcile_01.md` (updated).

## Safety confirmation

No live orders; no forced paper orders; no manually submitted paper
orders; no autonomous smoke script run; no execution armed; no
strategy/threshold/gate change; no fabricated baseline, mark, price, P&L,
fill, order, or position at any phase; no historical baseline backfill;
no `.env.local` edit; no config flag change; zero DB migrations added
(the one migration this whole line of work required, `0045`, was already
committed before this bundle started); no provider/broker/network call in
any test; no generated evidence, smoke log, export, or untracked ledger
draft staged at any phase; no daemon started or restarted during any
phase (all validation used `cargo check`/`cargo test`/`cargo clippy`
against the real local `mqk-paper-postgres` container, never a running
daemon process).
