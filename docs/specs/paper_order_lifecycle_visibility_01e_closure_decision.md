# Paper Order Lifecycle Visibility — 01E Closure Decision

Patch group: `PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-AUDIT-AND-CLOSURE-01-COMBINED`
(`PAPER-ORDER-LIFECYCLE-VIS-01E-CLOSURE-ROADMAP-AND-NEXT-PROOF-01`), Phase E.

## 1. Is `PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-AUDIT-AND-CLOSURE-01-COMBINED` closed?

**Yes — `CLOSED_LOCAL`**, for the scope this bundle set out to close: a
durable, restart-surviving reconstruction of a paper run's
signal/no-trade/outbox/inbox chain, by `run_id` or by durably-resolved
latest run. Portfolio/P&L visibility is explicitly **not** closed by this
bundle (see §6) and is reported as an honest capability boundary, not a
silent gap.

## 2. What route was added or proven sufficient?

Added: `GET /api/v1/execution/paper-lifecycle?run_id=<uuid>`
(`core-rs/crates/mqk-daemon/src/routes/paper_lifecycle.rs`). No existing
route fully closed this — Phase A confirmed `GET /api/v1/execution/flow`
covers only outbox+lifecycle-events+fills (not signal evaluations or
no-trade diagnostics) and falls back to in-memory active-run resolution
when no `run_id` is given.

## 3. Is it DB-backed and restart-surviving?

Yes, for every stage it covers. Every field is sourced via `mqk_db` fetch
helpers reading committed Postgres rows. Directly proven in Phase D
against a real `STOPPED` (non-active) paper run.

## 4. Does it work by provided `run_id`?

Yes — `mqk_db::fetch_run(pool, run_id)`, 400 on a malformed UUID,
`not_found` on a well-formed UUID with no matching row.

## 5. Does it work for latest run?

Yes — `mqk_db::fetch_latest_run_for_engine(pool, "mqk-daemon", "PAPER")`
when `run_id` is omitted, proven by `pl_07_no_run_id_resolves_latest_paper_run`.
This is durable resolution: it does not require the run to be
ARMED/RUNNING, closing the exact gap Phase A identified in `execution/flow`.

## 6. Which tables does it join/read?

`runs`, `strategy_signal_evaluations` (via new run-scoped
`fetch_strategy_signal_evaluations_for_run`), `autonomous_no_trade_diagnostics`
(via new run-scoped `fetch_autonomous_no_trade_diagnostics_for_run`),
`oms_outbox` (via existing `outbox_fetch_for_supervisor`), `oms_inbox`
(via existing `inbox_load_all_applied_for_run` +
`inbox_load_unapplied_for_run`). It does **not** read any
portfolio/position/accounting table — none exists durably in the repo
(confirmed in Phase A); `portfolio_truth_state` and `pnl_truth_state` are
therefore always reported as `"in_memory_only_not_restart_surviving"`
rather than fabricated.

## 7. What lifecycle states can it return?

Route-level `truth_state`: `db_unavailable`, `invalid_request`,
`not_found`, `no_rows`, `active`. When `active`,
`lifecycle_summary.overall_lifecycle_state`: `order_filled_pnl_pending`,
`order_rejected_or_failed`, `order_submitted_fill_pending`,
`no_signal_durably_explained`, `partial_visibility`.

## 8. Does it prove full lifecycle visibility for a filled order in tests?

Yes — `pl_11_outbox_plus_inbox_fill_is_order_filled_pnl_pending`
(synthetic fixture) plus the Phase D hand-trace against the real
`15cf4309-...` run's real fill rows, both agreeing on
`order_filled_pnl_pending`.

## 9. Did real paper DB readback find proof-02 rows?

Yes — see Phase D doc §2–3. The latest real PAPER run
(`15cf4309-210b-5406-8ed8-46377e093195`) has a complete real
signal→outbox→inbox-ack→inbox-fill chain for a real AAPL order, matching
the shape `PAPER-TRADE-LIFECYCLE-PROOF-02` describes. This bundle did not
re-verify the market-hours narrative around that trade (no live session
available off-market) — it verified the durable row shape independently.

## 10. Was any migration added?

No. Confirmed in Phase A and unchanged through Phase D: every table this
route reads already existed before this bundle.

## 11. Were any DB writes made outside test fixtures?

No. Every write in this bundle's test suites is a test-owned fixture row
(deterministic UUIDs, isolated `mqk_test` DB port 5434), cleaned up by
each test. Phase D's real paper-DB inspection was `select`-only.

## 12. Were any provider/broker/network calls made in tests?

No. Zero network calls anywhere in this bundle's code or tests.

## 13. Were any live/paper orders attempted?

No. No test or route handler in this bundle submits, cancels, or
replaces an order.

## 14. Were any thresholds/gates/config changed?

No. No strategy, risk, gate, or config-flag logic was touched anywhere in
this bundle.

## 15. What exact next market-hours proof should be run?

`PAPER-TRADE-LIFECYCLE-PROOF-03-PNL-VISIBILITY-VERIFY-COMBINED` (unchanged
recommendation from the prior bundle) — during market hours, confirm
`GET /api/v1/portfolio/summary` P&L visibility end-to-end with a live
paper position. This new route does not change that recommendation; it
adds a durable lens onto the OMS-side chain that recommendation already
depends on for order-truth cross-checking.

## 16. What exact next off-market completion bundle is recommended?

`STRATEGY-LAB-COMPLETION-AND-SCANNER-FOUNDATION-01-COMBINED` — this
bundle found the OMS-side lifecycle-visibility gap fully closable without
a migration and closed it; it did not surface a further specific
order-lifecycle seam requiring an immediate follow-up bundle. (Portfolio/
P&L durable visibility remains a known, honestly-labeled gap — see §6 —
but building it would mean a new durable position-ledger design, which is
a materially larger scope than a `PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-02`
follow-up; it is not recommended as the immediate next off-market bundle.)

Final status:

```text
PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-AUDIT-AND-CLOSURE-01-COMBINED: CLOSED_LOCAL
PAPER-TRADE-LIFECYCLE-PROOF-02: LIFECYCLE-PERSISTENT-VISIBILITY-CLOSED
PAPER-DAILY-PNL-BASELINE-CAPTURE-AND-OPERATOR-CLOSURE-01-COMBINED: CLOSED_LOCAL (unchanged)
PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED: DAILY-PNL-CAPTURE-AND-READ-SIDE-CLOSED (unchanged)
```
