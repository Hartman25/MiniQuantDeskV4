# Paper Daily P&L Baseline — Design Only

Patch group: `PAPER-PNL-OFFMARKET-COMPLETION-01-COMBINED`, Phase D.
**Design document only. No code. No schema migration. No route behavior
change. No baseline-capture implementation.** Builds nothing; this phase's
only artifact is this document (plus the ledger/roadmap-reconcile updates
that accompany it).

## 1. Current problem

`GET /api/v1/portfolio/summary.daily_pnl` is always `null`, with
`daily_pnl_unavailable_reason = "no_day_start_equity_baseline_in_schema"`
(`core-rs/crates/mqk-daemon/src/routes/portfolio.rs:38`,
`DAILY_PNL_REASON_NO_BASELINE`). Computing a truthful daily P&L requires
comparing *today's* account equity (or position value) against a
*day-start* or *previous-session-close* baseline. Repo-wide grep across
`mqk-db`, `mqk-daemon`, `mqk-portfolio`, `mqk-runtime`, and `docs` (this
phase, and repeated from the original `paper_pnl_operator_visibility_01a`
audit) finds no such baseline anywhere: `BrokerAccount` (`mqk-schemas`)
carries only a same-instant `equity`/`cash`/`currency` snapshot with no
prior-value field, and no table in `mqk-db`'s schema stores a dated
equity-at-a-point-in-time row.

## 2. Why `daily_pnl` must not be fabricated

CLAUDE.md's operator-truth-discipline invariant ("No fabricated truth. No
optimistic defaults.") and fail-closed invariant directly forbid
inventing a plausible-looking baseline (e.g. "assume today's first-seen
equity is day-start," silently, without persisting or labeling it as
such) — a silently-approximated number that *looks* like a real daily P&L
is worse than an honest `null`, because an operator cannot distinguish it
from a real one. Any baseline this system reports must be persisted,
timestamped, and provenance-tagged (which source produced it, and when)
so `pnl_truth_state`-style honesty can apply to it exactly as it already
does to `mark_price`/`unrealized_pnl`.

## 3. Candidate baseline sources

| Source | Description | Pros | Cons |
|---|---|---|---|
| **Day-start equity snapshot** | Daemon captures `BrokerAccount.equity` once at the first tick after a new trading-day boundary is detected, persists it keyed by date. | Simple; reuses the existing `broker_snapshot` polling/ack path; no new broker call. | Requires the daemon to be running exactly at day-start to capture it — a daemon that starts mid-day (or restarts) has no day-start snapshot for that day and must fail closed for that day, not backfill. |
| **Previous-session close** | Persist the *last* captured `BrokerAccount.equity` of each trading day as that day's close; the next day's `daily_pnl` baseline is the prior day's close. | Naturally rolls forward; works even if the daemon starts mid-day, as long as *some* prior day's close was captured. | First day ever run has no prior close (fail closed, same as day-start's mid-day-start problem, but only once instead of recurring). Requires an explicit "close" event, which paper trading (no real market close signal from the local synthetic broker) does not naturally emit. |
| **Broker account last-equity snapshot (if broker supports it)** | Some brokers expose their own "last equity" / "day P&L" field directly (Alpaca's account object has `last_equity`). Use the broker's own number instead of deriving one locally. | Zero local computation, zero local persistence; broker is definitionally the source of truth per CLAUDE.md's broker-is-authoritative invariant. | `mqk_schemas::BrokerAccount` does not currently carry this field for either the paper or Alpaca adapter — this option is itself a design decision (extend `BrokerAccount`), not a data source that already exists. Also broker-kind-dependent: the paper broker has no analogous concept unless deliberately added. |
| **Computed prior-day mark-to-market baseline** | Reconstruct day-start value from prior-day-close positions × prior-day-close marks (via `md_bars`) + cash, entirely from data already ingested. | No new capture timing dependency — can be computed retroactively from `md_bars` + a positions-history table, if one existed. | Requires a *positions-history* table (point-in-time position snapshots), which also does not exist today — this is a bigger prerequisite than the P&L baseline itself, and doubles the design surface. |

## 4. Recommended design

**Previous-session close**, persisted as an explicit, date-keyed,
provenance-tagged snapshot captured opportunistically whenever the daemon
observes a broker-snapshot equity value near/after each trading day's
close, using the existing `mqk-daemon` market-calendar/session-profile
seam (`core-rs/crates/mqk-daemon/src/state/market_calendar.rs` —
`MarketSessionState`, `ExchangeSourcedCalendarProvider`,
`classify_equity_us_regular_session`) to detect the close boundary rather
than a naive wall-clock guess. Rationale:

- It tolerates daemon restarts mid-day (the common case in this repo,
  per `session_boundary = "in_memory_only"` on every broker-snapshot
  surface) — `daily_pnl` for *today* is unavailable only if *no* prior
  close was ever captured, not every time the daemon restarts.
- It reuses the account-equity value already flowing through
  `broker_snapshot` (no new broker field, no new adapter-level
  dependency), keeping this decoupled from broker-kind (paper vs.
  Alpaca).
- It composes naturally with the existing `pnl_truth_state` /
  `*_unavailable_reason` honesty pattern this bundle's Phase B already
  extended: a missing baseline for *today specifically* (first day ever
  run, or a calendar gap) is a distinct, nameable truth state, not a
  crash or a silent zero.

The "day-start equity snapshot" alternative is not rejected outright — it
is a reasonable *addition* once "previous-close" exists (day-start ≈
yesterday's persisted close, cross-checked against a fresh capture at
today's open for drift detection) — but is not the minimum viable design.

## 5. Proposed schema (not implemented)

Possible table, modeled on the existing
`sys_broker_position_baseline` singleton/keyed-row precedent
(`core-rs/crates/mqk-db/src/broker_baseline.rs`) for provenance-tagging
conventions, but keyed per trading day rather than a single sentinel row:

```text
table: sys_account_equity_baseline
  trading_date         date        not null   -- exchange trading-day this baseline closes
  equity_micros         bigint     not null   -- BrokerAccount.equity at capture, in micros
  cash_micros            bigint    not null
  currency                text     not null
  captured_at_utc         timestamptz not null  -- caller-supplied, not DEFAULT now() (DB rule)
  captured_by             text     not null     -- e.g. "daemon:auto_close_capture" | "operator:manual"
  broker_snapshot_source  text     not null     -- "synthetic" | "external", mirrors PortfolioPositionsResponse.snapshot_source
  audit_event_id           uuid    not null     -- deterministic (UUIDv5), per audit_repo_truth_rules.md

  primary key (trading_date)
```

- **Unique key:** `trading_date` — one baseline row per trading day, so a
  re-capture on the same day is an idempotent upsert (matching DB rule:
  "every write path across restart or retry must be idempotent"),
  never a duplicate.
- **Retention:** no deletion policy proposed here — daily rows are small
  and low-volume (≤365/year); a retention/archival decision belongs to
  the implementation patch, not this design.
- **Proof fields:** `captured_at_utc`, `captured_by`,
  `broker_snapshot_source` make every row's provenance inspectable —
  exactly the "distinguish unavailable, empty, and present" discipline
  CLAUDE.md requires, applied to a historical row instead of a live one.
- **Provenance fields:** `audit_event_id` ties each baseline capture into
  the existing deterministic-audit-ID discipline
  (`.claude/rules/audit_repo_truth_rules.md`), so a baseline capture is
  itself an auditable event, not a silent side-effect.

This is a proposal for the *next* patch to evaluate and refine — no
migration file is added by this phase.

## 6. Proposed route semantics

`GET /api/v1/portfolio/summary.daily_pnl` / a new
`daily_pnl_truth_state` field (mirroring the existing
`pnl_truth_state` pattern) would report:

- `"active"` — a baseline row exists for the correct prior trading day;
  `daily_pnl = current_equity - baseline.equity_micros` is computed and
  populated.
- `"baseline_unavailable"` — no baseline row exists for the required
  prior trading day at all (e.g. first day this capture mechanism has
  ever run for this account).
- `"stale_baseline"` — a baseline row exists, but its `trading_date` is
  further in the past than the immediately preceding trading day (e.g. the
  daemon was down across a multi-day gap and no capture happened on the
  day that should have produced today's baseline) — reported as stale
  rather than silently used, consistent with the `live-weights` /
  `positions` "never fabricate, always name the reason" convention.
- `"no_snapshot"` — mirrors the existing `truth_state`/`pnl_truth_state`
  behavior when there is no current `broker_snapshot` at all.
- `"db_unavailable"` — no DB pool configured; baseline lookup was never
  attempted, mirroring `compute_broker_positions_pnl`'s existing
  `db_unavailable` truth state.

## 7. Proposed tests (for the implementation patch, not written here)

- Baseline capture is idempotent: capturing twice for the same
  `trading_date` upserts, not duplicates (restart-safety proof per
  CLAUDE.md).
- No baseline row for any prior day → `daily_pnl = null`,
  `daily_pnl_truth_state = "baseline_unavailable"`.
- Baseline row present for yesterday, current equity available →
  `daily_pnl` computed correctly (DB-backed, seeded-row test, matching the
  `PPV-05`/`PPV-06` seeded-bar pattern this bundle already uses).
- Baseline row present but its `trading_date` is stale (>1 trading day
  old, using the existing `market_calendar` seam to determine "the
  immediately preceding trading day") → `"stale_baseline"`, not silently
  used.
- No DB pool configured → `"db_unavailable"`, matching the existing
  `compute_broker_positions_pnl` DB-absent behavior.
- Weekend/holiday: capturing on a non-trading day must not create a
  `trading_date` row for that non-trading day (uses
  `ExchangeSourcedCalendarProvider`/`NyseWeekdaysProvider` to determine
  "is this a trading day" before capture, not a raw wall-clock date).

## 8. Migration safety plan

- New migration is strictly append-only and additive: one new table
  (`sys_account_equity_baseline`), no changes to any existing table or
  column — satisfies the DB rules' append-only/no-renumbering invariant
  trivially since nothing existing is touched.
  - Per DB rules: no `DEFAULT now()` / no `DEFAULT gen_random_uuid()` —
    every row's `captured_at_utc` and `audit_event_id` must be
    caller-supplied at insert time, not DB-generated.
- Migration is safe to re-run (idempotent `CREATE TABLE IF NOT EXISTS`),
  matching the existing migration-idempotency rule.
- No backfill of historical baselines is proposed — the table starts
  empty; `daily_pnl` for a given day only becomes available once a real
  capture has occurred for the prior trading day. No synthetic/fabricated
  historical baseline is ever inserted to "seed" the table.

## 9. Interaction with market sessions / weekends / holidays

The capture mechanism must key off the existing trading-day / session
infrastructure already built for `ASSET-CORE-05` (`market_calendar.rs`:
`MarketSessionState`, `ExchangeSourcedCalendarProvider`,
`NyseWeekdaysProvider`, `classify_equity_us_regular_session`) rather than
inventing a second, parallel notion of "day":

- A capture attempt on a weekend/holiday (non-trading day) must not
  create a `trading_date` row for that date — `trading_date` is always an
  actual exchange trading day.
- After a weekend or holiday gap, "yesterday's baseline" means the most
  recent actual trading day's close, not the literal calendar-yesterday —
  this is exactly what `trading_date`-keyed rows (§5) with an explicit
  "immediately preceding trading day" lookup (rather than
  `today - 1 day`) are for.
- A multi-day outage (daemon down across a holiday weekend, or longer)
  produces a real gap in captured baselines; the route semantics (§6)
  must report `"stale_baseline"` or `"baseline_unavailable"` rather than
  silently comparing against a baseline several trading days old as if it
  were yesterday's.

## 10. Why this should be a separate future patch

This design touches a new table, a new capture-timing mechanism tied to
market-session detection, and new route-level truth-state vocabulary —
three separate pieces of non-trivial, testable surface. Bundling it into
this off-market completion patch would violate CLAUDE.md's
one-patch-per-turn / minimal-scope discipline, and this bundle's own
explicit instruction not to implement daily-P&L baseline capture. The
`timeframe` query-param fix (Phases A–C of this bundle) is a narrow,
low-risk route change; a baseline-capture mechanism is a new durable-state
subsystem and deserves its own dedicated patch group with its own
phase-by-phase scenario-test proof.

## 11. Recommended future patch ID

```text
PAPER-DAILY-PNL-BASELINE-01-COMBINED
```
