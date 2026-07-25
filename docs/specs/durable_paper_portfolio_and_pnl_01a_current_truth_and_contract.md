# DURABLE-PAPER-PORTFOLIO-AND-PNL-01A — Current-Truth Audit and Binding Contract

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01A-CURRENT-TRUTH-AND-CONTRACT`
Scope: audit + design lock only. No production behavior change in this patch.
Supported lane: paper, Alpaca, single-symbol long-only US equity/ETF, supervised.

## 1. Source audit summary (current truth, with citations)

### 1.1 Broker snapshot acceptance — no single funnel today

`AppState.broker_snapshot: Arc<RwLock<Option<mqk_schemas::BrokerSnapshot>>>`
(`mqk-daemon/src/state.rs:293`) is written at five call sites:

1. `state/orchestrator_build.rs:146` — `BrokerSnapshotTruthSource::Synthetic`
   branch of `build_execution_orchestrator`: synthesizes a snapshot from
   local OMS/portfolio state at run-start when none exists yet.
2. `state/orchestrator_build.rs:189` — `BrokerSnapshotTruthSource::External`
   branch of the same function: a real, blocking Alpaca REST fetch at
   run-start (cold fetch).
3. `state/orchestrator_build.rs:396` — terminal-fill-expiry refresher closure,
   triggered by orchestrator tick Phase 0c; also `External`.
4. `routes/repair.rs:2920` — `POST /api/v1/ops/repair/adopt-broker-position-baseline`,
   on-demand real fetch when no snapshot exists and no run is active.
5. `routes/trading.rs:195`/`221` — dev-only injection/clear routes, gated by
   `MQK_DEV_ALLOW_SNAPSHOT_INJECT`; not real broker truth under any
   circumstance.

A sixth, periodic path exists in `state/loop_runner.rs` (`EXTERNAL_SNAPSHOT_REFRESH_TICKS`,
`state.rs:174`, ≈60s cadence) using the same `external_snapshot_refresher`/
`snapshot_fetcher` seam as (2)/(3).

**Real vs. synthesized is already distinguished** by a separate field,
`AppState.broker_snapshot_source: BrokerSnapshotTruthSource` (`Synthetic`/
`External`), echoed to routes as `"synthetic"`/`"external"` strings. Both
paths produce the same `mqk_schemas::BrokerSnapshot` struct
(`mqk-schemas/src/lib.rs:70-76`); `fills: Vec<BrokerFill>` on that struct is
**always empty** on both paths — `mqk-broker-alpaca/src/snapshot.rs`'s own
doc comment states fill delivery is a separate seam's job.

### 1.2 Fill authority — `oms_inbox`, already durable

Fills arrive exclusively via `oms_inbox` (`mqk-db/src/inbox.rs`), not
`orders.rs` (outbox/order-submission lifecycle only, zero fill content) and
not `order_lifecycle.rs` (cancel/replace-only, fills explicitly excluded by
its own header comment). `InboxRow` carries `event_kind` (`"fill"` /
`"partial_fill"` among others), `message_json` (deserializes to
`mqk_execution::BrokerEvent::{Fill,PartialFill}` — `symbol, side, delta_qty,
price_micros, fee_micros`), a durable monotonic `inbox_id` (ingest order),
and a nullable `broker_fill_id` (economic identity, distinct from the
`(run_id, broker_message_id)` transport-dedup key). `inbox_mark_applied`
gives crash-recovery journaling. `inbox_load_all_applied_for_run` and
`inbox_load_unapplied_for_run` already provide a complete, ordered
(`order by inbox_id asc`), replayable fill stream per run — including
partial fills.

**Conclusion: no new fill/accounting-event table is required.** `oms_inbox`
already is the durable, ordered, deduped, replayable fill ledger this bundle
needs. Introducing a second table that duplicates fill content would violate
`db_rules.md`'s "no write paths outside the established outbox/inbox/run
seams without proof" and create a second source of truth that could drift
from the first. What's missing is not fill data — it's a **durable
projection** of that data (an idempotently-applied FIFO accounting summary)
and a **durable snapshot** of the broker's own account/position truth.

### 1.3 FIFO accounting engine — already live, not on the daemon hot path

`mqk-portfolio` (`accounting.rs`, `ledger.rs`, `types.rs`) is a complete,
pure, deterministic FIFO engine: `apply_fill`/`apply_entry` (buy/sell FIFO
lot consumption, signed lots, realized P&L per fill), `recompute_from_ledger`
(full replay from an ordered fill list — the exact primitive a restart-safe
projection needs), and a typed `Ledger` façade with `verify_integrity`,
`unrealized_pnl_micros`. It is **already invoked** in `mqk-daemon`:
`state/snapshot.rs`'s `recover_oms_and_portfolio` replays applied inbox fills
through it at cold-start, and `routes/repair.rs`'s halted-run-portfolio-snapshot
route reconstructs a `PortfolioState` from inbox rows on demand via the same
`apply_fill` primitive. It is not, however, wired into any durable,
continuously-updated, restart-surviving read model — every current use
recomputes from scratch, in memory, per call. Bundle 4 reuses `apply_fill`/
`recompute_from_ledger` directly; it does not reimplement FIFO logic.

### 1.4 Marks for unrealized P&L

`routes/portfolio.rs::compute_broker_positions_pnl` already sources marks
from `md_bars` via `fetch_recent_completed_bars_for_strategy` (last completed
bar's close, default timeframe `"1D"`, overridable), feeding
`mqk_portfolio::unrealized_pnl_micros`. This is the only mark source in the
codebase for this purpose (no live-quote path) and is reused unchanged by
Bundle 4.

### 1.5 Daily-P&L baseline — reused unchanged

`mqk-db/src/account_equity_baseline.rs` (`sys_account_equity_baseline`, one
row per `trading_date`, idempotent last-write-wins upsert,
`upsert_account_equity_baseline`/`fetch_account_equity_baseline_for_date`)
is the accepted foundation. Its writer is `routes/control_plane.rs`'s
`"capture-account-equity-baseline"` ops action; its reader is
`routes/portfolio.rs::resolve_daily_pnl`. **Bundle 4 does not touch this
table, its writer, or its reader logic** — `daily_pnl`/`daily_pnl_truth_state`
on the new durable-summary route is populated by calling the existing
`resolve_daily_pnl` helper, not by recomputing daily P&L from the new
accounting projection.

### 1.6 Existing precedent for durable portfolio persistence

One precedent exists: `POST /api/v1/ops/repair/halted-run-portfolio-snapshot`
persists a JSON reconstruction into the generic `audit_events` table
(`event_type = "ops.repair.portfolio_snapshot"`), keyed by a UUIDv5 of
`run_id + max_fill_inbox_id`, computed fresh per call — a manual,
operator-triggered, HALTED-run-only action, not a first-class table and not
continuous. Bundle 4 supersedes this pattern with first-class, continuously
maintained tables; the repair route is out of scope for this bundle and is
left untouched.

### 1.7 Deterministic ID convention — reused

The established pattern, used throughout the codebase (`autonomous_daily_operation.rs`,
`control_plane.rs`'s account-equity-baseline action, `repair.rs`'s portfolio-snapshot
action, and multiple migrations), is:
```rust
Uuid::new_v5(&Uuid::NAMESPACE_DNS, format!("mqk.<domain>.v1|{field}|{field}|...").as_bytes())
```
Bundle 4's new identities follow this exact convention (§3).

### 1.8 Migrations

Highest committed migration is `0052_autonomous_daily_blocker_signature.sql`.
Convention: `NNNN_snake_case_description.sql`, zero-padded 4-digit sequential
ID, one manifest.json entry per file. Bundle 4's migration(s) begin at
`0053`, discovered fresh in B4-B (not hardcoded here, per manifest
discipline).

### 1.9 `control_plane.rs` is a plausible, precedented host

`control_plane.rs` already performs durable, non-route-trivial DB writes from
its `ops_action` dispatcher (`persist_arm_state_canonical`,
`upsert_account_equity_baseline`, restart-intent persistence) — it is not
arm/disarm/halt-only. This confirms the mission's constraint ("do not add
route-side persistence") is about the **read routes** (`routes/portfolio.rs`,
the new durable-summary routes), not a blanket ban on any route-adjacent
write — but per the binding decision below, Bundle 4 does not add a new
`control_plane.rs` action either; persistence is wired into the acceptance
seam itself (§2), and reads remain strictly GET-only.

## 2. Locked answers to the B4-A contract questions

**Canonical broker-snapshot acceptance seam.** There is no single existing
funnel. B4-C introduces one: `AppState::accept_external_broker_snapshot`
(exact name/signature decided in B4-C), which becomes the only place that
writes `AppState.broker_snapshot` for the `External` (real Alpaca) source.
Call sites (2), (3), and the periodic refresher in §1.1 are refactored to
call through it instead of writing the `RwLock` directly. The `Synthetic`
branch (1) and the dev-only injection routes (5) **never** call it and
**never** produce durable authoritative Paper+Alpaca portfolio truth — this
is the source-authority distinction the mission requires (§ PERSISTENCE
RULES). The on-demand repair fetch (4) is out of scope for Bundle 4 (it
serves a different, already-accepted repair flow) and is left unchanged.

**Real vs. synthesized provenance.** Carried forward from the existing
`BrokerSnapshotTruthSource` enum, stored durably as a `source` column on the
new snapshot table (`"external_alpaca"` for real, and if synthesized
snapshots are ever persisted for diagnostics, `"synthetic_diagnostic"` —
tagged distinctly and never eligible to satisfy authoritative readiness, per
mission requirement).

**Fill authority.** `oms_inbox`, unchanged, reused as-is (§1.2). No new fill
table.

**Realized P&L reconstructability from existing inbox rows.** Yes, in the
general case — `inbox_load_all_applied_for_run` plus `mqk_portfolio::apply_fill`
already prove this (§1.3, §1.6). The one case where it is *not* fully
reconstructable is a pre-existing broker position whose opening fill(s)
predate this run's (or any known run's) inbox history — handled by the
accounting-epoch state (§4), never by fabricating an opening fill.

**Is an additional normalized fill ledger required?** No (§1.2). A durable
**accounting projection** is required instead (§3) — not a second copy of
fill content, but the idempotently-applied summary derived from it.

**Marks for unrealized P&L.** `md_bars` via the existing
`compute_broker_positions_pnl` mark-lookup helper, reused unchanged (§1.4).

**Accounting epoch/completeness proof.** A position's realized P&L is
`complete` only when every open (and historically closed) lot contributing
to it can be traced to an applied inbox fill for a known run. Any
pre-existing position adopted without a matching opening fill in the
replayed inbox history is `incomplete` — position quantity/cost basis from
the broker snapshot remains authoritative, realized P&L for that symbol
becomes `unavailable` with an explicit reason, and no synthetic opening fill
is ever fabricated to balance the ledger (§4, §5).

**Pre-existing broker positions.** Treated exactly as above — visible via
the durable snapshot's position rows (broker-truth, unconditional), while
the accounting projection independently reports `accounting_epoch:
"incomplete"` for that symbol.

**Partial-fill replay deduplication.** `oms_inbox`'s existing `inbox_id`
(monotonic, durable) is the deterministic replay cursor: the durable
accounting projection stores a `last_applied_inbox_id` watermark per run and
only applies inbox rows with `inbox_id > watermark`, in ascending order,
inside one transaction per apply-batch. Re-running the same batch is a
no-op (watermark already past those rows) — this is the idempotency
mechanism, reusing `inbox_id` as the deterministic identity rather than
inventing a new one.

**Fees.** Carried through unchanged from `BrokerEvent::{Fill,PartialFill}.fee_micros`,
accumulated into a durable `fees_micros` running total alongside cash.

**Restart and re-reconcile.** On restart, the durable accounting projection
(cash, realized P&L, watermark) is read back directly — no in-memory
recomputation is required for it to be correct, because every apply was
already committed transactionally. `AppState.broker_snapshot`/`execution_snapshot`
remain in-memory-only and are repopulated by existing cold-start/reconcile
logic exactly as today; the durable projection is additive, not a
replacement input to reconcile.

**Existing routes to extend vs. new route.** `GET /api/v1/portfolio/summary`
is broker-snapshot-scoped (no `run_id`); the new durable truth is
run-scoped and historical (multiple snapshots over time). A **new,
dedicated, read-only route family** is required (§6) rather than overloading
`/summary`'s existing, differently-shaped response. `GET
/api/v1/execution/paper-lifecycle` is extended additively (it already
hardcodes `portfolio_truth_state = "in_memory_only_not_restart_surviving"`
today — this becomes a real, computed value once durable truth exists).

**Migrations.** One new migration file at the next sequential ID after
`0052` (discovered fresh in B4-B), following the existing naming and
manifest-registration convention.

## 3. Durable schema shape (B4-B implements)

Minimum architecture, per audit finding that a full duplicate fill ledger is
not justified:

- `sys_paper_portfolio_snapshots` — one row per accepted authoritative
  snapshot: `snapshot_id (uuid pk, deterministic v5)`, `captured_at_utc`,
  `deployment_mode`, `source` (`"external_alpaca"` | `"synthetic_diagnostic"`),
  `equity_micros`, `cash_micros`, `currency`, `truth_state`, `run_id
  (nullable)`, `operation_id (nullable)`.
- `sys_paper_portfolio_snapshot_positions` — child rows keyed by
  `snapshot_id`: `symbol`, `qty_signed`, `avg_entry_price_micros`,
  `provenance`.
- `sys_paper_portfolio_accounting_state` — one durable row per `run_id`:
  `cash_micros`, `realized_pnl_micros`, `fees_micros`,
  `last_applied_inbox_id` (replay watermark), `accounting_epoch`
  (`"complete"` | `"incomplete"`), `accounting_epoch_reason (nullable)`,
  `updated_at_utc`. Open FIFO lots are **not** stored as a separate durable
  table in this bundle — they are recomputed on read via
  `mqk_portfolio::recompute_from_ledger` replaying applied inbox rows up to
  the stored watermark (an O(fills-per-run) replay, bounded by a single
  run's lifetime; acceptable given the supervised, single-symbol,
  single-run-at-a-time operating lane this bundle targets). If a future
  bundle's proof shows this replay is too slow or the accounting epoch is
  incomplete, that is a `mark_unavailable`/`accounting_epoch: "incomplete"`
  condition, never a fabricated substitute.

Deterministic identities, all following §1.7's convention:
`Uuid::new_v5(NAMESPACE_DNS, "mqk.paper-portfolio-snapshot.v1|{captured_at_utc}|{run_id}|{source}")`
for `snapshot_id`; the accounting-state row is keyed directly by `run_id`
(already a stable, existing identity — no derived UUID needed).

## 4. Truth-state vocabulary (locked, closed set)

The mission's required minimum set is adopted verbatim, extended only where
the audit shows a genuinely distinct condition:

`active`, `not_found`, `snapshot_unavailable`, `snapshot_stale`,
`fill_history_incomplete`, `accounting_epoch_unavailable`, `mark_unavailable`,
`baseline_unavailable`, `reconcile_blocked`, `db_unavailable`, `query_failed`,
`unsupported_source`.

No states are dropped. `accounting_epoch` itself (§3) uses the narrower
closed pair `complete`/`incomplete` as a field on the durable row — the
broader vocabulary above governs the *API surface's* truth-state fields
(§6), consistent with the existing `pnl_truth_state`/`daily_pnl_truth_state`
precedent in `routes/portfolio.rs` (§1.4/§1.5), which this bundle extends
rather than replaces.

## 5. Binding design principles carried forward unmodified

Paper + Alpaca only; whole-share equity/ETF; single currency USD; FIFO via
`mqk-portfolio` (reused, not reimplemented); micros/integer arithmetic;
authoritative snapshot provenance (§2); idempotent fill/event persistence
(via `inbox_id` watermark, §2); restart-safe reconstruction (§2); read-only
routes never write (§6); no synthetic initial lots; no fabricated realized
P&L; no fabricated marks; no false zero values; explicit accounting
completeness state (§3/§4); explicit snapshot/fill/mark/baseline provenance
throughout.

## 6. API surface direction (B4-E implements)

New read-only route family, additive to the existing `routes/portfolio.rs`
router, not overloading `/summary`:

- `GET /api/v1/portfolio/durable-summary`
- `GET /api/v1/portfolio/durable-positions`
- `GET /api/v1/portfolio/durable-snapshots?limit=20`

`GET /api/v1/execution/paper-lifecycle` gains real (not hardcoded)
`portfolio_truth_state`/`pnl_truth_state` values sourced from the durable
projection, read-only, once B4-C/B4-D land.

## 7. No production behavior change in this patch

This patch is docs-only. `git diff --stat` for this commit touches only this
spec file and its guard script.
