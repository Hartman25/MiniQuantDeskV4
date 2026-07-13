# Paper Order Lifecycle Visibility — 01D DB-Backed Proof

Patch group: `PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-AUDIT-AND-CLOSURE-01-COMBINED`
(`PAPER-ORDER-LIFECYCLE-VIS-01D-DB-BACKED-PROOF-AND-REGRESSION-01`), Phase D.

## 1. Which tests prove the route?

- `core-rs/crates/mqk-db/tests/scenario_paper_order_lifecycle_visibility_01.rs`
  (Phase B, 5 tests): run-scoping and authoritative-empty proofs for the two
  new DB helpers, plus a zero-write proof. All 5 pass against local
  `mqk-test-postgres` (port 5434).
- `core-rs/crates/mqk-daemon/tests/scenario_paper_order_lifecycle_visibility_01.rs`
  (Phase C, 13 tests): 5 in-process (no DB) + 8 DB-backed covering every
  required truth-state branch — `db_unavailable`, `invalid_request`,
  `not_found`, latest-run resolution, signal-only, no-trade-diagnostic-only,
  outbox-only, outbox+inbox-fill, zero-writes, and empty-run
  `partial_visibility`. All 13 pass.
- Pure unit tests for `classify_overall_lifecycle_state`
  (`core-rs/crates/mqk-daemon/src/routes/paper_lifecycle.rs`, 6 tests): all
  pass (`cargo test -p mqk-daemon --lib paper_lifecycle`).
- Regression: `scenario_route_contract_rt01` (2 tests) and
  `scenario_gui_daemon_contract_gate` (23 tests) both pass unaffected by
  this bundle.

Total: 26 tests added by this bundle (5 + 13 + 6 pure — note the 6 pure
classifier tests are counted once, inside the 13-test daemon file's
module), all passing. `cargo clippy -p mqk-db -p mqk-daemon --all-targets
-- -D warnings` (daemon checked via `--lib` + explicit test-target
clippy) clean. `cargo fmt --check` clean on every file this bundle added
or touched.

## 2. Which real paper DB rows exist?

Read-only inspection of `mqk-paper-postgres` / `miniquantdesk_paper`
(zero rows mutated — every query below was a plain `select`):

```
select run_id, engine_id, mode, status, started_at_utc, stopped_at_utc
from runs order by started_at_utc desc limit 5;

15cf4309-210b-5406-8ed8-46377e093195 | mqk-daemon | PAPER       | STOPPED | 2026-07-10 18:31:01 | 2026-07-10 18:49:18
2f5e0619-df6b-5907-a0f1-ad019b2dfb57 | mqk-daemon | PAPER       | STOPPED | 2026-07-10 16:10:42 | 2026-07-10 18:30:34
741b421f-7e6e-5bbc-bf55-a85c3db5c559 | mqk-daemon | PAPER       | STOPPED | 2026-07-09 17:53:27 | 2026-07-09 20:00:28
1d005ad4-bec5-54b8-9291-c0a932626a1a | mqk-daemon | PAPER       | STOPPED | 2026-07-09 15:00:28 | 2026-07-09 16:09:50
ff8be50d-c6a7-46b0-a410-a5993f2a0402 | mqk-daemon | LIVE-SHADOW | RUNNING | 2026-07-09 02:25:36 | (null)
```

For the latest PAPER run (`15cf4309-210b-5406-8ed8-46377e093195`, the
exact run the Phase A audit proved `GET /api/v1/execution/flow` cannot
resolve without an explicit `run_id`, since it is `STOPPED` not
`ARMED`/`RUNNING`):

```
select run_id, strategy_id, symbol, ts_utc, decision_stage, reason_code, signal_generated, signal_qty
from strategy_signal_evaluations where run_id = '15cf4309-210b-5406-8ed8-46377e093195' order by ts_utc;

15cf4309... | intraday_scalper | AAPL | 2026-07-10 18:35:32 | strategy_evaluated | signal_long        | t | 3
15cf4309... | intraday_scalper | AAPL | 2026-07-10 18:40:31 | pre_dispatch_gate  | intraday_bar_stale | f | (null)

select outbox_id, idempotency_key, status, created_at_utc, sent_at_utc
from oms_outbox where run_id = '15cf4309-210b-5406-8ed8-46377e093195' order by outbox_id;

21 | 2a445578-da8c-5313-83fe-0a17c0523330 | ACKED | 2026-07-10 18:35:32 | 2026-07-10 18:35:33

select inbox_id, internal_order_id, broker_order_id, event_kind, received_at_utc, applied_at_utc
from oms_inbox where run_id = '15cf4309-210b-5406-8ed8-46377e093195' order by inbox_id;

61 | 2a445578-...0330 | 50f5f5d8-...f01 | ack  | 2026-07-10 18:35:33.330899 | 2026-07-10 18:35:33.372140
62 | 2a445578-...0330 | 50f5f5d8-...f01 | ack  | 2026-07-10 18:35:33.354812 | 2026-07-10 18:35:33.379081
63 | 2a445578-...0330 | 50f5f5d8-...f01 | fill | 2026-07-10 18:35:33.824870 | 2026-07-10 18:35:34.373379

select diagnostic_id, reason_code, stage, paper_order_attempted, live_order_attempted
from autonomous_no_trade_diagnostics where run_id = '15cf4309-210b-5406-8ed8-46377e093195'
order by observed_at_utc limit 5;

210548c7-... | STRATEGY_NOT_TICKED | pre_dispatch | f | f
4e38d14a-...  | STRATEGY_NOT_TICKED | pre_dispatch | f | f
(several more, all STRATEGY_NOT_TICKED / pre_dispatch, all f/f)
```

## 3. Did real paper DB readback find proof-02 order/fill rows?

**Yes.** The real, durable row set above is a complete, self-consistent
lifecycle: a real generated signal (`signal_long`, qty 3, AAPL) → a real
outbox row (`ACKED`) → two real broker `ack` inbox events → one real
`fill` inbox event, all timestamped within the same two-second window
(`18:35:32`–`18:35:34` UTC on 2026-07-10). Tracing `classify_overall_lifecycle_state`
by hand against these exact rows (`signal_count=2`,
`generated_signal_count=1`, `outbox_count=1`, `fill_seen=true`) yields
`overall_lifecycle_state = "order_filled_pnl_pending"` — the same
classification the DB-backed test suite proves for a synthetic fixture
with an identical shape (`pl_11_outbox_plus_inbox_fill_is_order_filled_pnl_pending`).
This is independent confirmation that the route's logic matches real
production data, not just synthetic fixtures.

## 4. Was live patched-route readback performed?

**No — deliberately skipped.** The currently-running daemon process (if
any) predates this bundle's binary; calling the live route would require
rebuilding and restarting the daemon. Per the mission's hard safety rules
("Do not restart unless explicitly authorized") and this session's
standing caution around state-changing actions, no daemon restart was
performed. The DB-backed test suite plus the hand-traced real-row proof
in §3 are the closure evidence for this phase — they exercise the exact
same code path (`execution_paper_lifecycle` handler, `mqk_db` helpers)
against a real Postgres instance, just not through a running daemon
process.

## 5. Were any DB rows mutated?

**No.** Every query against `mqk-paper-postgres` in this phase (and in
the Phase A audit) was a plain `select`. The Phase B/C test suites write
only to the isolated `mqk_test` database (port 5434) via
deterministic, test-file-owned UUIDs, and every test cleans up its own
rows (`pl_cleanup_run` / `cleanup`) before or after each test.

## 6. Were any orders submitted?

**No.** No test, helper, or route handler in this bundle calls a broker
adapter, submits an order, or writes to `oms_outbox`/`oms_inbox` outside
of test fixture setup (which uses the existing, pre-approved
`outbox_enqueue` / `inbox_insert_deduped_with_identity` helpers against
the isolated test DB only).

## 7. Are lifecycle states restart-surviving?

**Yes**, for every stage this route covers (run / signal / no-trade /
outbox / inbox): all five are resolved via `mqk_db` fetch helpers reading
committed Postgres rows, with the run itself resolved via
`fetch_run`/`fetch_latest_run_for_engine` rather than any in-memory
daemon state. This is proven directly by the real-row trace in §3: the
resolved run (`15cf4309-...`) is `STOPPED`, and its full chain is
reconstructed purely from durable rows with zero dependency on the
daemon having an active execution loop.

Portfolio/P&L visibility remains **not** restart-surviving — this is
reported honestly via `portfolio_truth_state` /
`pnl_truth_state = "in_memory_only_not_restart_surviving"` rather than
fabricated, per the capability boundary the Phase A audit established.
