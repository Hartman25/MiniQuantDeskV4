# DURABLE-PAPER-PORTFOLIO-AND-PNL-01E — Read-Only API and Paper-Lifecycle Integration

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01E-READ-ONLY-API`
Exposes B4-B/B4-C/B4-D's durable truth via three new GET-only routes and
extends the existing paper-lifecycle route additively. No mutation route
anywhere in this patch.

## New routes

- `GET /api/v1/portfolio/durable-summary?run_id=` — account/cash/currency
  from the latest durable Paper+Alpaca snapshot, accounting truth (realized
  P&L, fees, cumulative cash movement, accounting epoch) from
  `sys_paper_portfolio_accounting_state`, unrealized P&L (reusing the
  existing `compute_broker_positions_pnl` mark-lookup helper against the
  durable snapshot's positions), and daily P&L (reusing the existing,
  unmodified `resolve_daily_pnl`).
- `GET /api/v1/portfolio/durable-positions?run_id=` — the durable snapshot's
  position rows, with a snapshot-staleness check (180s threshold, mirroring
  `routes/system.rs::BROKER_SNAPSHOT_STALE_SECS`, applied to the durable
  `captured_at_utc` instead of the in-memory one).
- `GET /api/v1/portfolio/durable-snapshots?limit=20` — bounded recent
  snapshot history (`fetch_recent_paper_portfolio_snapshots`, 1-200,
  default 20).

All three additive to the existing `routes/portfolio.rs` router — none
overloads `/summary`/`/positions`, whose shape and broker-snapshot (not
run) scoping don't fit this restart-surviving, run-scoped truth, per the
B4-A contract's §2 decision.

`run_id` resolution on `durable-summary` mirrors
`routes/paper_lifecycle.rs` exactly: an explicit `?run_id=` query param, or
else the latest durable PAPER run for engine `mqk-daemon` —
never in-memory active-run state, so the route keeps working identically
across a restart. `durable-positions` accepts the same param (echoed on
the response) but its snapshot lookup itself is not run-scoped — B4-B's
`fetch_latest_paper_portfolio_snapshot` reports the single latest
Paper+Alpaca snapshot across all runs, which coincides with "this run's
latest" in the one-run-at-a-time supervised lane this bundle targets. This
is a documented property, not silently glossed over.

## Truth-state vocabulary (reused, not reinvented)

`snapshot_truth_state`: `"active"` | `"snapshot_unavailable"` |
`"snapshot_stale"` | `"db_unavailable"` | `"query_failed"`.
`accounting_truth_state`/`realized_pnl_truth_state`: `"active"` |
`"fill_history_incomplete"` | `"not_found"` | `"db_unavailable"` |
`"query_failed"`. `unrealized_pnl_truth_state`/`daily_pnl_truth_state`
reuse the exact vocabulary `compute_broker_positions_pnl`/`resolve_daily_pnl`
already established (`"active"`, `"mark_unavailable"`, `"db_unavailable"`,
`"baseline_unavailable"`, `"stale_baseline"`, etc.) — extended, not
replaced. `null` always means unavailable; a true zero is the literal
numeric `0` (proven by `durable_summary_incomplete_epoch_blocks_realized_pnl_but_shows_position`,
which asserts `realized_pnl` is `null` while `account_equity` stays populated
from the durable snapshot in the very same response).

## Paper-lifecycle integration

`GET /api/v1/execution/paper-lifecycle`'s `portfolio_truth_state`/
`pnl_truth_state` fields, previously hardcoded to
`"in_memory_only_not_restart_surviving"`, now read the same durable tables
(read-only — this route never triggers a snapshot persist or accounting
replay itself, only `mqk_db::fetch_latest_paper_portfolio_snapshot`/
`fetch_paper_portfolio_accounting_state`):

- `portfolio_truth_state`: `"active"` (a durable snapshot exists) |
  `"snapshot_unavailable"`.
- `pnl_truth_state`: `"active"` (accounting epoch complete) |
  `"fill_history_incomplete"` | `"not_found"` (no accounting row exists yet).

`classify_overall_lifecycle_state` (pure, unit-tested) gained one new
parameter, `durable_accounting: Option<&PaperPortfolioAccountingStateRecord>`,
consulted only when `fill_seen`: `"order_filled_pnl_pending"` (no durable
accounting row yet) | `"order_filled_portfolio_durable_pnl_available"`
(epoch complete) | `"order_filled_portfolio_durable_pnl_incomplete"` (epoch
incomplete) — the exact three-state vocabulary the master mission
specifies. All 8 of the classifier's unit tests (5 pre-existing + 3 new)
pass.

## A pre-existing test's assertion updated, honestly

`scenario_paper_order_lifecycle_visibility_01.rs`'s `pl_11` test asserted
the literal string `"in_memory_only_not_restart_surviving"` for both
fields — this is precisely the placeholder this patch replaces. Updated to
assert the correct honest values for that fixture (`"snapshot_unavailable"`/
`"not_found"`, since that test seeds an outbox+inbox fill but no durable
snapshot or accounting row).

## A cross-phase test-hygiene bug found and fixed

Wiring B4-D's accounting refresh into the same seam B4-C's tests exercise
(`accept_external_broker_snapshot`) gave every call in
`scenario_durable_paper_portfolio_snapshot_persistence_01.rs` (B4-C's own
test file) a new side effect: a `sys_paper_portfolio_accounting_state` row.
That file's `cleanup()` helper, written before B4-D existed, didn't delete
that table — so its `delete from runs` silently failed on the table's
`REFERENCES runs(run_id)` constraint (swallowed by the test's `let _ =`
error-ignoring pattern), leaking a `runs` row on every run and colliding
with the same deterministic `run_id` on the next invocation. Fixed by
adding the missing delete to that file's `cleanup()`, and purged the six
already-leaked rows from the local port-5434 test DB. All 9 of that file's
tests are green again, stable across three consecutive runs.

## Tests (`scenario_durable_paper_portfolio_read_only_api_01.rs`, 11 tests)

No-DB returns `"db_unavailable"` for all three durable routes; POST is
rejected (405) on all three; `durable-summary` reports
`"snapshot_unavailable"`/`"not_found"` with no durable data; populates
`realized_pnl` once accounting is `"complete"`; blocks `realized_pnl` (null,
with a reason) while still showing the position once accounting is
`"incomplete"`; `durable-positions` returns positions from the latest
snapshot and flags a >180s-old snapshot as `"snapshot_stale"`;
`durable-snapshots` respects `?limit=` and orders newest-first;
paper-lifecycle reports `"order_filled_portfolio_durable_pnl_available"`/
`"...incomplete"` once wired to real durable accounting; a dedicated
read-only proof calls every new route plus paper-lifecycle three times each
and asserts zero row-count delta across
snapshots/positions/accounting-state/outbox/inbox/runs.

All 11 pass on first run against the isolated port-5434 test DB.

## Verification

- `cargo check -p mqk-daemon`: clean.
- `cargo clippy -p mqk-daemon --lib -- -D warnings`: clean.
- `cargo clippy -p mqk-daemon --test scenario_durable_paper_portfolio_read_only_api_01 -- -D warnings`: clean.
- `cargo fmt --check` on every touched file: no diff.
- `cargo test -p mqk-daemon --lib`: 342/342 pass (includes the 8
  `classify_overall_lifecycle_state` unit tests).
- `cargo test -p mqk-daemon --test scenario_durable_paper_portfolio_read_only_api_01 -- --include-ignored --test-threads=1`: 11/11 pass.
- Regression check, all green: `scenario_paper_order_lifecycle_visibility_01`
  (13/13, `--test-threads=1` per its documented convention),
  `scenario_paper_daily_pnl_baseline_01` (11/11),
  `scenario_autonomous_completed_bar_driver_01` (56/56),
  `scenario_durable_paper_portfolio_snapshot_persistence_01` (9/9, after the
  cleanup-gap fix above, three consecutive runs),
  `scenario_durable_paper_portfolio_accounting_01` (13/13),
  `scenario_paper_pnl_operator_visibility_01` (13/13).
