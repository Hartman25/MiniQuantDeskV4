# Paper Daily P&L Baseline — 01A Current Truth Reconcile

Patch group: `PAPER-DAILY-PNL-BASELINE-01-COMBINED`, Phase A.

This document reconciles `docs/specs/paper_daily_pnl_baseline_design_only_01.md`
(the prior design-only doc, `PAPER-PNL-OFFMARKET-01D`) against current repo
truth at HEAD, and locks the exact implementation scope for Phases B–E of
this patch group. No code, schema, or route behavior changes in this phase.

## 1. Current HEAD

```text
8ae16b3c docs: close paper pnl offmarket completion
```

Branch `main`. Working tree clean (no dirty tracked files, no staged
files); only the allowed untracked `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`
and `smoke_logs/` present.

## 2. Current `daily_pnl` behavior and route fields

`GET /api/v1/portfolio/summary` (`core-rs/crates/mqk-daemon/src/routes/portfolio.rs`)
always returns `daily_pnl: None`:

- Snapshot present (`portfolio.rs:271-272`): `daily_pnl = None`,
  `daily_pnl_unavailable_reason = Some("no_day_start_equity_baseline_in_schema")`
  (constant `DAILY_PNL_REASON_NO_BASELINE`, `portfolio.rs:38`).
- No snapshot (`portfolio.rs:286-287`): `daily_pnl = None`,
  `daily_pnl_unavailable_reason = Some("no_broker_snapshot")`
  (constant `DAILY_PNL_REASON_NO_SNAPSHOT`, `portfolio.rs:39`).

`PortfolioSummaryResponse` (`core-rs/crates/mqk-daemon/src/api_types.rs:939-977`)
already carries `daily_pnl: Option<f64>` and `daily_pnl_unavailable_reason:
Option<String>`, but no `daily_pnl_truth_state` or baseline-provenance
fields — those fields do not exist today.

`RiskSummaryResponse.daily_pnl` (`api_types.rs:985`) and
`oms_metrics.rs:249`'s `daily_pnl: None` are separate, unrelated
always-`None` fields (risk-summary and OMS-metrics surfaces respectively)
and are out of scope for this patch — only `PortfolioSummaryResponse` is
touched.

## 3. Existing baseline table/helper — confirmed absent

Repo-wide grep at this HEAD confirms:

- **No account-equity baseline table exists.** The only baseline-shaped
  table is `sys_broker_position_baseline`
  (`core-rs/crates/mqk-db/migrations/0039_broker_position_baseline.sql`,
  helper `core-rs/crates/mqk-db/src/broker_baseline.rs`) — a **singleton**
  (`sentinel_id = 1` CHECK constraint) storing an operator-adopted broker
  *position* snapshot for reconcile-at-startup, not a dated equity value.
  It is a useful **pattern to mirror** (caller-supplied timestamp/UUID,
  upsert-by-key, no `DEFAULT now()`/`DEFAULT gen_random_uuid()`), not a
  reusable table.
- **No DB helper for daily or previous-close equity baseline exists.**
  `mqk-db/src/lib.rs` registers `broker_baseline`, `alert_acks`,
  `arm_state`, `audit`, `fill_quality`, `flow`, `inbox`, `incidents`,
  `order_lifecycle`, `orders`, `reconcile_state`, `restart_intent`, `runs`,
  `strategy`, `md` — none of these model a per-trading-day equity value.
- `mqk_schemas::BrokerAccount` (`core-rs/crates/mqk-schemas/src/lib.rs:63-68`)
  carries only a same-instant `equity`/`cash`/`currency` snapshot, no
  prior-value field.

This confirms the design-only doc's §1 finding is still accurate at this
HEAD: `PAPER-DAILY-PNL-BASELINE-01-COMBINED` starts from zero existing
baseline infrastructure.

## 4. Chosen design (locked)

**Previous-session-close equity baseline**, exactly as recommended in
`paper_daily_pnl_baseline_design_only_01.md` §4:

- Persisted as a `sys_account_equity_baseline` row **per trading date**
  (not a singleton), keyed by `trading_date`.
- Provenance-tagged: `captured_at_utc`, `captured_by`,
  `broker_snapshot_source`, `audit_event_id` (deterministic, UUIDv5 —
  `.claude/rules/audit_repo_truth_rules.md`).
- Fail-closed: `/api/v1/portfolio/summary` reports an explicit
  `daily_pnl_truth_state` and never computes `daily_pnl` from a missing,
  stale, or fabricated baseline.

## 5. Exact table/schema plan

New migration `0045_account_equity_baseline.sql` (next sequential number —
`0044_autonomous_no_trade_diagnostics.sql` is the current tip):

```sql
create table if not exists sys_account_equity_baseline (
    trading_date date primary key,
    equity_micros bigint not null,
    cash_micros bigint not null,
    currency text not null,
    captured_at_utc timestamptz not null,
    captured_by text not null,
    broker_snapshot_source text not null,
    audit_event_id uuid not null
);
```

- Matches `docs/specs/paper_daily_pnl_baseline_design_only_01.md` §5
  exactly, using `create table if not exists` for migration-idempotency
  (DB rule) instead of `0039`'s bare `create table` (that migration
  predates the idempotency rule's active enforcement on new migrations).
- No `DEFAULT now()`, no `DEFAULT gen_random_uuid()` — caller supplies
  `captured_at_utc` and `audit_event_id`.
- No existing table touched. Appended to `manifest.json` as id `0045`.

## 6. Exact DB helper plan

New file `core-rs/crates/mqk-db/src/account_equity_baseline.rs`, mirroring
`broker_baseline.rs`'s structure (caller-supplied timestamp/UUID,
`anyhow::Result`, `sqlx::PgPool`):

- `AccountEquityBaselineRecord` struct (trading_date, equity_micros,
  cash_micros, currency, captured_at_utc, captured_by,
  broker_snapshot_source, audit_event_id).
- `upsert_account_equity_baseline(pool, args) -> Result<AccountEquityBaselineRecord>`
  — `insert ... on conflict (trading_date) do update set ...`, mirroring
  `upsert_broker_position_baseline`'s `on conflict` shape but keyed by
  `trading_date` instead of the `sentinel_id = 1` singleton key.
- `fetch_account_equity_baseline_for_date(pool, trading_date) -> Result<Option<AccountEquityBaselineRecord>>`.
- Registered as `pub mod account_equity_baseline;` +
  `pub use account_equity_baseline::*;` in `mqk-db/src/lib.rs`, alongside
  the existing `broker_baseline` registration.

## 7. Exact route field plan

Additive fields on `PortfolioSummaryResponse`
(`core-rs/crates/mqk-daemon/src/api_types.rs`), reusing the existing
`daily_pnl` / `daily_pnl_unavailable_reason` fields rather than
duplicating them:

- `daily_pnl_truth_state: String` — new, mirrors the existing
  `pnl_truth_state` pattern already proven for unrealized P&L.
- `daily_pnl_baseline_trading_date: Option<String>` — new (`YYYY-MM-DD`).
- `daily_pnl_baseline_equity: Option<f64>` — new.
- `daily_pnl_baseline_source: Option<String>` — new (`captured_by` value).
- `daily_pnl_baseline_captured_at_utc: Option<String>` — new (RFC3339).

Truth states (Phase C, matching design doc §6): `"active"`,
`"baseline_unavailable"`, `"stale_baseline"`, `"no_snapshot"`,
`"db_unavailable"`.

## 8. Exact capture-seam plan

**Decision: no automatic or CLI capture implemented in this patch.**
Phase B/C implement only the schema, the DB upsert/fetch helpers (used
directly by DB-backed tests to seed rows), and the route read-side.
Phase D is docs-only, formally deferring the capture mechanism.

Rationale:

- `docs/specs/paper_daily_pnl_baseline_design_only_01.md` §10 already
  concluded the capture mechanism (new table + new market-session-timed
  capture trigger + new route truth-state vocabulary) is three
  non-trivial, independently testable pieces of surface, and recommended
  keeping it out of a single bundled patch.
- `mqk-cli` uses a `clap` `Subcommand` structure
  (`core-rs/crates/mqk-cli/src/main.rs`) that could host a
  `capture-equity-baseline` command, but doing so correctly requires
  wiring a DB pool, a broker-snapshot source, and market-calendar
  trading-day validation into the CLI binary — real new scope, not a
  small addition.
- CLAUDE.md's one-patch-per-turn / minimal-scope discipline governs: this
  patch's honest deliverable is "baseline schema + read-side visibility,"
  not "capture." Bundling capture in would risk an unproven, rushed
  write-path seam touching durable state on the very invariant
  (fail-closed truth) this patch exists to strengthen.

Future patch: `PAPER-DAILY-PNL-BASELINE-CAPTURE-01-COMBINED`.

## 9. Explicit non-goals

- No fabricated, guessed, inferred, or silently-approximated baseline.
- No provider, broker, or network calls in any test — DB-backed tests use
  seeded fixture rows only.
- No order submission, no live routing, no execution arming.
- No forced paper orders, no manually submitted paper orders.
- No strategy, gate, or config threshold changes.
- No historical baseline backfill — the table starts empty; `daily_pnl`
  stays unavailable until a real capture occurs for the required prior
  trading day (and this patch does not implement that capture).

## 10. Expected final status if implemented successfully

```text
PAPER-DAILY-PNL-BASELINE-01-COMBINED: PARTIAL / BASELINE-SCHEMA-AND-READ-SIDE-CLOSED-CAPTURE-SEAM-OPEN
```

`daily_pnl` becomes computable only once a baseline row exists for the
required prior trading day (via a future capture patch or a manually
seeded test fixture); until then it remains honestly `null` with an
explicit `daily_pnl_truth_state`.
