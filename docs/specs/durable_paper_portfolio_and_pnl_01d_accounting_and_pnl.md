# DURABLE-PAPER-PORTFOLIO-AND-PNL-01D — Durable Fill Accounting and P&L Truth

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01D-DURABLE-ACCOUNTING-AND-PNL`
Closes the durable fill-to-position-to-P&L accounting chain on top of B4-B's
`sys_paper_portfolio_accounting_state` and B4-C's canonical acceptance seam.

## FIFO authority — reused, not reimplemented

`state/paper_portfolio_accounting.rs::replay_paper_portfolio_accounting`
reuses `state::snapshot::recover_oms_and_portfolio` directly rather than
writing a second replay loop. This matters for correctness, not just style:
`recover_oms_and_portfolio` already applies the exact duplicate-fill guard
the live apply path uses — it builds an `OmsOrder` per submitted
(`SENT`/`ACKED`) outbox row and runs every applied inbox event through that
order's state machine (`order.apply(&oms_event, Some(&economic_event_id))`),
skipping the portfolio mutation whenever `filled_qty` didn't advance (a
duplicate economic event, e.g. the same fill delivered once via WS and once
via REST with different `broker_message_id`s). A second, simpler replay
that just iterated every applied `oms_inbox` row directly — as an early
draft of this patch did — would have silently double-counted that class of
duplicate. `mqk_portfolio`'s FIFO engine (`apply_fill`, called transitively)
is unmodified.

`cash_micros` in the durable accounting row is the **cumulative cash
movement produced by this run's fills** (computed with
`initial_cash_micros = 0`), not the absolute account cash balance — that
already lives on the durable snapshot (B4-C). `fees_micros` is summed from
the replayed ledger's `Fill` entries (not tracked as a running total inside
`mqk-portfolio` itself). `realized_pnl_micros` and open-lot quantities come
directly off the replayed `PortfolioState`.

## Accounting epoch — never fabricated

For every nonzero-quantity position in the caller-supplied broker position
list, the FIFO-replayed net quantity for that symbol must exactly match the
broker-reported quantity. Any mismatch — whether total absence of fill
history (a pre-existing, adopted position) or a partial mismatch (some
fills known, but not enough to explain the full reported quantity) —
marks `accounting_epoch = "incomplete"` with a specific reason string
(`pre_existing_position_no_matching_fill_history:{symbol}:broker_qty=...
:fill_history_derived_qty=...`). **No synthetic opening fill is ever
inserted or assumed** to force a match; `realized_pnl_micros` in that state
reflects only what the known fill history actually proves, not a
fabricated reconciliation.

## Production wiring

`persist_external_broker_snapshot_best_effort`'s sibling,
`refresh_paper_portfolio_accounting_state_best_effort`, is called from
inside `state::snapshot::accept_external_broker_snapshot` itself —
whenever a fresh authoritative snapshot is accepted (B4-C's three call
sites: run-start cold-fetch, periodic refresh, terminal-fill-expiry
refresh), the accounting projection is refreshed using that same
snapshot's `run_id` and `positions`. No new call site, no new polling loop:
snapshot acceptance and accounting refresh happen together, since both are
naturally driven by "a fresh authoritative broker truth just arrived."

Gating and failure handling mirror B4-C exactly: Paper + Alpaca only
(checked in the function itself), every failure (DB unavailable, replay
error, a `Rejected` watermark regression — which should not occur in normal
operation and is logged as a warning if it ever does) is logged via
`tracing::warn!` and swallowed, never blocking or reverting the snapshot
acceptance that triggered it.

## Daily P&L baseline — untouched

This patch adds zero code paths that read or write
`sys_account_equity_baseline`. The existing daily-P&L baseline capture
action, table, and `resolve_daily_pnl` reader are completely unmodified —
proven by `daily_pnl_baseline_table_untouched`, which asserts a zero row-count
delta on that table across a full accounting refresh.

## Unrealized P&L — explicitly out of scope for this patch

Unrealized P&L requires a mark price, which is not part of this durable
accounting projection. That responsibility stays with the existing
`compute_broker_positions_pnl` mark-lookup helper (`routes/portfolio.rs`,
sourced from `md_bars`) and is wired into the new read-only API surface in
B4-E, which reads both this table and that existing helper together. No
mark-related test exists in this patch's test file for that reason — adding
one here would test code that doesn't exist yet.

## Tests (`scenario_durable_paper_portfolio_accounting_01.rs`, 13 tests)

Every test drives the real production seam
(`AppState::accept_external_broker_snapshot_for_test`, which now also
triggers the accounting refresh) against real `oms_outbox`/`oms_inbox`
fixture rows built the same shape the daemon itself writes (a `SENT`
outbox row backing each order, `BrokerEvent::Fill`/`PartialFill` JSON
serialized exactly as the wire format, `inbox_mark_applied` stamped) — not
a shortcut that bypasses the durable data shape.

Buy opens a long position (zero realized P&L, cash reflects full cost);
second buy adds a FIFO lot; partial sell realizes a FIFO gain; partial sell
realizes a FIFO loss; fees reduce cash on both sides; full sell closes the
position; duplicate refresh (no new inbox rows) is a zero-delta no-op;
partial-fill-then-final-fill applies the exact total exactly once (not 6,
not 4, not 20); restart replay (a fresh `AppState` replaying the same
durable history) produces byte-identical state; a pre-existing unmatched
position blocks the accounting epoch without fabricating a fill; a partial
mismatch (not just total absence) is also caught; a flat portfolio has
known-zero truth (not merely absent); the daily-P&L baseline table is
untouched.

All 13 pass, stable across five consecutive default-parallel runs (no
cross-test race: every test uses its own deterministic `run_id` and cleans
up its own `oms_outbox`/`oms_inbox`/`sys_paper_portfolio_*` rows).

## Verification

- `cargo check -p mqk-daemon`: clean.
- `cargo clippy -p mqk-daemon --lib -- -D warnings`: clean.
- `cargo clippy -p mqk-daemon --test scenario_durable_paper_portfolio_accounting_01 -- -D warnings`: clean.
- `cargo fmt --check` on the touched files: no diff.
- `cargo test -p mqk-daemon --test scenario_durable_paper_portfolio_accounting_01`: 13/13 pass, five consecutive runs.
- Regression check: `scenario_autonomous_completed_bar_driver_01` (56/56),
  `scenario_paper_daily_pnl_baseline_01` (11/11),
  `scenario_durable_paper_portfolio_snapshot_persistence_01` (9/9) — all unchanged.
