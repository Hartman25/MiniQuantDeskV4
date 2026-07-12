# Paper Daily P&L Baseline — 01E Closure Decision

Patch group: `PAPER-DAILY-PNL-BASELINE-01-COMBINED`, Phase E.

## 1. Is `PAPER-DAILY-PNL-BASELINE-01-COMBINED` closed?

```text
PAPER-DAILY-PNL-BASELINE-01-COMBINED: PARTIAL / BASELINE-SCHEMA-AND-READ-SIDE-CLOSED-CAPTURE-SEAM-OPEN
```

Schema, DB helpers, and route read-side visibility are `CLOSED_LOCAL` and
proven against a real local Postgres. Baseline capture is explicitly
**not** built — this bundle was scoped, from Phase A onward, to schema +
read-side only (see Phase D's `paper_daily_pnl_baseline_01d_capture_boundary_decision.md`).
`daily_pnl` will read `"baseline_unavailable"` on a real running daemon
until `PAPER-DAILY-PNL-BASELINE-CAPTURE-01-COMBINED` adds a real capture
mechanism.

## 2. What schema/table was added?

`sys_account_equity_baseline` (migration
`core-rs/crates/mqk-db/migrations/0045_account_equity_baseline.sql`),
primary key `trading_date date`, plus `equity_micros bigint`,
`cash_micros bigint`, `currency text`, `captured_at_utc timestamptz`,
`captured_by text`, `broker_snapshot_source text`, `audit_event_id uuid`
— all `not null`, no `DEFAULT now()`/`DEFAULT gen_random_uuid()`. Confirmed
live in the local paper Postgres (`mqk-paper-postgres`, port 5440) via
read-only `information_schema.columns` query — see §11.

## 3. What DB helpers were added?

`core-rs/crates/mqk-db/src/account_equity_baseline.rs`:
`AccountEquityBaselineRecord`, `UpsertAccountEquityBaselineArgs`,
`upsert_account_equity_baseline` (idempotent upsert-by-`trading_date`,
last-write-wins per date), `fetch_account_equity_baseline_for_date`
(returns `None` when absent). Registered in `mqk-db/src/lib.rs`.

## 4. What route fields were added?

On `PortfolioSummaryResponse`
(`core-rs/crates/mqk-daemon/src/api_types.rs`): `daily_pnl_truth_state:
String`, `daily_pnl_baseline_trading_date: Option<String>`,
`daily_pnl_baseline_equity: Option<f64>`, `daily_pnl_baseline_source:
Option<String>`, `daily_pnl_baseline_captured_at_utc: Option<String>`.
Existing `daily_pnl: Option<f64>` and `daily_pnl_unavailable_reason:
Option<String>` fields are reused, not duplicated.

## 5. When does `daily_pnl` compute?

Only when `daily_pnl_truth_state == "active"`: a broker snapshot exists,
a DB pool is configured, and a baseline row exists in
`sys_account_equity_baseline` for the exact required prior trading day
(the most recent actual NYSE trading day before now, found via
`NyseWeekdaysProvider`). `daily_pnl = current_account_equity -
baseline.equity_micros`.

## 6. When does it stay unavailable?

- No broker snapshot: `"no_snapshot"`.
- No DB pool configured: `"db_unavailable"`.
- No baseline row for the required date, and no older row found either
  (within a bounded 30-calendar-day lookback): `"baseline_unavailable"`.
- A baseline row exists but only for a date older than the required prior
  trading day: `"stale_baseline"` — reported, never silently used as if
  correct.

## 7. Was any baseline fabricated?

No. Every baseline row in every test was written through the same
`upsert_account_equity_baseline` helper the (future) production capture
path will use, with explicit caller-supplied values. The route never
inserts, updates, or deletes a baseline row — proven by `PDB-08`
(`scenario_paper_daily_pnl_baseline_01.rs`) via an unchanged row count and
unchanged row content across repeated route calls.

## 8. Was any historical baseline backfilled?

No. The table starts, and remains, empty in the real paper DB (§11 — zero
rows). No migration or code path inserts a seed/placeholder row.

## 9. Was baseline capture implemented, or deferred?

Deferred. See §2 of `paper_daily_pnl_baseline_01d_capture_boundary_decision.md`.

## 10. If capture was deferred, exact future patch ID

```text
PAPER-DAILY-PNL-BASELINE-CAPTURE-01-COMBINED
```

## 11. Was any DB migration added?

Yes — `0045_account_equity_baseline.sql`, appended to `manifest.json` as
id `0045`. Confirmed live in the local paper Postgres by read-only query
(Phase E pre-flight, this session):

```text
table_name                   | column_name             | data_type
sys_account_equity_baseline  | trading_date            | date
sys_account_equity_baseline  | equity_micros           | bigint
sys_account_equity_baseline  | cash_micros             | bigint
sys_account_equity_baseline  | currency                | text
sys_account_equity_baseline  | captured_at_utc         | timestamp with time zone
sys_account_equity_baseline  | captured_by             | text
sys_account_equity_baseline  | broker_snapshot_source  | text
sys_account_equity_baseline  | audit_event_id          | uuid
```

`select * from sys_account_equity_baseline` returned 0 rows — no capture
mechanism has ever run against this database, and all test-seeded rows
were cleaned up by their own tests.

## 12. Were any provider/broker/network calls made in tests?

No. All DB-backed tests connect only to the local `mqk-test-postgres`
(port 5434) or `mqk-paper-postgres` (port 5440) containers already running
on this machine — no provider or broker adapter is constructed or called
by any test in this patch group.

## 13. Were any orders submitted?

No. No order-related code path is touched anywhere in this patch group.
`PDB-09`/`PPV-09`-style zero-write proofs cover the outbox/baseline
tables; no test constructs a runtime, orchestrator, or broker adapter.

## 14. Were any thresholds/gates/config changed?

No. No strategy, risk-gate, freshness-gate, session-gate, or config-flag
code was touched. No `.env.local` edit.

## 15. What exact next market-hours proof should be run?

```text
PAPER-TRADE-LIFECYCLE-PROOF-03-PNL-VISIBILITY-VERIFY-COMBINED
```

Rebuild and restart the daemon with this bundle's binary and call
`GET /api/v1/portfolio/summary` during market hours with a real paper
position; confirm `unrealized_pnl` remains as previously proven and
`daily_pnl_truth_state` reads `"baseline_unavailable"` (expected, since no
capture mechanism exists yet) rather than crashing or fabricating a value.

## 16. What exact next off-market patch is recommended?

```text
PAPER-DAILY-PNL-BASELINE-CAPTURE-01-COMBINED
```

Build the actual capture mechanism (CLI-first, market-calendar-gated,
idempotent-by-`trading_date`, provenance-tagged) so `daily_pnl` can reach
`"active"` on a real running daemon.
